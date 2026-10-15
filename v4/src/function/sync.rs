//! `SyncFunction`: how a built-in runs (R58), and `SyncCtx`, what it sees.
//!
//! A built-in reads the message through the run view and writes through the
//! run context, so a write is spliced into the view and marked, never
//! converted, until the next checkpoint. Crate-private: a host implements
//! [`Function`](super::Function), whose calls go through the async chain.

use crate::base::{Change, ErrorRecord, Result};
use crate::expr::arena::with_arena;
use crate::expr::{Expr, ExprEngine, FromValue};
use crate::function::Outcome;
use crate::state::RunContext;
use datavalue::{DataValue, OwnedDataValue};
use std::sync::Arc;

/// A built-in's call (R58). Crate-private: hosts implement `Function`.
pub(crate) trait SyncFunction {
    /// One call. Writes go through `cx` at once and stand even if the call
    /// then returns `Err`.
    fn call<'p>(&'p self, cx: &mut SyncCtx<'_, '_, 'p>) -> Result<Outcome>;
}

/// What a built-in sees: the run context, the run's facts, and the buffers
/// the recorder reads.
///
/// `'p` is the program's: the config, the expression engine and the ids live
/// as long as it does, so a result evaluated from the config can be spliced
/// into the view.
pub(crate) struct SyncCtx<'c, 'm, 'p> {
    /// The run context: the view to read, the writes to make.
    pub(crate) rc: &'c mut RunContext<'m, 'p>,
    /// The expression engine the program was compiled with.
    pub(crate) exprs: &'p ExprEngine,
    /// The message's payload.
    pub(crate) payload: &'c OwnedDataValue,
    /// The workflow's id.
    pub(crate) workflow_id: &'p str,
    /// The task's id.
    pub(crate) task_id: &'p str,
    /// Whether `Change`s are recorded (I67).
    pub(crate) capture: bool,
    /// Whether the run's trace keeps changes: only then does a built-in
    /// record a write the trace alone needs ([`Change::for_trace`]).
    pub(crate) traced: bool,
    /// This task's changes, read by the recorder.
    pub(crate) changes: &'c mut Vec<Change>,
    /// Set by a built-in that recorded a change the trace alone keeps, so
    /// the recorder makes the audit's copy without it, and only then.
    pub(crate) marked: bool,
    /// The message's error list.
    pub(crate) errors: &'c mut Vec<ErrorRecord>,
    /// The run's clock reading, RFC 3339, for error records.
    pub(crate) now: &'c str,
}

impl std::fmt::Debug for SyncCtx<'_, '_, '_> {
    /// The ids and the capture flag; not the context.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncCtx")
            .field("workflow_id", &self.workflow_id)
            .field("task_id", &self.task_id)
            .field("capture", &self.capture)
            .finish_non_exhaustive()
    }
}

impl<'p> SyncCtx<'_, '_, 'p> {
    /// Read-only evaluation against the view, in a pooled arena; a folded
    /// constant is lent.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Budget`](crate::base::ErrorKind::Budget) when the
    /// operation ceiling is exceeded; [`ErrorKind::Eval`](crate::base::ErrorKind::Eval)
    /// for any other failure, including a result that is not a `T`.
    pub(crate) fn eval<'e, T: FromValue>(&mut self, expr: &'e Expr<T>) -> Result<T::Out<'e>> {
        eval(self.rc, self.exprs, expr)
    }

    /// A condition: only a literal `true` passes; the error is returned for
    /// the caller to classify.
    pub(crate) fn test(&mut self, expr: &Expr) -> Result<bool> {
        test(self.rc, self.exprs, expr)
    }

    /// Push a `Change` when `capture` is on.
    pub(crate) fn change(
        &mut self,
        path: Arc<str>,
        old_value: OwnedDataValue,
        new_value: OwnedDataValue,
        removed: bool,
    ) {
        if self.capture {
            self.changes.push(Change {
                path,
                old_value,
                new_value,
                removed,
                replaced: false,
                trace_only: false,
            });
        }
    }
}

/// Evaluates `expr` against the view, in a pooled arena, so nothing it
/// allocates stays in the run arena: [`SyncCtx::eval`], for the engine's own
/// reads, such as `loop.over`. A folded constant is lent and reads no view.
pub(crate) fn eval<'e, T: FromValue>(
    rc: &mut RunContext<'_, '_>,
    exprs: &ExprEngine,
    expr: &'e Expr<T>,
) -> Result<T::Out<'e>> {
    if let Some(c) = expr.constant() {
        return Ok(T::lend(c));
    }
    rc.read(|_, root| {
        with_arena(|arena| {
            let view = exprs.eval_in(expr.logic(), root, arena)?;
            expr.convert(view).map(T::give)
        })
    })
}

/// Tests a condition against the view, in a pooled arena, so nothing it
/// allocates stays in the run arena: only a literal `true` passes, as v3's
/// `evaluate_condition`. The error is returned for the caller to classify.
/// A folded constant reads no view.
pub(crate) fn test(rc: &mut RunContext<'_, '_>, exprs: &ExprEngine, expr: &Expr) -> Result<bool> {
    if let Some(c) = expr.constant() {
        return Ok(matches!(c, OwnedDataValue::Bool(true)));
    }
    rc.read(|_, root| {
        with_arena(|arena| {
            exprs
                .eval_in(expr.logic(), root, arena)
                .map(|v| matches!(v, DataValue::Bool(true)))
        })
    })
}

/// An error record carrying a task's ids and the run's timestamp, as v3 fills
/// them: what a built-in records, and what the recorder records for it.
pub(crate) fn task_record(
    code: &str,
    message: String,
    workflow_id: &str,
    task_id: &str,
    now: &str,
) -> ErrorRecord {
    let mut record = ErrorRecord::new(code, message);
    record.workflow_id = Some(workflow_id.to_owned());
    record.task_id = Some(task_id.to_owned());
    record.timestamp = Some(now.to_owned());
    record
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::ErrorKind;
    use crate::expr::arena;
    use crate::state::Tuning;
    use serde_json::json;
    use std::borrow::Cow;

    const NOW: &str = "2026-10-14T00:00:00+00:00";

    /// Runs `f` with a context over `{data: {a: 1, t: true, zero: 0}}`; the
    /// changes it recorded.
    fn with_cx(capture: bool, f: impl FnOnce(&mut SyncCtx<'_, '_, '_>)) -> Vec<Change> {
        let exprs = ExprEngine::default();
        let mut slot = OwnedDataValue::from(&json!({
            "data": {"a": 1, "t": true, "zero": 0}, "metadata": {}, "temp_data": {}
        }));
        let (mut changes, mut errors) = (Vec::new(), Vec::new());
        let mut rc = RunContext::new(&mut slot, Tuning::default());
        f(&mut SyncCtx {
            rc: &mut rc,
            exprs: &exprs,
            payload: &OwnedDataValue::Null,
            workflow_id: "w",
            task_id: "t",
            capture,
            traced: false,
            changes: &mut changes,
            marked: false,
            errors: &mut errors,
            now: NOW,
        });
        rc.finish();
        changes
    }

    fn expr<T: FromValue>(raw: serde_json::Value) -> Expr<T> {
        ExprEngine::default().compile(&raw).expect("compiles")
    }

    #[test]
    fn eval_lends_a_constant_and_reads_the_view_as_written() {
        with_cx(true, |cx| {
            let five: Expr = expr(json!(5));
            assert!(matches!(cx.eval(&five), Ok(Cow::Borrowed(_))));

            let a: Expr = expr(json!({"cat": ["a=", {"var": "data.a"}]}));
            let run_arena =
                |cx: &mut SyncCtx<'_, '_, '_>| cx.rc.read(|owner, _| arena::used(&owner.arena));
            let before = run_arena(cx);
            assert_eq!(*cx.eval(&a).unwrap(), OwnedDataValue::from("a=1"));
            assert_eq!(
                run_arena(cx),
                before,
                "a read leaves the run arena as it was"
            );

            let path: crate::expr::Path = "data.a".parse().unwrap();
            cx.rc
                .edit(|e| e.set(&path, OwnedDataValue::from(2).to_arena(e.arena()), None));
            assert_eq!(*cx.eval(&a).unwrap(), OwnedDataValue::from("a=2"));

            let not_a_u64: Expr<u64> = expr(json!({"var": "data.t"}));
            let error = cx.eval(&not_a_u64).unwrap_err();
            assert_eq!(*error.kind(), ErrorKind::Eval);
            assert!(error.message().contains("did not give a u64"), "{error}");
        });
    }

    #[test]
    fn test_passes_only_a_literal_true() {
        with_cx(true, |cx| {
            for (raw, passes) in [
                (json!(true), true),
                (json!(1), false),
                (json!("true"), false),
                (json!({"var": "data.t"}), true),
                (json!({"var": "data.a"}), false),
                (json!({"var": "data.missing"}), false),
            ] {
                assert_eq!(cx.test(&expr(raw.clone())).unwrap(), passes, "{raw}");
            }
            let fails: Expr = expr(json!({"/": [1, {"var": "data.zero"}]}));
            assert_eq!(*cx.test(&fails).unwrap_err().kind(), ErrorKind::Eval);
        });
    }

    #[test]
    fn task_record_carries_the_ids_and_the_runs_timestamp() {
        let record = task_record("X_FAILED", "it failed".into(), "w", "t", NOW);
        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            r#"{"code":"X_FAILED","message":"it failed","path":null,"workflow_id":"w","task_id":"t","timestamp":"2026-10-14T00:00:00+00:00"}"#
        );
    }

    #[test]
    fn change_is_pushed_only_under_capture() {
        for capture in [true, false] {
            let changes = with_cx(capture, |cx| {
                cx.change("data.a".into(), 1.into(), 2.into(), false);
                cx.change("data.b".into(), 3.into(), OwnedDataValue::Null, true);
            });
            if capture {
                assert_eq!(
                    serde_json::to_string(&changes).unwrap(),
                    r#"[{"path":"data.a","old_value":1,"new_value":2},{"path":"data.b","old_value":3,"new_value":null,"removed":true}]"#
                );
            } else {
                assert!(changes.is_empty());
            }
        }
    }
}
