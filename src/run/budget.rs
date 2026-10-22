//! Run-level condition error policy, task/expression budgets, and host
//! accounting hooks.

use crate::base::{BudgetCause, Error, ErrorKind, ExprCause, Result};
use crate::expr::{Expr, ExprEngine, FromValue};
use bumpalo::Bump;
use datalogic_rs::Logic;
use datavalue::DataValue;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// How condition evaluation failures are handled during a run.
///
/// Configured on [`RunOptions`](super::RunOptions::with_condition_error_policy)
/// or as an engine-wide default on [`EngineConfig`](super::EngineConfig::with_condition_error_policy).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionErrorPolicy {
    /// Compatibility behavior: workflow, group, and task condition evaluation
    /// errors skip the gated step, and `filter` ordinary evaluation errors
    /// follow the task's `on_reject` policy ([`ErrorKind::Budget`] in `filter`
    /// still halts the run).
    #[default]
    Skip,
    /// Fail-closed: any condition evaluation error (workflow, group, task, or
    /// `filter`, whether [`ErrorKind::Eval`] or [`ErrorKind::Budget`]) halts
    /// the run immediately as [`RunStatus::Stopped`](super::RunStatus::Stopped)
    /// with no further task dispatches.
    Stop,
}

/// The kind of step about to be dispatched when charging a task budget or
/// consulting a [`RunAccountant`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DispatchKind<'a> {
    /// An inline built-in task (`map`, `validation`, `filter`, `log`, etc.).
    InlineBuiltin {
        /// Workflow ID.
        workflow_id: &'a str,
        /// Task ID.
        task_id: &'a str,
        /// Built-in function name.
        function: &'a str,
    },
    /// A host-registered async function (`Call`, `Parse`, or `Publish`).
    HostTask {
        /// Workflow ID.
        workflow_id: &'a str,
        /// Task ID.
        task_id: &'a str,
        /// Registered function name.
        function: &'a str,
    },
    /// One sweep of a workflow loop.
    LoopStep {
        /// Workflow ID.
        workflow_id: &'a str,
        /// Zero-based sweep index (`0, 1, 2, ...`).
        iteration: u64,
    },
    /// One element of a `for_each` fan-out.
    ForEachElement {
        /// Workflow ID.
        workflow_id: &'a str,
        /// Task ID.
        task_id: &'a str,
        /// Zero-based element index.
        index: usize,
    },
}

/// RAII guard returned by [`RunAccountant::before_task_dispatch`].
///
/// Holds an optional drop callback so host accountants can track in-flight
/// concurrency or release per-dispatch leases when a step finishes or is
/// cancelled (including when a `for_each` branch is dropped on fail-fast).
pub struct OccupancyGuard<'a> {
    on_drop: Option<Box<dyn FnOnce() + Send + 'a>>,
}

impl<'a> OccupancyGuard<'a> {
    /// A guard that does nothing when dropped.
    pub const fn noop() -> Self {
        Self { on_drop: None }
    }

    /// A guard that runs `on_drop` once when dropped.
    pub fn new(on_drop: impl FnOnce() + Send + 'a) -> Self {
        Self {
            on_drop: Some(Box::new(on_drop)),
        }
    }
}

impl Default for OccupancyGuard<'_> {
    fn default() -> Self {
        Self::noop()
    }
}

impl fmt::Debug for OccupancyGuard<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OccupancyGuard")
            .field("active", &self.on_drop.is_some())
            .finish()
    }
}

impl Drop for OccupancyGuard<'_> {
    fn drop(&mut self) {
        if let Some(f) = self.on_drop.take() {
            f();
        }
    }
}

/// Host-owned accounting and admission hook for a single run.
///
/// Implementations must be thread-safe (`Send + Sync`) because `for_each`
/// branches may invoke hooks concurrently across tasks.
pub trait RunAccountant: Send + Sync {
    /// Called immediately before dispatching an inline built-in, host task,
    /// loop sweep, or `for_each` element.
    ///
    /// Returning `Err(e)` aborts the dispatch before any side effects occur;
    /// the error is propagated as the step failure (typically [`ErrorKind::Budget`]).
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if the accountant rejects the dispatch.
    fn before_task_dispatch(&self, kind: DispatchKind<'_>) -> Result<OccupancyGuard<'_>> {
        let _ = kind;
        Ok(OccupancyGuard::noop())
    }

    /// Called after each metered expression evaluation (both `Ok` and `Err`)
    /// with the number of datalogic operations consumed.
    fn on_expr_evaluated(&self, ops: u64) {
        let _ = ops;
    }

    /// Called when a host function charges domain-specific work units via
    /// [`Ctx::accountant`](crate::function::Ctx::accountant).
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if the custom charge exceeds host limits.
    fn charge_custom(&self, kind: &str, units: u64) -> Result<()> {
        let _ = (kind, units);
        Ok(())
    }
}

/// Shared per-run task and expression op budget tracker.
pub(crate) struct TaskBudget<'a> {
    max_tasks: Option<u64>,
    tasks_used: AtomicU64,
    max_expr_ops: Option<u64>,
    expr_ops_used: AtomicU64,
    accountant: Option<&'a dyn RunAccountant>,
}

impl fmt::Debug for TaskBudget<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskBudget")
            .field("max_tasks", &self.max_tasks)
            .field("tasks_used", &self.tasks_used.load(Ordering::Relaxed))
            .field("max_expr_ops", &self.max_expr_ops)
            .field("expr_ops_used", &self.expr_ops_used.load(Ordering::Relaxed))
            .field("has_accountant", &self.accountant.is_some())
            .finish()
    }
}

impl<'a> TaskBudget<'a> {
    /// Creates a new per-run budget tracker.
    pub(crate) const fn new(
        max_tasks: Option<u64>,
        max_expr_ops: Option<u64>,
        accountant: Option<&'a dyn RunAccountant>,
    ) -> Self {
        Self {
            max_tasks,
            tasks_used: AtomicU64::new(0),
            max_expr_ops,
            expr_ops_used: AtomicU64::new(0),
            accountant,
        }
    }

    /// An unlimited tracker with no accountant, used by standalone test helpers.
    pub(crate) const fn unlimited() -> Self {
        Self::new(None, None, None)
    }

    /// The host accountant attached to this run, if any.
    pub(crate) const fn accountant(&self) -> Option<&'a dyn RunAccountant> {
        self.accountant
    }

    /// Whether any run-level budget or accountant is active on this run.
    pub(crate) const fn is_run_limited(&self) -> bool {
        self.max_tasks.is_some() || self.max_expr_ops.is_some() || self.accountant.is_some()
    }

    /// Atomically charges one dispatch against `max_tasks` and consults the
    /// [`RunAccountant`], returning its [`OccupancyGuard`] if admitted.
    pub(crate) fn charge_dispatch(&self, kind: DispatchKind<'_>) -> Result<OccupancyGuard<'_>> {
        if let Some(limit) = self.max_tasks {
            let admitted =
                self.tasks_used
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                        (used < limit).then_some(used + 1)
                    });
            if admitted.is_err() {
                let used = limit.saturating_add(1);
                return Err(Error::new(
                    ErrorKind::Budget,
                    format!("run task budget exceeded ({used} > {limit} max tasks)"),
                )
                .with_expr_cause(ExprCause::budget_exceeded(used, limit, None)));
            }
        }
        match self.accountant {
            Some(acc) => acc.before_task_dispatch(kind),
            None => Ok(OccupancyGuard::noop()),
        }
    }

    /// Evaluates `expr` against `root` in `arena`, charging the run's
    /// `expr_ops_budget` and notifying the [`RunAccountant`] on both `Ok` and `Err`.
    pub(crate) fn eval_view<'e, T: FromValue>(
        &self,
        exprs: &ExprEngine,
        expr: &'e Expr<T>,
        root: &DataValue<'_>,
        arena: &Bump,
    ) -> Result<T::Out<'e>> {
        if let Some(c) = expr.constant() {
            return Ok(T::lend(c));
        }
        let ops_limit = self.remaining_ops_limit(exprs);
        let (mut res, ops) = exprs.eval_view_metered(expr, root, arena, ops_limit);
        self.reconcile_ops(ops, &mut res);
        res
    }

    /// Evaluates `logic` against `root` in `arena`, charging the run's
    /// `expr_ops_budget` and notifying the [`RunAccountant`] on both `Ok` and `Err`.
    pub(crate) fn eval_in<'v>(
        &self,
        exprs: &ExprEngine,
        logic: &'v Logic,
        reads_secret: bool,
        root: &'v DataValue<'v>,
        arena: &'v Bump,
    ) -> Result<(&'v DataValue<'v>, Option<u64>)> {
        let ops_limit = self.remaining_ops_limit(exprs);
        let (mut res, ops) = exprs.eval_in_metered(logic, reads_secret, root, arena, ops_limit);
        self.reconcile_ops(ops, &mut res);
        res.map(|v| (v, ops))
    }

    /// Computes the per-evaluation op ceiling to pass to `datalogic-rs` when a
    /// run-level `expr_ops_budget` is active.
    fn remaining_ops_limit(&self, exprs: &ExprEngine) -> Option<u64> {
        let limit = self.max_expr_ops?;
        let used = self.expr_ops_used.load(Ordering::Relaxed);
        let remaining = limit.saturating_sub(used);
        #[cfg(feature = "budget")]
        {
            let engine_limit = exprs.logic().resolve_ops_budget(None);
            if engine_limit < remaining {
                return Some(engine_limit);
            }
        }
        let _ = exprs;
        Some(remaining)
    }

    /// Charges `ops` against `expr_ops_used`, notifies `accountant`, and
    /// upgrades or rewrites `res` if the run-level `expr_ops_budget` was
    /// exceeded.
    fn reconcile_ops<T>(&self, ops: Option<u64>, res: &mut Result<T>) {
        let Some(ops) = ops else {
            return;
        };
        if let Some(acc) = self.accountant {
            acc.on_expr_evaluated(ops);
        }
        let Some(limit) = self.max_expr_ops else {
            return;
        };
        let prev = self.expr_ops_used.fetch_add(ops, Ordering::Relaxed);
        let total = prev.saturating_add(ops);
        if total <= limit {
            return;
        }
        match res {
            Err(err) if *err.kind() == ErrorKind::Budget => {
                // `datalogic-rs` already halted at the exact AST node; update
                // the budget numbers on `ExprCause` to reflect the run-level
                // cumulative total and limit when the run ceiling was tighter.
                if let Some(mut cause) = err.expr_cause().cloned()
                    && cause.budget.is_none_or(|b| b.limit <= limit)
                {
                    cause.budget = Some(BudgetCause { used: total, limit });
                    cause.ops = Some(ops);
                    let updated = mem_take_error(err).with_expr_cause(cause);
                    *err = updated;
                }
            }
            _ => {
                *res = Err(Error::new(
                    ErrorKind::Budget,
                    format!("run expression ops budget exceeded ({total} > {limit} max ops)"),
                )
                .with_expr_cause(ExprCause::budget_exceeded(
                    total,
                    limit,
                    Some(ops),
                )));
            }
        }
    }
}

fn mem_take_error(err: &mut Error) -> Error {
    std::mem::replace(err, Error::new(ErrorKind::Eval, ""))
}
