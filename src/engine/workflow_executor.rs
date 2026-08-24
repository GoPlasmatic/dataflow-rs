//! # Workflow Execution Module
//!
//! This module handles the execution of workflows and their associated tasks.
//! It provides a clean separation between workflow orchestration and task execution.

use crate::engine::error::{
    DataflowError, ErrorContextConfig, ErrorInfo, Result, service_error_code,
};
use crate::engine::executor::{
    ArenaContext, evaluate_condition, evaluate_condition_in_arena, with_arena,
};
use crate::engine::functions::BoxedFunctionHandler;
use crate::engine::message::{AuditTrail, Change, Message};
use crate::engine::observer::{ExecutionObserver, TaskEvent};
use crate::engine::task::Task;
use crate::engine::task_executor::TaskExecutor;
use crate::engine::task_outcome::TaskOutcome;
use crate::engine::trace::{ExecutionStep, ExecutionTrace, StepTiming, duration_us_between};
use crate::engine::utils::{
    compute_path_parts, set_nested_value, set_nested_value_parts, strip_hash_prefix,
};
use crate::engine::workflow::{LoopConfig, Workflow};
use chrono::{DateTime, Utc};
use core::time::Duration;
use datalogic_rs::{Engine, Logic};
use datavalue::OwnedDataValue;
use log::{debug, error, info, warn};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Result of handling a task, including possible control flow signals
enum TaskControlFlow {
    /// Continue executing the next task
    Continue,
    /// Stop executing further tasks in this workflow (filter halt)
    HaltWorkflow,
}

/// Constants shared by every task in one pass over a workflow's task list.
///
/// Bundles the per-message timestamp with the loop counter so that threading
/// the counter through the task loop did not push `run_tasks_slice_in_arena`
/// and `handle_task_result` past clippy's argument-count threshold.
#[derive(Clone, Copy)]
struct PassCtx {
    /// The single `Utc::now()` read for this `process_message` call, shared by
    /// every `AuditTrail` it produces.
    now: DateTime<Utc>,
    /// Loop counter of the sweep this pass is, or `None` for a workflow
    /// without a `loop`.
    loop_counter: Option<i64>,
}

impl PassCtx {
    /// The single pass of a workflow without a `loop`.
    #[inline]
    fn once(now: DateTime<Utc>) -> Self {
        Self {
            now,
            loop_counter: None,
        }
    }
}

/// The two per-*task* values `handle_task_result` needs beyond the shared
/// [`PassCtx`].
///
/// Bundled rather than passed separately because `handle_task_result` already
/// sits at clippy's `too_many_arguments` threshold, and `PassCtx` cannot carry
/// them — it is per-pass and shared by every task in a sweep.
#[derive(Clone, Copy)]
struct TaskPass {
    /// The task-level `continue_on_error` flag.
    continue_on_error: bool,
    /// The task-level `terminal` flag — halt the workflow once this task has
    /// run, whatever it returned.
    terminal: bool,
    /// `message.errors.len()` immediately before this task ran, so the errors it
    /// contributed can be identified as the tail beyond this index.
    errors_before: usize,
}

/// One slice of a workflow's task list, plus the group state that spans slices.
///
/// Bundled into a single parameter because `run_tasks_slice_in_arena` already
/// sits at clippy's `too_many_arguments` threshold, and because the three
/// travel together: an absolute task index is `offset + i`, and `gate` is the
/// only thing that has to survive from one slice to the next.
struct TaskSlice<'a, 'arena> {
    /// The tasks to run — a sub-slice of `workflow.tasks`.
    tasks: &'arena [Task],
    /// Index of `tasks[0]` within `workflow.tasks`.
    offset: usize,
    /// Group state for the whole pass, shared across every slice in it.
    gate: &'a mut GroupGate,
}

/// Result of running one slice of a workflow's task list.
enum SliceOutcome {
    /// The slice ran to its end.
    Completed,
    /// A task halted the workflow — `TaskOutcome::Halt`, `Task::terminal`, or
    /// the end of a terminal group.
    Halted,
    /// A group condition was false and its span ends beyond this slice, so the
    /// caller must resume at this absolute task index.
    JumpTo(usize),
}

/// Tracks which task groups are currently open during one pass over a
/// workflow's task list.
///
/// Group spans are recorded at parse time on the task that opens them
/// (`Task::group_starts`), so the executor keeps walking a flat `&[Task]`.
/// This gate turns those spans back into control flow: evaluate a group's
/// condition **once** on entry, jump past the span when it is false, and halt
/// when a terminal group closes.
///
/// A workflow using no groups never pushes, so the gate costs one
/// `Vec::is_empty` check per task and never allocates.
#[derive(Default)]
struct GroupGate {
    /// `(end, terminal)` for each open group, outermost first.
    open: Vec<(usize, bool)>,
}

impl GroupGate {
    /// Close every open group whose span ends at or before `idx`, returning
    /// `true` if any of them was `terminal`.
    ///
    /// Driven by `end` rather than by a per-task close count because a jump can
    /// skip straight past the task that would have carried the count: with
    /// `group A { group B { t1 } }` and `B`'s condition false, nothing in `A`
    /// ever executes, yet `A` was entered and — if terminal — must still halt.
    fn close_through(&mut self, idx: usize) -> bool {
        let mut terminal = false;
        while let Some(&(end, is_terminal)) = self.open.last() {
            if end > idx {
                break;
            }
            self.open.pop();
            terminal |= is_terminal;
        }
        terminal
    }

    /// Evaluate the groups opening at `task`, outermost first. Returns
    /// `Some(end)` when one's condition is false and the cursor must jump past
    /// its span; the groups outside it stay open.
    fn enter<F>(&mut self, task: &Task, mut eval: F) -> Result<Option<usize>>
    where
        F: FnMut(&Arc<Logic>) -> Result<bool>,
    {
        for group in &task.group_starts {
            let entered = match group.compiled_condition.as_ref() {
                None => true,
                Some(compiled) => eval(compiled)?,
            };
            if !entered {
                return Ok(Some(group.end));
            }
            self.open.push((group.end, group.terminal));
        }
        Ok(None)
    }
}

/// Record the skip of every task in `workflow.tasks[from..to]` — the span of a
/// group whose condition was false.
///
/// The trace stays task-granular rather than growing a group-level step, so
/// `StepResult` and the npm wire type it mirrors are unchanged.
fn note_group_skip(
    mut trace: Option<&mut ExecutionTrace>,
    workflow: &Workflow,
    from: usize,
    to: usize,
    loop_counter: Option<i64>,
) {
    for task in &workflow.tasks[from..to.min(workflow.tasks.len())] {
        note_task_skip(trace.as_deref_mut(), &workflow.id, &task.id, loop_counter);
    }
}

/// Result of one pass over a workflow's task list.
enum PassOutcome {
    /// The workflow condition evaluated false — no task ran.
    ConditionFalse,
    /// Every task ran (or was individually skipped) to the end of the list.
    Completed,
    /// A task returned [`TaskOutcome::Halt`].
    Halted,
}

/// Return the index of the first task at or after `start` that is *not* a
/// synchronous built-in. Used to chunk `workflow.tasks` into sync-only
/// stretches that can share a single `ArenaContext`.
fn next_async_boundary(tasks: &[Task], start: usize) -> usize {
    let mut i = start;
    while i < tasks.len() && tasks[i].function.is_sync_builtin() {
        i += 1;
    }
    i
}

/// Log and (if tracing) record a whole-workflow skip. `reason` is only for the
/// debug log — `ExecutionStep::workflow_skipped` doesn't carry one, so a
/// rollout-bucket exclusion and a false condition are indistinguishable in the
/// trace, same as before this was factored out of its four call sites.
fn note_workflow_skip(trace: Option<&mut ExecutionTrace>, workflow_id: &str, reason: &str) {
    debug!("Skipping workflow {} - {}", workflow_id, reason);
    if let Some(t) = trace {
        t.add_step(ExecutionStep::workflow_skipped(workflow_id));
    }
}

/// Log and (if tracing) record a single task's condition skip.
///
/// The async task loop and the shared-arena one both reach this point with the
/// same state, and previously spelled the block out twice — every field added
/// to the skipped step had to be added in both places, with nothing to catch a
/// one-sided edit. Companion to [`note_workflow_skip`] above.
fn note_task_skip(
    trace: Option<&mut ExecutionTrace>,
    workflow_id: &str,
    task_id: &str,
    loop_counter: Option<i64>,
) {
    debug!("Skipping task {} - condition not met", task_id);
    if let Some(t) = trace {
        t.add_step(
            ExecutionStep::task_skipped(workflow_id, task_id).with_loop_counter(loop_counter),
        );
    }
}

/// Whether `workflow` serves this message's routing bucket.
///
/// A workflow with no `rollout`, or a message with no bucket, is admitted. The
/// missing-bucket case admits deliberately: every message any existing caller
/// builds has no bucket, and the wasm entry points have no way to set one, so
/// rejecting would silently stop those workflows running.
///
/// Nested `match` rather than a let-chain: MSRV is 1.85. See
/// `write_progress_metadata` below for the same reason.
fn rollout_admits(workflow: &Workflow, message: &Message) -> bool {
    match workflow.rollout {
        None => true,
        Some(r) => match message.routing_bucket() {
            None => true,
            Some(b) => r.accepts(b),
        },
    }
}

/// Whether `workflow` may join a shared-arena run of consecutive fully-sync
/// workflows.
///
/// A looping workflow is excluded even when every task is a sync built-in: its
/// sweeps run through `execute_inner`, which opens a fresh arena scope per
/// sweep. Bump arenas never free mid-scope, so sweeping inside one shared
/// scope would grow memory with the iteration count.
fn joins_sync_run(workflow: &Workflow) -> bool {
    workflow.fully_sync && workflow.loop_config.is_none()
}

/// Resolve the counter's pre-split write path, once per looping workflow.
///
/// `LogicCompiler` pre-splits `temp_data.{counter}` at build time. A workflow
/// constructed directly rather than through `Engine::builder` never got that
/// pass, so the parts are computed here instead — once, ahead of the sweep
/// loop, rather than re-formatted and re-split on every sweep.
///
/// An unnamed counter resolves to an empty slice, which `set_nested_value_parts`
/// treats as a no-op: the loop is still bounded, the value simply is not
/// exposed to JSONLogic (the audit trail carries it either way).
fn resolve_counter_parts(config: &LoopConfig) -> Arc<[Arc<str>]> {
    match &config.counter {
        Some(counter) if config.counter_parts.is_empty() => {
            compute_path_parts("temp_data", counter)
        }
        _ => Arc::clone(&config.counter_parts),
    }
}

/// Build a fresh `metadata.progress` object value.
fn new_progress_object(workflow_id: &str, task_id: &str, status: u16) -> OwnedDataValue {
    OwnedDataValue::Object(vec![
        (
            "workflow_id".to_string(),
            OwnedDataValue::String(workflow_id.to_string()),
        ),
        (
            "task_id".to_string(),
            OwnedDataValue::String(task_id.to_string()),
        ),
        (
            "status_code".to_string(),
            OwnedDataValue::from(u64::from(status)),
        ),
    ])
}

/// Overwrite a string slot by reusing its existing buffer where possible.
///
/// The ids written per task are drawn from a small, repeating set — in a loop
/// they are outright constant across every sweep — so the common case is
/// writing the value that is already there. Comparing first turns that case
/// into a no-op, and the mismatch case still reuses the allocation.
fn overwrite_str_in_place(slot: &mut OwnedDataValue, value: &str) {
    match slot {
        OwnedDataValue::String(existing) => {
            if existing != value {
                existing.clear();
                existing.push_str(value);
            }
        }
        _ => *slot = OwnedDataValue::String(value.to_string()),
    }
}

/// Overwrite the three fields of an existing 3-key `progress` object without
/// reallocating it. Returns `false` when the object's shape diverges from
/// `{workflow_id, task_id, status_code}`, in which case the caller replaces
/// the slot wholesale (partial overwrites here are harmless — the whole slot
/// gets replaced).
fn overwrite_progress_in_place(
    fields: &mut [(String, OwnedDataValue)],
    workflow_id: &str,
    task_id: &str,
    status: u16,
) -> bool {
    if fields.len() != 3 {
        return false;
    }
    let mut matched = 0;
    for (k, v) in fields.iter_mut() {
        match k.as_str() {
            "workflow_id" => {
                overwrite_str_in_place(v, workflow_id);
                matched += 1;
            }
            "task_id" => {
                overwrite_str_in_place(v, task_id);
                matched += 1;
            }
            "status_code" => {
                *v = OwnedDataValue::from(u64::from(status));
                matched += 1;
            }
            _ => {}
        }
    }
    matched == 3
}

/// Write `metadata.progress = {workflow_id, task_id, status_code}` with a
/// single tree walk. From the second task of a message onward the slot
/// already holds the expected 3-key object, so the three values are
/// overwritten in place, reusing the id `String` buffers — no allocation at
/// all once the shape settles. First write (or any shape divergence)
/// replaces the slot wholesale; a context whose `metadata` is missing or
/// non-Object falls back to the generic `set_nested_value` writer, which
/// creates intermediate containers as needed.
fn write_progress_metadata(
    context: &mut OwnedDataValue,
    workflow_id: &str,
    task_id: &str,
    status: u16,
) {
    // Nested `if let` rather than a let-chain: let-chains are stable only from
    // Rust 1.88 and this crate's MSRV is 1.85. Keep it that way.
    if let OwnedDataValue::Object(top) = context {
        if let Some((_, OwnedDataValue::Object(meta))) =
            top.iter_mut().find(|(k, _)| k == "metadata")
        {
            match meta.iter_mut().find(|(k, _)| k == "progress") {
                Some((_, slot)) => {
                    if let OwnedDataValue::Object(fields) = slot {
                        if overwrite_progress_in_place(fields, workflow_id, task_id, status) {
                            return;
                        }
                    }
                    *slot = new_progress_object(workflow_id, task_id, status);
                }
                None => {
                    meta.push((
                        "progress".to_string(),
                        new_progress_object(workflow_id, task_id, status),
                    ));
                }
            }
            return;
        }
    }
    set_nested_value(
        context,
        "metadata.progress",
        new_progress_object(workflow_id, task_id, status),
    );
}

/// Build one context record for a failed task.
///
/// `workflow_id`, `task_id` and `status` come from the executor rather than from
/// the `ErrorInfo`: `validation` builds its entries with `ErrorInfo::simple_ref`,
/// which leaves both ids `None`, and `ErrorInfo` carries no status at all.
///
/// The error `message` and the operator-only `detail` are deliberately omitted —
/// this value lands in `Message.context`, which is serialized back to callers.
fn new_error_record(workflow_id: &str, task_id: &str, code: &str, status: u16) -> OwnedDataValue {
    OwnedDataValue::Object(vec![
        ("workflow_id".to_string(), OwnedDataValue::from(workflow_id)),
        ("task_id".to_string(), OwnedDataValue::from(task_id)),
        ("code".to_string(), OwnedDataValue::from(code)),
        (
            "status".to_string(),
            OwnedDataValue::from(u64::from(status)),
        ),
    ])
}

/// Take `node` as an `Object`, replacing whatever non-`Object` sat there.
///
/// Normalise first, then destructure — the inverse order (match, then assign in
/// the fallback arm and re-match the same binding) is NLL problem case #3 and
/// does not compile.
fn as_object_slot(node: &mut OwnedDataValue) -> &mut Vec<(String, OwnedDataValue)> {
    if !matches!(node, OwnedDataValue::Object(_)) {
        *node = OwnedDataValue::Object(Vec::new());
    }
    match node {
        OwnedDataValue::Object(fields) => fields,
        _ => unreachable!("just normalised to an Object"),
    }
}

/// Append one record per entry in `new_errors` to the configured context path,
/// keeping at most `cfg.limit` of them.
///
/// Hand-walks to the slot the way [`write_progress_metadata`] does. The generic
/// [`set_nested_value`] cannot express an append — it indexes arrays by numeric
/// segment and `Null`-pads the gap — and silently no-ops when a non-numeric
/// segment meets an `Array`. A slot holding something other than an `Array` is
/// replaced wholesale rather than skipped, so the shape a workflow author reads
/// is predictable even if a `map` task wrote over the path first.
///
/// The array is created lazily, only when there is something to push, so a
/// message whose tasks all succeed keeps the exact wire shape it had before the
/// option existed — the key is absent, not `[]`.
fn append_error_records(
    context: &mut OwnedDataValue,
    cfg: &ErrorContextConfig,
    workflow_id: &str,
    task_id: &str,
    status: u16,
    new_errors: &[ErrorInfo],
) {
    if new_errors.is_empty() {
        return;
    }
    // Walk to the parent of the final segment, creating containers as needed,
    // then take the slot itself.
    let Some((last, parents)) = cfg.path_parts.split_last() else {
        return;
    };

    let mut node = context;
    for part in parents {
        let key = strip_hash_prefix(part);
        if !matches!(node, OwnedDataValue::Object(_)) {
            // A scalar or array on the way down cannot hold a named child. The
            // host declared the engine owns this path, so resolve the conflict
            // in favour of the records rather than dropping them — but say so:
            // whatever was written here is being discarded.
            warn!(
                "error context path `{}` runs through a non-object at `{}` — replacing it",
                cfg.path, key
            );
        }
        let fields = as_object_slot(node);
        let idx = match fields.iter().position(|(k, _)| k == key) {
            Some(i) => i,
            None => {
                fields.push((key.to_string(), OwnedDataValue::Object(Vec::new())));
                fields.len() - 1
            }
        };
        node = &mut fields[idx].1;
    }

    let key = strip_hash_prefix(last);
    let fields = as_object_slot(node);
    let idx = match fields.iter().position(|(k, _)| k == key) {
        Some(i) => i,
        None => {
            fields.push((key.to_string(), OwnedDataValue::Array(Vec::new())));
            fields.len() - 1
        }
    };
    let slot = &mut fields[idx].1;
    if !matches!(slot, OwnedDataValue::Array(_)) {
        warn!(
            "error context path `{}` held a non-array — replacing it",
            cfg.path
        );
        *slot = OwnedDataValue::Array(Vec::new());
    }
    let OwnedDataValue::Array(items) = slot else {
        unreachable!("just ensured an Array");
    };

    for error in new_errors {
        items.push(new_error_record(workflow_id, task_id, &error.code, status));
    }
    // Keep-newest: a looping workflow with a failing body would otherwise grow
    // this list once per sweep, and `Message.context` is deep-cloned into every
    // trace snapshot.
    if items.len() > cfg.limit {
        items.drain(..items.len() - cfg.limit);
    }
}

/// Handles the execution of workflows and their tasks
///
/// The `WorkflowExecutor` is responsible for:
/// - Evaluating workflow conditions
/// - Orchestrating task execution within workflows
/// - Managing workflow-level error handling
/// - Recording audit trails
pub struct WorkflowExecutor {
    /// Task executor for executing individual tasks
    task_executor: Arc<TaskExecutor>,
    /// Shared datalogic engine for condition evaluation
    engine: Arc<Engine>,
    /// Optional per-task observer. `None` keeps the instrumentation — and its
    /// clock reads — entirely out of the dispatch path.
    observer: Option<Arc<dyn ExecutionObserver>>,
    /// Optional context path where per-task failure codes are mirrored. `None`
    /// keeps the whole mechanism out of the dispatch path.
    error_context: Option<Arc<ErrorContextConfig>>,
}

impl WorkflowExecutor {
    /// Create a new WorkflowExecutor
    pub fn new(task_executor: Arc<TaskExecutor>, engine: Arc<Engine>) -> Self {
        Self {
            task_executor,
            engine,
            observer: None,
            error_context: None,
        }
    }

    /// Attach an observer to an existing executor. Replaces any previous one.
    pub fn with_observer(mut self, observer: Arc<dyn ExecutionObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// The registered observer, if any.
    ///
    /// Used by `Engine::with_new_workflows` to carry the observer across a hot
    /// reload — without it, metrics would stop silently at the first reload.
    pub fn observer(&self) -> Option<&Arc<dyn ExecutionObserver>> {
        self.observer.as_ref()
    }

    /// Attach an error-context path to an existing executor. Replaces any
    /// previous one.
    pub(crate) fn with_error_context(mut self, cfg: Arc<ErrorContextConfig>) -> Self {
        self.error_context = Some(cfg);
        self
    }

    /// The configured error-context path, if any.
    ///
    /// Used by `Engine`'s executor rebuilds to carry the setting across a hot
    /// reload or a `with_observer` call — without it, failure codes would stop
    /// being recorded silently.
    pub(crate) fn error_context(&self) -> Option<&Arc<ErrorContextConfig>> {
        self.error_context.as_ref()
    }

    /// Emit a task event, deriving the status from the dispatch result.
    ///
    /// Called before `handle_task_result`, which takes `result` by value and
    /// whose `?` propagates on a hard failure — emitting afterwards would
    /// silently drop exactly the tasks a host most wants timed.
    #[inline]
    fn emit_task_event(
        &self,
        workflow: &Workflow,
        task: &Task,
        result: &Result<(TaskOutcome, Vec<Change>)>,
        started_at: Option<DateTime<Utc>>,
    ) {
        if let Some(observer) = self.observer.as_ref() {
            let status = match result {
                Ok((outcome, _)) => outcome.audit_status(),
                Err(_) => Some(500),
            };
            let duration = started_at
                .map(|s| Duration::from_micros(duration_us_between(s, Utc::now())))
                .unwrap_or_default();
            observer.task_finished(&TaskEvent {
                workflow_id: &workflow.id,
                task_id: &task.id,
                function: task.function.function_name(),
                status,
                duration,
            });
        }
    }

    /// Clock read for the observer, only when one is attached.
    ///
    /// Gated so that `process_message`'s documented "one `Utc::now()` per
    /// message" holds for every caller that has not opted in.
    #[inline]
    fn observer_clock(&self) -> Option<DateTime<Utc>> {
        self.observer.as_ref().map(|_| Utc::now())
    }

    /// Get a clone of the task_functions Arc for reuse in new engines
    pub fn task_functions(&self) -> Arc<HashMap<String, BoxedFunctionHandler>> {
        self.task_executor.task_functions()
    }

    /// Execute a workflow if its condition is met
    ///
    /// This method:
    /// 1. Evaluates the workflow condition
    /// 2. Executes tasks sequentially if condition is met
    /// 3. Handles error recovery based on workflow configuration
    /// 4. Updates message metadata and audit trail
    ///
    /// # Arguments
    /// * `workflow` - The workflow to execute
    /// * `message` - The message being processed
    ///
    /// # Returns
    /// * `Result<bool>` - Ok(true) if workflow was executed, Ok(false) if skipped, Err on failure
    pub async fn execute(
        &self,
        workflow: &Workflow,
        message: &mut Message,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        self.execute_inner(workflow, message, None, now).await
    }

    /// Execute a workflow with step-by-step tracing
    ///
    /// Similar to `execute` but records execution steps for debugging.
    pub async fn execute_with_trace(
        &self,
        workflow: &Workflow,
        message: &mut Message,
        trace: &mut ExecutionTrace,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        self.execute_inner(workflow, message, Some(trace), now)
            .await
    }

    /// Run `workflow` against `message`: the rollout gate, then either a single
    /// pass over the task list or — for a workflow carrying a `loop` — a
    /// bounded sweep loop.
    ///
    /// `trace` is `None` for the production path and `Some(&mut trace)` for the
    /// debug path; stepping is the only behavioural difference between them.
    async fn execute_inner(
        &self,
        workflow: &Workflow,
        message: &mut Message,
        mut trace: Option<&mut ExecutionTrace>,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        // Traffic-split gate, ahead of any arena work so an excluded workflow
        // costs no `ArenaContext::from_owned` walk. Reuses the existing skipped
        // path verbatim, so an excluded workflow is indistinguishable from a
        // false condition.
        if !rollout_admits(workflow, message) {
            note_workflow_skip(trace.as_deref_mut(), &workflow.id, "outside rollout bucket");
            return Ok(false);
        }

        if let Some(loop_config) = workflow.loop_config.as_ref() {
            return self
                .execute_loop(workflow, loop_config, message, trace, now)
                .await;
        }

        match self
            .execute_pass(workflow, message, trace.as_deref_mut(), PassCtx::once(now))
            .await
        {
            Ok(PassOutcome::ConditionFalse) => {
                // Last use of `trace` on this path — no reborrow needed.
                note_workflow_skip(trace, &workflow.id, "condition not met");
                Ok(false)
            }
            Ok(_) => {
                info!("Successfully completed workflow: {}", workflow.id);
                Ok(true)
            }
            Err(e) => {
                // Single-channel contract: every error appears in
                // `message.errors`. The `Result::Err` return only signals to
                // the caller that we stopped before processing further
                // workflows. The workflow-level wrapper records workflow
                // context that the underlying task error doesn't carry.
                if self.record_workflow_error(workflow, message, &e) {
                    Err(e)
                } else {
                    Ok(true)
                }
            }
        }
    }

    /// Drive a looping workflow: repeat [`Self::execute_pass`] while the
    /// counter is below `max` and the workflow condition holds.
    ///
    /// Per-sweep order — write counter, check bound, check condition, run
    /// tasks, advance counter — is the documented contract. The counter is in
    /// `temp_data` before the first condition evaluation, so a condition that
    /// indexes by it works on sweep 0.
    ///
    /// Returns `Ok(false)` only when no sweep ever ran, which is what a
    /// condition-skipped workflow reports.
    async fn execute_loop(
        &self,
        workflow: &Workflow,
        config: &LoopConfig,
        message: &mut Message,
        mut trace: Option<&mut ExecutionTrace>,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let mut counter = config.init;
        let mut sweeps_run: u32 = 0;
        let counter_parts = resolve_counter_parts(config);

        loop {
            // Written before the bound and condition checks so a condition
            // indexing by the counter — the per-item pattern — resolves on the
            // very first sweep. No arena refresh is needed: `execute_pass`
            // builds its `ArenaContext` from `message.context` after this write.
            set_nested_value_parts(
                &mut message.context,
                &counter_parts,
                OwnedDataValue::from_i64(counter),
            );

            // `>=`, not `>`, and that is load-bearing for termination rather
            // than a style choice. `increment >= 1` is validated at build time
            // and the advance below saturates, so the counter strictly
            // increases until it pins at `i64::MAX` — which satisfies
            // `>= config.max` for every representable `max`. With `>` a loop
            // whose counter saturates would spin forever.
            if counter >= config.max {
                // Normal completion: `max` is always author-supplied, so
                // reaching it is the stated bound rather than a runaway. A
                // condition that was still true wanted to keep going, which is
                // worth a log line but not an error.
                if workflow.compiled_condition.is_some() {
                    warn!(
                        "Workflow {} stopped at its loop bound (max {}) with the condition \
                         still true after {} sweep(s)",
                        workflow.id, config.max, sweeps_run
                    );
                }
                break;
            }

            let pass = PassCtx {
                now,
                loop_counter: Some(counter),
            };

            match self
                .execute_pass(workflow, message, trace.as_deref_mut(), pass)
                .await
            {
                Ok(PassOutcome::ConditionFalse) => {
                    if sweeps_run == 0 {
                        // Never entered: indistinguishable from a plain
                        // condition-skipped workflow, and reported as one.
                        note_workflow_skip(trace.as_deref_mut(), &workflow.id, "condition not met");
                    } else {
                        debug!(
                            "Workflow {} loop exited at counter {} - condition no longer met",
                            workflow.id, counter
                        );
                    }
                    break;
                }
                Ok(PassOutcome::Halted) => {
                    sweeps_run += 1;
                    debug!(
                        "Workflow {} loop halted at counter {}",
                        workflow.id, counter
                    );
                    break;
                }
                Ok(PassOutcome::Completed) => {
                    sweeps_run += 1;
                }
                Err(e) => {
                    sweeps_run += 1;
                    // Same single-channel contract as the non-looping path. On
                    // `continue_on_error` the loop advances past the failing
                    // sweep rather than abandoning the rest — the per-item case
                    // wants item 8 processed after item 7 failed.
                    if self.record_workflow_error(workflow, message, &e) {
                        return Err(e);
                    }
                }
            }

            counter = counter.saturating_add(config.increment);
        }

        if sweeps_run > 0 {
            info!(
                "Successfully completed workflow: {} ({} loop sweep(s))",
                workflow.id, sweeps_run
            );
        }
        Ok(sweeps_run > 0)
    }

    /// One pass over `workflow.tasks`: evaluate the workflow condition, then
    /// run the task list once. This is the whole of a non-looping workflow, and
    /// one sweep of a looping one.
    ///
    /// The workflow condition is folded into the *first* sync stretch's arena
    /// scope: one `ArenaContext::from_owned` walk serves both the condition
    /// eval and the leading run of sync built-in tasks. The owned path
    /// (`eval_to_owned`) deep-borrowed the entire context — including the
    /// heavy `data.input` payload — for the condition, and `execute_tasks`
    /// then walked the same context again to build the first stretch's arena
    /// form. Mixed sync+async workflows now pay one walk where they paid two.
    /// No `.await` occurs inside the scope, preserving the `!Send` arena
    /// invariant.
    async fn execute_pass(
        &self,
        workflow: &Workflow,
        message: &mut Message,
        mut trace: Option<&mut ExecutionTrace>,
        pass: PassCtx,
    ) -> Result<PassOutcome> {
        /// Outcome of the folded condition-plus-first-stretch arena scope.
        enum FirstStretch {
            /// Workflow condition evaluated false — skip the workflow.
            Skipped,
            /// A filter task halted the workflow inside the first stretch.
            Halted,
            /// Continue with the remaining tasks, resuming at this index —
            /// the first async boundary, or further on when a skipped group's
            /// span reached past it.
            Continue(usize),
        }

        let tasks = &workflow.tasks;
        let first_boundary = next_async_boundary(tasks, 0);
        // One gate for the whole pass: a group can open in the folded first
        // stretch and close somewhere in the async tail.
        let mut gate = GroupGate::default();

        let first: Result<FirstStretch> =
            if workflow.compiled_condition.is_none() && first_boundary == 0 {
                // No condition and the workflow leads with an async task —
                // nothing to fold; don't build an arena context for nothing.
                Ok(FirstStretch::Continue(0))
            } else {
                with_arena(|arena| -> Result<FirstStretch> {
                    let mut arena_ctx = ArenaContext::from_owned(&message.context, arena);

                    let should_execute = match workflow.compiled_condition.as_ref() {
                        None => true,
                        Some(compiled) => evaluate_condition_in_arena(
                            &self.engine,
                            Some(compiled),
                            arena_ctx.as_data_value(),
                            arena,
                        )?,
                    };
                    if !should_execute {
                        return Ok(FirstStretch::Skipped);
                    }
                    if first_boundary == 0 {
                        return Ok(FirstStretch::Continue(0));
                    }
                    let outcome = self.run_tasks_slice_in_arena(
                        TaskSlice {
                            tasks: &tasks[..first_boundary],
                            offset: 0,
                            gate: &mut gate,
                        },
                        workflow,
                        message,
                        &mut arena_ctx,
                        trace.as_deref_mut(),
                        pass,
                    )?;
                    Ok(match outcome {
                        SliceOutcome::Halted => FirstStretch::Halted,
                        SliceOutcome::JumpTo(target) => FirstStretch::Continue(target),
                        SliceOutcome::Completed => FirstStretch::Continue(first_boundary),
                    })
                })
            };

        // Drive the remaining (async-containing) tail. The workflow-level error
        // contract lives in the caller, which is the one place that knows
        // whether this pass was a whole workflow or one sweep of a loop.
        match first? {
            FirstStretch::Skipped => Ok(PassOutcome::ConditionFalse),
            FirstStretch::Halted => Ok(PassOutcome::Halted),
            FirstStretch::Continue(resume_at) => {
                let halted = self
                    .execute_tasks(workflow, message, trace, pass, resume_at, &mut gate)
                    .await?;
                Ok(if halted {
                    PassOutcome::Halted
                } else {
                    PassOutcome::Completed
                })
            }
        }
    }

    /// Record a `WORKFLOW_ERROR` to `message.errors` and log at the level
    /// `continue_on_error` implies. Returns `true` when the caller should stop
    /// processing further workflows (i.e. `continue_on_error` is `false`).
    ///
    /// Shared by `execute_inner` (returns from its own `Result<bool>`) and
    /// `execute_sync_workflow_run` (returns from its `with_arena` closure or
    /// continues the loop) — the recording and log-level decision are
    /// identical; only what happens next differs by call site.
    fn record_workflow_error(
        &self,
        workflow: &Workflow,
        message: &mut Message,
        e: &DataflowError,
    ) -> bool {
        message.errors.push(
            ErrorInfo::builder(
                "WORKFLOW_ERROR",
                format!("Workflow {} error: {}", workflow.id, e),
            )
            .workflow_id(&workflow.id)
            .build(),
        );

        if workflow.continue_on_error {
            warn!(
                "Workflow {} encountered error but continuing: {:?}",
                workflow.id, e
            );
            false
        } else {
            error!("Workflow {} failed: {:?}", workflow.id, e);
            true
        }
    }

    /// Execute the tasks of a workflow from index `start` onward.
    ///
    /// Groups consecutive synchronous built-in tasks into a single
    /// `with_arena` scope so the arena form of `message.context` is built
    /// once at the start of the stretch and reused across `parse_json`,
    /// `map`, `validation`, `log`, and `filter`. Async tasks (HTTP, Kafka,
    /// custom handlers) break the stretch — the arena flushes any pending
    /// state back to `OwnedDataValue` automatically (since each sync task
    /// already mutates `message.context` in place) and the next stretch
    /// rebuilds the arena form.
    ///
    /// `start` is non-zero when `execute_inner` already ran the leading sync
    /// stretch inside the folded condition scope.
    ///
    /// When `trace` is `Some`, the loop also records `ExecutionStep` entries
    /// after each task (skipped/executed) including per-mapping snapshots
    /// for `Map` tasks.
    ///
    /// Returns `Ok(true)` when a task halted the workflow.
    async fn execute_tasks(
        &self,
        workflow: &Workflow,
        message: &mut Message,
        mut trace: Option<&mut ExecutionTrace>,
        pass: PassCtx,
        start: usize,
        gate: &mut GroupGate,
    ) -> Result<bool> {
        let tasks = &workflow.tasks;
        let mut idx = start;
        while idx < tasks.len() {
            let stretch_end = next_async_boundary(tasks, idx);

            if stretch_end > idx {
                // Run [idx, stretch_end) as a sync stretch inside one arena.
                match self.run_sync_stretch(
                    TaskSlice {
                        tasks: &tasks[idx..stretch_end],
                        offset: idx,
                        gate,
                    },
                    workflow,
                    message,
                    trace.as_deref_mut(),
                    pass,
                )? {
                    SliceOutcome::Halted => return Ok(true),
                    // A group opening inside the stretch was skipped and its
                    // span reaches past the stretch — resume where it ends.
                    SliceOutcome::JumpTo(target) => {
                        idx = target;
                        continue;
                    }
                    SliceOutcome::Completed => idx = stretch_end,
                }
            }

            if idx < tasks.len() {
                // Single async task (or non-sync-builtin) at `idx`.
                let task = &tasks[idx];

                if gate.close_through(idx) {
                    return Ok(true);
                }
                let jump = gate.enter(task, |compiled| {
                    evaluate_condition(&self.engine, Some(compiled), &message.context)
                })?;
                if let Some(target) = jump {
                    note_group_skip(
                        trace.as_deref_mut(),
                        workflow,
                        idx,
                        target,
                        pass.loop_counter,
                    );
                    idx = target;
                    continue;
                }

                let should_execute = evaluate_condition(
                    &self.engine,
                    task.compiled_condition.as_ref(),
                    &message.context,
                )?;

                if !should_execute {
                    note_task_skip(
                        trace.as_deref_mut(),
                        &workflow.id,
                        &task.id,
                        pass.loop_counter,
                    );
                    idx += 1;
                    continue;
                }

                // Clock reads only when a trace is live or an observer is
                // attached, so the plain path keeps its documented
                // one-`Utc::now()`-per-message invariant.
                let trace_start = if trace.is_some() {
                    Some(Utc::now())
                } else {
                    None
                };
                let obs_start = trace_start.or_else(|| self.observer_clock());

                // Sampled before the body runs — see the sync-stretch site.
                let errors_before = message.errors.len();

                let result = self.task_executor.execute(task, message).await;

                // Before `handle_task_result`, whose `?` would drop failed tasks.
                self.emit_task_event(workflow, task, &result, obs_start);

                // No arena refresh here: no `ArenaContext` is live on this path,
                // and `run_sync_stretch` rebuilds one from `message.context` at
                // the start of the next stretch.
                let control_flow = self.handle_task_result(
                    result,
                    &workflow.id_arc,
                    &task.id_arc,
                    TaskPass {
                        continue_on_error: task.continue_on_error,
                        terminal: task.terminal,
                        errors_before,
                    },
                    message,
                    pass,
                )?;

                // Async tasks at the boundary have no per-mapping snapshots —
                // they're either HTTP/Kafka/Enrich or a custom handler.
                if let Some(t) = trace.as_deref_mut() {
                    let started_at = trace_start.unwrap_or(pass.now);
                    t.add_executed_step(
                        &workflow.id,
                        &task.id,
                        message,
                        StepTiming {
                            started_at,
                            duration_us: duration_us_between(started_at, Utc::now()),
                        },
                        None,
                        pass.loop_counter,
                    );
                }

                if matches!(control_flow, TaskControlFlow::HaltWorkflow) {
                    return Ok(true);
                }
                idx += 1;
            }
        }

        // A terminal group closing on the last task still has to halt: for a
        // workflow carrying a `loop`, halting breaks the loop where completing
        // would start another sweep.
        Ok(gate.close_through(tasks.len()))
    }

    /// Execute a contiguous run of sync-builtin tasks inside one
    /// `with_arena` scope. The arena context is built once at the start and
    /// refreshed in place after each mutating task. Returns `Ok(true)` if a
    /// filter task halted the workflow.
    ///
    /// This is the single-workflow entry; the cross-workflow path
    /// (`execute_sync_workflow_run`) shares the same task loop via
    /// `run_tasks_slice_in_arena` but carries one `ArenaContext` across several
    /// workflows.
    fn run_sync_stretch(
        &self,
        slice: TaskSlice<'_, '_>,
        workflow: &Workflow,
        message: &mut Message,
        trace: Option<&mut ExecutionTrace>,
        pass: PassCtx,
    ) -> Result<SliceOutcome> {
        with_arena(|arena| -> Result<SliceOutcome> {
            let mut arena_ctx = ArenaContext::from_owned(&message.context, arena);
            self.run_tasks_slice_in_arena(slice, workflow, message, &mut arena_ctx, trace, pass)
        })
    }

    /// Run `tasks` against an already-built `ArenaContext`, evaluating each
    /// task's condition in-arena and refreshing the cache after each mutating
    /// task. Returns `Ok(true)` if a filter task halted the workflow.
    ///
    /// Factored out of `run_sync_stretch` so both the single-workflow stretch
    /// and the cross-workflow shared-arena run (`execute_sync_workflow_run`)
    /// share one implementation. The caller owns the `ArenaContext` lifetime,
    /// so the cross-workflow path can reuse the same arena form of
    /// `message.context` across consecutive workflows instead of rebuilding it.
    fn run_tasks_slice_in_arena<'arena>(
        &self,
        slice: TaskSlice<'_, 'arena>,
        workflow: &Workflow,
        message: &mut Message,
        arena_ctx: &mut ArenaContext<'arena>,
        mut trace: Option<&mut ExecutionTrace>,
        pass: PassCtx,
    ) -> Result<SliceOutcome> {
        let arena = arena_ctx.arena();
        let TaskSlice {
            tasks,
            offset,
            gate,
        } = slice;
        let slice_end = offset + tasks.len();

        let mut i = 0;
        while i < tasks.len() {
            let task = &tasks[i];
            let abs = offset + i;

            // Close any group whose span ended before this task. A terminal one
            // ends the workflow here, before the next task runs.
            if gate.close_through(abs) {
                return Ok(SliceOutcome::Halted);
            }

            // Open the groups that start here, evaluating each condition once.
            // A false one skips the whole span without touching the member
            // tasks' own conditions.
            let jump = gate.enter(task, |compiled| {
                evaluate_condition_in_arena(
                    &self.engine,
                    Some(compiled),
                    arena_ctx.as_data_value(),
                    arena,
                )
            })?;
            if let Some(target) = jump {
                note_group_skip(
                    trace.as_deref_mut(),
                    workflow,
                    abs,
                    target,
                    pass.loop_counter,
                );
                if target >= slice_end {
                    return Ok(SliceOutcome::JumpTo(target));
                }
                i = target - offset;
                continue;
            }

            // Task condition — evaluate against the arena form so we don't
            // re-borrow the thread-local `RefCell`. A `None` compiled
            // condition (compiler folds the default literal `true` to
            // `None`) skips both the eval and the per-task arena context
            // slice build.
            let should_execute = match task.compiled_condition.as_ref() {
                None => true,
                Some(compiled) => evaluate_condition_in_arena(
                    &self.engine,
                    Some(compiled),
                    arena_ctx.as_data_value(),
                    arena,
                )?,
            };

            if !should_execute {
                note_task_skip(
                    trace.as_deref_mut(),
                    &workflow.id,
                    &task.id,
                    pass.loop_counter,
                );
                i += 1;
                continue;
            }

            // Per-task snapshot buffer — only used for Map tasks in trace
            // mode, and only when the trace's policy wants them. Allocating an
            // empty Vec is cheap and the buffer stays empty for non-Map tasks.
            let mut mapping_snapshots: Vec<Value> = Vec::new();
            let want_mapping_contexts = trace
                .as_deref()
                .is_some_and(|t| t.options().mapping_contexts);
            let mapping_snapshots_buf = if want_mapping_contexts {
                Some(&mut mapping_snapshots)
            } else {
                None
            };

            // Clock reads only when a trace is live or an observer is attached,
            // so the plain path keeps its documented
            // one-`Utc::now()`-per-message invariant.
            let trace_start = if trace.is_some() {
                Some(Utc::now())
            } else {
                None
            };
            let obs_start = trace_start.or_else(|| self.observer_clock());

            // Sampled before the body runs: `validation` and
            // `TaskContext::add_error` both push during it, so the tail beyond
            // this index is exactly what this task contributed.
            let errors_before = message.errors.len();

            let result =
                self.execute_sync_task_in_arena(task, message, arena_ctx, mapping_snapshots_buf);

            // Before `handle_task_result`, whose `?` would drop failed tasks.
            self.emit_task_event(workflow, task, &result, obs_start);

            let flow = self.handle_task_result(
                result,
                &workflow.id_arc,
                &task.id_arc,
                TaskPass {
                    continue_on_error: task.continue_on_error,
                    terminal: task.terminal,
                    errors_before,
                },
                message,
                pass,
            );

            // Refresh the slots `handle_task_result` wrote so the next task —
            // and, in the cross-workflow path, the next workflow's condition —
            // sees them, without re-arenaing unrelated metadata children
            // (mapped `metadata.routing.*`, chained workflow state, …) after
            // every task.
            //
            // Deliberately *before* the `?`. An `Err` here does not necessarily
            // end this arena scope: `execute_sync_workflow_run` continues into
            // the next workflow carrying this same `ArenaContext` whenever the
            // failing task had `continue_on_error: false` but its workflow had
            // `continue_on_error: true`, and that workflow's condition would
            // otherwise be evaluated against a stale `metadata.progress`.
            arena_ctx.refresh_for_path(&message.context, "metadata.progress");
            // Gated on a failure actually being recorded: this walk deep-copies
            // the target subtree into the arena, so running it after every
            // successful task would be a permanent cost on the hot path.
            if let Some(cfg) = self.error_context_refresh(message, errors_before) {
                arena_ctx.refresh_for_path_parts(&message.context, &cfg.path_parts);
            }

            let control_flow = flow?;

            if let Some(t) = trace.as_deref_mut() {
                let started_at = trace_start.unwrap_or(pass.now);
                let mapping_contexts = if mapping_snapshots.is_empty() {
                    None
                } else {
                    Some(mapping_snapshots)
                };
                t.add_executed_step(
                    &workflow.id,
                    &task.id,
                    message,
                    StepTiming {
                        started_at,
                        duration_us: duration_us_between(started_at, Utc::now()),
                    },
                    mapping_contexts,
                    pass.loop_counter,
                );
            }

            if matches!(control_flow, TaskControlFlow::HaltWorkflow) {
                return Ok(SliceOutcome::Halted);
            }
            i += 1;
        }
        Ok(SliceOutcome::Completed)
    }

    /// Drive a message through `workflows` in order, grouping maximal runs of
    /// consecutive `fully_sync` workflows into a single shared-arena scope
    /// (`execute_sync_workflow_run`) and falling back to the per-workflow
    /// `.await` path (`execute_inner`) for any workflow containing an async
    /// task.
    ///
    /// A thin `&[&Workflow]` wrapper over [`Self::run_all_borrowed`], which is
    /// the actual shared entry all four `Engine::process_message*` variants
    /// call directly (against `&[Workflow]` from the engine's own registry,
    /// with no per-message `Vec<&Workflow>` collect). This method exists for
    /// a caller that already holds borrowed references.
    pub async fn run_all(
        &self,
        workflows: &[&Workflow],
        message: &mut Message,
        trace: Option<&mut ExecutionTrace>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.run_all_borrowed(workflows, message, trace, now).await
    }

    /// Generic driver behind [`Self::run_all`]: accepts any slice whose
    /// elements borrow as `Workflow` — `&[Workflow]` directly from the
    /// engine's registry (no per-message `Vec<&Workflow>` collect) or the
    /// `&[&Workflow]` shape the public entry keeps for compatibility.
    pub(crate) async fn run_all_borrowed<W: std::borrow::Borrow<Workflow>>(
        &self,
        workflows: &[W],
        message: &mut Message,
        mut trace: Option<&mut ExecutionTrace>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let mut i = 0;
        while i < workflows.len() {
            if joins_sync_run(workflows[i].borrow()) {
                // Extend over the maximal run of consecutive fully-sync
                // workflows and execute them in one shared arena scope.
                let mut j = i + 1;
                while j < workflows.len() && joins_sync_run(workflows[j].borrow()) {
                    j += 1;
                }
                self.execute_sync_workflow_run(
                    &workflows[i..j],
                    message,
                    trace.as_deref_mut(),
                    now,
                )?;
                i = j;
            } else {
                // Mixed sync+async (or fully-async) workflow: the existing
                // driver interleaves per-stretch arenas with `.await`.
                self.execute_inner(workflows[i].borrow(), message, trace.as_deref_mut(), now)
                    .await?;
                i += 1;
            }
        }
        Ok(())
    }

    /// Execute a maximal run of consecutive fully-sync workflows inside ONE
    /// shared `with_arena` scope. The message context is deep-walked into the
    /// arena once for the whole run, then carried — with the existing
    /// incremental `refresh_for_path` after each mutating task — across
    /// workflow boundaries, instead of being rebuilt per workflow.
    ///
    /// Per-workflow semantics are preserved exactly: each workflow's condition
    /// is evaluated (in-arena), a false condition skips only that workflow, a
    /// filter-halt stops only that workflow, and task errors are wrapped with
    /// the workflow id and honor `continue_on_error` (continue, or propagate
    /// `Err` out of the run to stop the whole message) — mirroring
    /// `execute_inner`.
    ///
    /// **Tokio safety:** this method is synchronous and the `fully_sync`
    /// precondition guarantees every task is a sync built-in, so no `.await`
    /// occurs while the `!Send` arena borrow is live. The borrow checker
    /// enforces this — the shared `ArenaContext` cannot escape the closure.
    fn execute_sync_workflow_run<W: std::borrow::Borrow<Workflow>>(
        &self,
        workflows: &[W],
        message: &mut Message,
        mut trace: Option<&mut ExecutionTrace>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        // `joins_sync_run` keeps looping workflows out of this path, so every
        // workflow here runs exactly one pass and carries no loop counter.
        debug_assert!(
            workflows.iter().all(|w| joins_sync_run(w.borrow())),
            "only non-looping fully-sync workflows may join a shared-arena run"
        );
        let pass = PassCtx::once(now);

        with_arena(|arena| -> Result<()> {
            let mut arena_ctx = ArenaContext::from_owned(&message.context, arena);

            for workflow in workflows {
                let workflow: &Workflow = workflow.borrow();

                // Same gate as `execute_inner`. This is the site a fully-sync
                // workflow actually reaches: `fully_sync` routes every
                // map/log/validation/filter-only workflow here and never through
                // `execute_inner`, so gating only there would silently not apply
                // to most workflows.
                if !rollout_admits(workflow, message) {
                    note_workflow_skip(
                        trace.as_deref_mut(),
                        &workflow.id,
                        "outside rollout bucket",
                    );
                    continue;
                }

                // Workflow condition in-arena: a folded `None` skips the eval;
                // a real condition reuses the carried context instead of the
                // owned-path `eval_to_owned` deep-walk.
                let should_execute = match workflow.compiled_condition.as_ref() {
                    None => true,
                    Some(compiled) => evaluate_condition_in_arena(
                        &self.engine,
                        Some(compiled),
                        arena_ctx.as_data_value(),
                        arena,
                    )?,
                };

                if !should_execute {
                    note_workflow_skip(trace.as_deref_mut(), &workflow.id, "condition not met");
                    continue;
                }

                // Group state is per-workflow: this run carries one arena
                // across several workflows, but never a group.
                let mut gate = GroupGate::default();
                match self.run_tasks_slice_in_arena(
                    TaskSlice {
                        tasks: &workflow.tasks,
                        offset: 0,
                        gate: &mut gate,
                    },
                    workflow,
                    message,
                    &mut arena_ctx,
                    trace.as_deref_mut(),
                    pass,
                ) {
                    // A halt stops only this workflow; carry on with the next
                    // one (and keep the shared arena context). The slice spans
                    // the whole task list, so a jump can only land at its end.
                    Ok(_outcome) => {
                        info!("Successfully completed workflow: {}", workflow.id);
                    }
                    Err(e) => {
                        // Single-channel contract — mirror `execute_inner`.
                        if self.record_workflow_error(workflow, message, &e) {
                            return Err(e);
                        }
                    }
                }
            }
            Ok(())
        })
    }

    /// Dispatch a single sync-builtin task via the consolidated
    /// `FunctionConfig::try_execute_in_arena`. `next_async_boundary` guarantees
    /// the stretch contents are sync built-ins, so the `None` arm is
    /// unreachable in practice.
    ///
    /// `mapping_snapshots` is only consulted by the `Map` variant; non-Map
    /// sync builtins ignore it. Pass `None` from the production path.
    fn execute_sync_task_in_arena<'arena>(
        &self,
        task: &'arena Task,
        message: &mut Message,
        arena_ctx: &mut ArenaContext<'arena>,
        mapping_snapshots: Option<&mut Vec<Value>>,
    ) -> Result<(TaskOutcome, Vec<Change>)> {
        debug!(
            "Executing sync task in arena: {} ({})",
            task.id,
            task.function.function_name()
        );
        debug_assert!(
            task.function.is_sync_builtin(),
            "execute_sync_task_in_arena called with non-sync-builtin task: {}",
            task.function.function_name()
        );
        // In debug builds the assert above catches mis-dispatch; in release
        // we still surface the invariant violation as a recoverable engine
        // error rather than panicking via `unreachable!`.
        task.function
            .try_execute_in_arena(message, arena_ctx, &self.engine, mapping_snapshots)
            .ok_or_else(|| {
                DataflowError::Task(format!(
                    "execute_sync_task_in_arena dispatched to non-sync-builtin task '{}' \
                     (engine bug — sync-stretch should only contain sync-builtin tasks)",
                    task.function.function_name()
                ))
            })?
    }

    /// Mirror every error this task contributed to `message.errors` into the
    /// configured context path.
    ///
    /// Taking the delta beyond `task.errors_before` rather than recording at each
    /// push site is what makes coverage match `errors()` exactly. Two of the four
    /// per-task producers never reach a failure arm at all: the `validation`
    /// built-in appends its per-rule failures and then returns `Status(400)`,
    /// which lands in the *success* arm, and `TaskContext::add_error` can fire on
    /// a task that succeeds outright. Neither is visible to a host that wraps
    /// handlers either, since the sync built-ins never reach the registry.
    ///
    /// Must run *after* the two pushes `handle_task_result` performs itself
    /// (`TASK_STATUS_ERROR` and the task error), or they fall outside the delta —
    /// which would silently drop every `map` failure, since `map` returns
    /// `Status(500)` without touching `errors` itself.
    ///
    /// Returns nothing — the sync stretch reads [`Self::error_context_refresh`]
    /// instead, so the arena refresh stays off the no-failure path.
    #[inline]
    fn mirror_task_errors(
        &self,
        message: &mut Message,
        workflow_id: &str,
        task_id: &str,
        status: u16,
        task: TaskPass,
    ) {
        let Some(cfg) = self.error_context.as_ref() else {
            return;
        };
        // Disjoint borrows of two fields of `Message`, so the split is needed to
        // read the error tail while mutating the context.
        let Message {
            context, errors, ..
        } = message;
        let new_errors = errors.get(task.errors_before..).unwrap_or(&[]);
        append_error_records(context, cfg, workflow_id, task_id, status, new_errors);
    }

    /// Whether the sync stretch must refresh the arena for the error-context
    /// path after this task — i.e. the option is on and the task contributed at
    /// least one error.
    #[inline]
    fn error_context_refresh<'a>(
        &'a self,
        message: &Message,
        errors_before: usize,
    ) -> Option<&'a Arc<ErrorContextConfig>> {
        let cfg = self.error_context.as_ref()?;
        if message.errors.len() > errors_before {
            Some(cfg)
        } else {
            None
        }
    }

    /// Handle the result of a task execution.
    ///
    /// `workflow_id_arc` and `task_id_arc` are the compile-time cached
    /// `Arc<str>` mirrors of `workflow.id` / `task.id`; we Arc-clone them into
    /// each `AuditTrail` rather than reallocating from the `&str` form.
    fn handle_task_result(
        &self,
        result: Result<(TaskOutcome, Vec<Change>)>,
        workflow_id_arc: &Arc<str>,
        task_id_arc: &Arc<str>,
        task: TaskPass,
        message: &mut Message,
        pass: PassCtx,
    ) -> Result<TaskControlFlow> {
        let workflow_id: &str = workflow_id_arc;
        let task_id: &str = task_id_arc;
        let continue_on_error = task.continue_on_error;
        match result {
            Ok((TaskOutcome::Skip, _)) => {
                // No audit trail, no progress write, and no error-context record
                // — the task has explicitly opted out of the per-task record
                // (filter gate set to `Skip`). `audit_status()` is `None` here,
                // so a record would need a fabricated `status`, breaking the
                // fixed four-key shape that makes the path predictable to branch
                // on. Reaching this with errors recorded takes a handler that
                // calls `add_error` and *then* skips; the entry is still on
                // `message.errors()`.
                debug!("Task {} signaled skip", task_id);
                Ok(TaskControlFlow::Continue)
            }
            Ok((outcome, changes)) => {
                // `Skip` already returned above; the remaining variants all
                // record an audit entry. `audit_status()` is `Some` for
                // Success/Status/Halt — expect is for documentation only.
                let status = outcome
                    .audit_status()
                    .expect("Skip handled above; remaining variants emit audit status");
                // `Task::terminal` reaches the same halt as `TaskOutcome::Halt`,
                // but it is applied *after* the status classification below —
                // see the `flow` fold. Deciding here would make halting the
                // first branch of the chain, so a terminal task returning 500
                // would stop without recording `TASK_STATUS_ERROR` and without
                // propagating when `continue_on_error` is false.
                let halt_requested = outcome.halts_workflow() || task.terminal;

                // Record audit trail. workflow_id_arc/task_id_arc are populated
                // by LogicCompiler at engine construction; cloning them is a
                // refcount bump, not a string copy. `now` is shared with all
                // other AuditTrails in this process_message call.
                message.audit_trail.push(AuditTrail {
                    timestamp: pass.now,
                    workflow_id: Arc::clone(workflow_id_arc),
                    task_id: Arc::clone(task_id_arc),
                    status: status as usize,
                    changes,
                    loop_counter: pass.loop_counter,
                });

                // Update progress metadata for workflow chaining. Always
                // emitted: when multiple workflows are registered in the same
                // engine, downstream workflows route on
                // `metadata.progress.{workflow_id,task_id,status_code}` to
                // advance through linear sequences. After the first task the
                // slot already holds the expected 3-key object, so the write
                // overwrites the three values in place — only the two id
                // `String` allocs remain. (This beat both three separate
                // `set_nested_value` calls and the batched slot replace on
                // the realistic workload.)
                write_progress_metadata(&mut message.context, workflow_id, task_id, status);

                // Decide the control flow first rather than returning from
                // inside each branch, so the error-context mirror below runs on
                // exactly one path. The halt and `!continue_on_error` exits would
                // otherwise each need their own call, and a future exit added
                // without one would silently stop recording.
                let flow = if (400..500).contains(&status) {
                    warn!("Task {} returned client error status: {}", task_id, status);
                    Ok(TaskControlFlow::Continue)
                } else if status >= 500 {
                    error!("Task {} returned server error status: {}", task_id, status);
                    // Single-channel contract: surface 5xx outcomes through
                    // `message.errors` as well as the audit trail, so callers
                    // that scan `errors()` see a 5xx-status task even when
                    // the workflow continues past it.
                    message.errors.push(
                        ErrorInfo::builder(
                            "TASK_STATUS_ERROR",
                            format!("Task {} returned status {}", task_id, status),
                        )
                        .workflow_id(workflow_id)
                        .task_id(task_id)
                        .build(),
                    );
                    if continue_on_error {
                        Ok(TaskControlFlow::Continue)
                    } else {
                        Err(DataflowError::Task(format!(
                            "Task {} failed with status {}",
                            task_id, status
                        )))
                    }
                } else {
                    Ok(TaskControlFlow::Continue)
                };

                // Upgrade a `Continue` to a halt, leaving the 5xx `Err` and the
                // recording above untouched. `TaskOutcome::Halt`'s own status is
                // 299 — neither 4xx nor 5xx — so its behaviour is unchanged.
                let flow = match flow {
                    Ok(TaskControlFlow::Continue) if halt_requested => {
                        info!("Task {} halted workflow {}", task_id, workflow_id);
                        Ok(TaskControlFlow::HaltWorkflow)
                    }
                    other => other,
                };

                // After the `TASK_STATUS_ERROR` push above, so it lands inside
                // this task's delta.
                self.mirror_task_errors(message, workflow_id, task_id, status, task);
                flow
            }
            Err(e) => {
                error!("Task {} failed: {:?}", task_id, e);

                // Record error in audit trail (Arc clones are refcount bumps).
                message.audit_trail.push(AuditTrail {
                    timestamp: pass.now,
                    workflow_id: Arc::clone(workflow_id_arc),
                    task_id: Arc::clone(task_id_arc),
                    status: 500,
                    changes: vec![],
                    loop_counter: pass.loop_counter,
                });

                // Same invariant as the Ok arm: `metadata.progress` is written
                // after every task, unconditionally, so a downstream workflow
                // gating on it still sees this task ran even though it errored.
                write_progress_metadata(&mut message.context, workflow_id, task_id, 500);

                // Add error to message. A service-classified error contributes
                // its own `kind` as the code and carries its operator-only
                // `detail`; everything else takes its variant's code.
                // Deliberately lifted at the task site only: the two
                // `WORKFLOW_ERROR` wrappers wrap the same propagated error, so
                // lifting there too would put two entries with the same
                // `code` on the message — making "count errors by code"
                // double-count — and would stop `WORKFLOW_ERROR` reliably
                // meaning "a workflow stopped".
                //
                // `format!("{}", e)` stays caller-safe because `Service`'s
                // `Display` is `{message}` — the detail is never interpolated.
                let mut info = ErrorInfo::builder(
                    service_error_code(&e),
                    format!("Task {} error: {}", task_id, e),
                )
                .workflow_id(workflow_id)
                .task_id(task_id);
                // Nested `if let`, not a let-chain: MSRV is 1.85.
                if let Some(detail) = e.detail() {
                    info = info.detail(detail);
                }
                message.errors.push(info.build());

                // `500` matches the audit entry and the progress write above: a
                // handler `Err` has no status of its own.
                self.mirror_task_errors(message, workflow_id, task_id, 500, task);

                if !continue_on_error {
                    Err(e)
                } else if task.terminal {
                    // `terminal` is about position, not outcome: the author said
                    // "nothing after this runs". The error stays on
                    // `message.errors()` either way.
                    info!(
                        "Terminal task {} halted workflow {} after failing",
                        task_id, workflow_id
                    );
                    Ok(TaskControlFlow::HaltWorkflow)
                } else {
                    Ok(TaskControlFlow::Continue)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::compiler::LogicCompiler;
    use serde_json::json;
    use std::collections::HashMap;

    /// Test-only helper: build an `OwnedDataValue` from a `json!` literal.
    fn dv(v: serde_json::Value) -> OwnedDataValue {
        OwnedDataValue::from(&v)
    }

    /// Compile `json` into a single runnable workflow plus its engine.
    fn compiled(json: &str) -> (Workflow, Arc<datalogic_rs::Engine>) {
        let compiler = LogicCompiler::new();
        let workflow = Workflow::from_json(json).expect("workflow should parse");
        let compiled = compiler
            .compile_workflows(vec![workflow])
            .expect("workflow should compile");
        (
            compiled.into_iter().next().expect("one workflow"),
            compiler.into_engine(),
        )
    }

    /// A `WorkflowExecutor` over an empty handler registry.
    fn executor(engine: Arc<datalogic_rs::Engine>) -> WorkflowExecutor {
        let task_executor = Arc::new(TaskExecutor::new(
            Arc::new(HashMap::new()),
            Arc::clone(&engine),
        ));
        WorkflowExecutor::new(task_executor, engine)
    }

    /// A `WorkflowExecutor` that mirrors failure codes to `metadata.errors`.
    fn executor_with_error_context(engine: Arc<datalogic_rs::Engine>) -> WorkflowExecutor {
        let cfg = ErrorContextConfig::new("metadata.errors".to_string(), 32)
            .expect("metadata.errors is a valid path");
        executor(engine).with_error_context(Arc::new(cfg))
    }

    #[tokio::test]
    async fn appending_records_mid_stretch_keeps_the_arena_cache_consistent() {
        // A failing `validation` followed by a `map`, both sync built-ins, so
        // they share one `ArenaContext`. Two things are under test:
        //
        // 1. the `map` reads the record appended by the `validation`, which only
        //    works if the append refreshed the arena; and
        // 2. `apply_mutation_parts_write_through`'s `#[cfg(test)]`
        //    `assert_matches_owned` runs on the `map`'s write, giving free
        //    differential verification that the refresh left the arena cache
        //    identical to a from-scratch rebuild of the owned context. That
        //    assertion is compiled out for the `tests/` binaries, so it can only
        //    be exercised from here.
        let (workflow, engine) = compiled(
            r#"{ "id": "w", "name": "w", "tasks": [
                { "id": "check", "name": "check", "continue_on_error": true,
                  "function": {"name": "validation", "input": {"rules": [
                      {"logic": false, "message": "nope"}]}}},
                { "id": "react", "name": "react",
                  "function": {"name": "map", "input": {"mappings": [
                      {"path": "data.seen", "logic": {"var": "metadata.errors.0.code"}}]}}}
            ]}"#,
        );
        let mut message = Message::from_value(&json!({}));

        executor_with_error_context(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("continue_on_error keeps the workflow running");

        assert_eq!(
            message.context["data"].get("seen"),
            Some(&dv(json!("VALIDATION_ERROR"))),
            "the map must read the record the validation appended in the same stretch"
        );
    }

    #[tokio::test]
    async fn the_error_context_path_is_untouched_when_every_task_succeeds() {
        let (workflow, engine) = compiled(&format!(
            r#"{{ "id": "w", "name": "w", "tasks": [{COUNTER_BODY}] }}"#
        ));
        let mut message = Message::from_value(&json!({}));

        executor_with_error_context(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("workflow should complete");

        assert_eq!(
            message.context["metadata"].get("errors"),
            None,
            "a clean run leaves the key absent, not an empty array"
        );
    }

    /// Every `loop_counter` recorded on the audit trail, in order.
    fn counters(message: &Message) -> Vec<Option<i64>> {
        message
            .audit_trail
            .iter()
            .map(|entry| entry.loop_counter)
            .collect()
    }

    /// A one-task `map` workflow body writing `data.n` from the counter.
    const COUNTER_BODY: &str = r#"{"id": "t", "name": "t", "function": {"name": "map",
        "input": {"mappings": [{"path": "data.n", "logic": {"var": "temp_data.i"}}]}}}"#;

    #[tokio::test]
    async fn loop_without_a_condition_runs_exactly_max_sweeps() {
        let (workflow, engine) = compiled(&format!(
            r#"{{ "id": "w", "name": "w", "loop": {{"counter": "i", "max": 3}},
                  "tasks": [{COUNTER_BODY}] }}"#
        ));
        let mut message = Message::from_value(&json!({}));

        let executed = executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("loop should complete");

        assert!(executed);
        // One audit entry per sweep, each stamped with its counter.
        assert_eq!(counters(&message), vec![Some(0), Some(1), Some(2)]);
        // The counter is left at the bound the loop stopped on.
        assert_eq!(message.context["temp_data"].get("i"), Some(&dv(json!(3))));
        // The body observed each value; the last one survives.
        assert_eq!(message.context["data"].get("n"), Some(&dv(json!(2))));
    }

    #[tokio::test]
    async fn loop_exits_early_when_the_condition_goes_false() {
        // Bounded at 10 but the condition stops it at 4.
        let (workflow, engine) = compiled(&format!(
            r#"{{ "id": "w", "name": "w",
                  "condition": {{"<": [{{"var": "temp_data.i"}}, 4]}},
                  "loop": {{"counter": "i", "max": 10}},
                  "tasks": [{COUNTER_BODY}] }}"#
        ));
        let mut message = Message::from_value(&json!({}));

        executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("loop should complete");

        assert_eq!(counters(&message), vec![Some(0), Some(1), Some(2), Some(3)]);
    }

    #[tokio::test]
    async fn loop_whose_condition_is_false_on_the_first_sweep_is_a_plain_skip() {
        let (workflow, engine) = compiled(&format!(
            r#"{{ "id": "w", "name": "w", "condition": false,
                  "loop": {{"counter": "i", "max": 5}},
                  "tasks": [{COUNTER_BODY}] }}"#
        ));
        let mut message = Message::from_value(&json!({}));

        let executed = executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("a skip is not an error");

        assert!(!executed, "a never-entered loop reports as skipped");
        assert!(message.audit_trail.is_empty());
    }

    #[tokio::test]
    async fn filter_halt_breaks_the_whole_loop_not_just_one_sweep() {
        let (workflow, engine) = compiled(
            r#"{ "id": "w", "name": "w", "loop": {"counter": "i", "max": 10},
                 "tasks": [
                   {"id": "gate", "name": "gate", "function": {"name": "filter",
                     "input": {"condition": {"<": [{"var": "temp_data.i"}, 2]},
                               "on_reject": "halt"}}},
                   {"id": "body", "name": "body", "function": {"name": "map",
                     "input": {"mappings": [
                        {"path": "data.n", "logic": {"var": "temp_data.i"}}]}}}] }"#,
        );
        let mut message = Message::from_value(&json!({}));

        executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("a halt is not an error");

        // Sweeps 0 and 1 run both tasks; sweep 2's gate halts and ends the
        // loop rather than moving on to sweep 3.
        let ids: Vec<&str> = message
            .audit_trail
            .iter()
            .map(|entry| entry.task_id.as_ref())
            .collect();
        assert_eq!(ids, ["gate", "body", "gate", "body", "gate"]);
        assert_eq!(
            counters(&message),
            vec![Some(0), Some(0), Some(1), Some(1), Some(2)]
        );
    }

    #[tokio::test]
    async fn init_and_increment_drive_the_counter() {
        let (workflow, engine) = compiled(&format!(
            r#"{{ "id": "w", "name": "w",
                  "loop": {{"counter": "i", "init": 10, "increment": 5, "max": 25}},
                  "tasks": [{COUNTER_BODY}] }}"#
        ));
        let mut message = Message::from_value(&json!({}));

        executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("loop should complete");

        assert_eq!(counters(&message), vec![Some(10), Some(15), Some(20)]);
    }

    #[tokio::test]
    async fn a_loop_without_a_named_counter_still_records_it_on_the_audit_trail() {
        let (workflow, engine) = compiled(
            r#"{ "id": "w", "name": "w", "loop": {"max": 2},
                 "tasks": [{"id": "t", "name": "t",
                            "function": {"name": "map", "input": {"mappings": []}}}] }"#,
        );
        let mut message = Message::from_value(&json!({}));

        executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("loop should complete");

        assert_eq!(counters(&message), vec![Some(0), Some(1)]);
        // Nothing was written to temp_data — the counter was never named.
        assert_eq!(message.context["temp_data"], dv(json!({})));
    }

    #[tokio::test]
    async fn a_non_looping_workflow_records_no_loop_counter() {
        let (workflow, engine) = compiled(
            r#"{ "id": "w", "name": "w",
                 "tasks": [{"id": "t", "name": "t",
                            "function": {"name": "map", "input": {"mappings": []}}}] }"#,
        );
        let mut message = Message::from_value(&json!({}));

        executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("should complete");

        assert_eq!(counters(&message), vec![None]);
    }

    #[tokio::test]
    async fn progress_metadata_is_written_on_every_sweep() {
        // `metadata.progress` is load-bearing for cross-workflow chaining; a
        // loop must not gate it.
        let (workflow, engine) = compiled(&format!(
            r#"{{ "id": "w", "name": "w", "loop": {{"counter": "i", "max": 3}},
                  "tasks": [{COUNTER_BODY}] }}"#
        ));
        let mut message = Message::from_value(&json!({}));

        executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("loop should complete");

        let progress = message.context["metadata"]
            .get("progress")
            .expect("progress must be written");
        assert_eq!(progress.get("workflow_id"), Some(&dv(json!("w"))));
        assert_eq!(progress.get("task_id"), Some(&dv(json!("t"))));
        assert_eq!(progress.get("status_code"), Some(&dv(json!(200))));
    }

    #[tokio::test]
    async fn the_engine_owns_the_counter_even_if_a_body_task_writes_it() {
        // A body task writing the counter path is overwritten at the next
        // increment, so termination reasoning stays local to LoopConfig.
        let (workflow, engine) = compiled(
            r#"{ "id": "w", "name": "w", "loop": {"counter": "i", "max": 3},
                 "tasks": [{"id": "t", "name": "t", "function": {"name": "map",
                    "input": {"mappings": [{"path": "temp_data.i", "logic": 99}]}}}] }"#,
        );
        let mut message = Message::from_value(&json!({}));

        executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("loop should complete");

        assert_eq!(
            counters(&message),
            vec![Some(0), Some(1), Some(2)],
            "the body's write must not stall or skew the loop"
        );
    }

    /// Run a bare counting loop with the given bounds and return the counter
    /// values the sweeps actually recorded.
    async fn counter_sequence(init: i64, increment: i64, max: i64) -> Vec<Option<i64>> {
        let (workflow, engine) = compiled(&format!(
            r#"{{ "id": "w", "name": "w",
                  "loop": {{"counter": "i", "init": {init},
                            "increment": {increment}, "max": {max}}},
                  "tasks": [{{"id": "t", "name": "t",
                              "function": {{"name": "map", "input": {{"mappings": []}}}}}}] }}"#
        ));
        let mut message = Message::from_value(&json!({}));
        executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("loop should complete");
        counters(&message)
    }

    #[tokio::test]
    async fn counter_sequence_matrix_over_init_increment_and_max() {
        // The half-open `counter < max` bound, swept across signs and step
        // sizes. Each expected list is the exact sequence of sweeps.
        let cases: Vec<(i64, i64, i64, Vec<i64>)> = vec![
            // Defaults: 0-based, step 1 — the array-index case.
            (0, 1, 1, vec![0]),
            (0, 1, 2, vec![0, 1]),
            (0, 1, 5, vec![0, 1, 2, 3, 4]),
            // Non-unit steps, including a range the step does not divide.
            (0, 2, 6, vec![0, 2, 4]),
            (0, 3, 10, vec![0, 3, 6, 9]),
            (0, 5, 3, vec![0]),
            (0, 100, 1, vec![0]),
            // Non-zero starts.
            (10, 5, 25, vec![10, 15, 20]),
            (3, 1, 6, vec![3, 4, 5]),
            // Negative and mixed-sign ranges.
            (-3, 1, 2, vec![-3, -2, -1, 0, 1]),
            (-4, 2, 1, vec![-4, -2, 0]),
            (-10, 5, -5, vec![-10]),
        ];

        for (init, increment, max, expected) in cases {
            let got = counter_sequence(init, increment, max).await;
            let expected: Vec<Option<i64>> = expected.into_iter().map(Some).collect();
            assert_eq!(got, expected, "init={init} increment={increment} max={max}");
        }
    }

    #[tokio::test]
    async fn the_counter_advance_saturates_instead_of_overflowing() {
        // A huge increment must end the loop, not wrap into a negative counter
        // and spin. Both the giant-step and the near-i64::MAX start are
        // exercised, since either could overflow a plain `+`.
        assert_eq!(
            counter_sequence(0, i64::MAX, 5).await,
            vec![Some(0)],
            "one sweep, then the advance saturates past max"
        );
        assert_eq!(
            counter_sequence(i64::MAX - 1, 1, i64::MAX).await,
            vec![Some(i64::MAX - 1)],
            "the last representable sweep still terminates"
        );
        assert_eq!(
            counter_sequence(i64::MAX - 2, i64::MAX, i64::MAX).await,
            vec![Some(i64::MAX - 2)]
        );
    }

    #[tokio::test]
    async fn a_task_condition_is_re_evaluated_against_the_counter_every_sweep() {
        // Per-sweep task conditions are the mechanism for "do this only on
        // some iterations"; a stale condition cache would break it.
        let (workflow, engine) = compiled(
            r#"{ "id": "w", "name": "w", "loop": {"counter": "i", "max": 4},
                 "tasks": [
                   {"id": "evens", "name": "evens",
                    "condition": {"==": [{"%": [{"var": "temp_data.i"}, 2]}, 0]},
                    "function": {"name": "map", "input": {"mappings": []}}},
                   {"id": "always", "name": "always",
                    "function": {"name": "map", "input": {"mappings": []}}}] }"#,
        );
        let mut message = Message::from_value(&json!({}));

        executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("loop should complete");

        let entries: Vec<(&str, Option<i64>)> = message
            .audit_trail
            .iter()
            .map(|e| (e.task_id.as_ref(), e.loop_counter))
            .collect();
        assert_eq!(
            entries,
            [
                ("evens", Some(0)),
                ("always", Some(0)),
                ("always", Some(1)),
                ("evens", Some(2)),
                ("always", Some(2)),
                ("always", Some(3)),
            ],
            "the gated task runs only on even counters"
        );
    }

    #[tokio::test]
    async fn a_filter_skip_does_not_keep_the_loop_alive_or_record_entries() {
        // `TaskOutcome::Skip` records no audit entry and no progress write.
        // The loop is driven by its bound, not by whether tasks recorded
        // anything, so it still runs exactly `max` sweeps.
        let (workflow, engine) = compiled(
            r#"{ "id": "w", "name": "w", "loop": {"counter": "i", "max": 3},
                 "tasks": [{"id": "gate", "name": "gate", "function": {"name": "filter",
                    "input": {"condition": false, "on_reject": "skip"}}}] }"#,
        );
        let mut message = Message::from_value(&json!({}));

        let executed = executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("skip is not an error");

        assert!(executed, "sweeps ran even though every task skipped");
        assert!(message.audit_trail.is_empty(), "Skip records no entry");
        assert_eq!(
            message.context["temp_data"].get("i"),
            Some(&dv(json!(3))),
            "the loop still ran to its bound"
        );
    }

    #[tokio::test]
    async fn a_4xx_task_status_is_recorded_per_sweep_without_stopping_the_loop() {
        // A failing `validation` yields 400: warned, recorded, loop continues.
        let (workflow, engine) = compiled(
            r#"{ "id": "w", "name": "w", "loop": {"counter": "i", "max": 3},
                 "tasks": [{"id": "check", "name": "check", "function": {"name": "validation",
                    "input": {"rules": [{"logic": {"==": [1, 2]}, "message": "nope"}]}}}] }"#,
        );
        let mut message = Message::from_value(&json!({}));

        executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("a 4xx does not stop the workflow");

        assert_eq!(counters(&message), vec![Some(0), Some(1), Some(2)]);
        assert!(
            message.audit_trail.iter().all(|e| e.status == 400),
            "every sweep recorded the 4xx"
        );
    }

    #[tokio::test]
    async fn the_rollout_gate_excludes_a_looping_workflow_before_any_sweep() {
        // The gate runs ahead of the loop, so an excluded workflow writes no
        // counter at all — it must be indistinguishable from a plain skip.
        let (workflow, engine) = compiled(
            r#"{ "id": "w", "name": "w",
                 "rollout": {"bucket_start": 0, "bucket_end": 50},
                 "loop": {"counter": "i", "max": 5},
                 "tasks": [{"id": "t", "name": "t",
                            "function": {"name": "map", "input": {"mappings": []}}}] }"#,
        );
        let mut message = Message::builder().routing_bucket(75).build();

        let executed = executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("an excluded workflow is not an error");

        assert!(!executed);
        assert!(message.audit_trail.is_empty());
        assert_eq!(
            message.context["temp_data"].get("i"),
            None,
            "no counter is written for an excluded workflow"
        );
    }

    #[tokio::test]
    async fn a_nested_counter_path_is_created_and_advanced() {
        let (workflow, engine) = compiled(
            r#"{ "id": "w", "name": "w",
                 "loop": {"counter": "cursor.index", "max": 3},
                 "tasks": [{"id": "t", "name": "t", "function": {"name": "map",
                    "input": {"mappings": [
                       {"path": "data.seen", "logic": {"var": "temp_data.cursor.index"}}]}}}] }"#,
        );
        let mut message = Message::from_value(&json!({}));

        executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("loop should complete");

        assert_eq!(
            message.context["temp_data"]["cursor"].get("index"),
            Some(&dv(json!(3)))
        );
        assert_eq!(
            message.context["data"].get("seen"),
            Some(&dv(json!(2))),
            "the body read the nested counter"
        );
    }

    #[tokio::test]
    async fn writing_the_counter_preserves_unrelated_temp_data() {
        let (workflow, engine) = compiled(
            r#"{ "id": "w", "name": "w", "loop": {"counter": "i", "max": 2},
                 "tasks": [{"id": "t", "name": "t",
                            "function": {"name": "map", "input": {"mappings": []}}}] }"#,
        );
        let mut message = Message::builder()
            .temp_data(dv(json!({"keep": "me", "nested": {"a": 1}})))
            .build();

        executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("loop should complete");

        assert_eq!(
            message.context["temp_data"].get("keep"),
            Some(&dv(json!("me")))
        );
        assert_eq!(
            message.context["temp_data"]["nested"].get("a"),
            Some(&dv(json!(1)))
        );
        assert_eq!(message.context["temp_data"].get("i"), Some(&dv(json!(2))));
    }

    #[tokio::test]
    async fn the_counter_overwrites_a_pre_existing_value_at_that_path() {
        // The engine owns the path: whatever was there before the loop is
        // replaced by `init` on the first sweep.
        let (workflow, engine) = compiled(
            r#"{ "id": "w", "name": "w", "loop": {"counter": "i", "init": 5, "max": 7},
                 "tasks": [{"id": "t", "name": "t",
                            "function": {"name": "map", "input": {"mappings": []}}}] }"#,
        );
        let mut message = Message::builder()
            .temp_data(dv(json!({"i": "not a number"})))
            .build();

        executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("loop should complete");

        assert_eq!(counters(&message), vec![Some(5), Some(6)]);
        assert_eq!(message.context["temp_data"].get("i"), Some(&dv(json!(7))));
    }

    #[tokio::test]
    async fn a_loop_records_audit_entries_with_capture_changes_off() {
        // `capture_changes(false)` suppresses the per-change diff, not the
        // audit entries themselves — so the loop counter is still recorded.
        let (workflow, engine) = compiled(
            r#"{ "id": "w", "name": "w", "loop": {"counter": "i", "max": 2},
                 "tasks": [{"id": "t", "name": "t", "function": {"name": "map",
                    "input": {"mappings": [
                       {"path": "data.n", "logic": {"var": "temp_data.i"}}]}}}] }"#,
        );
        let mut message = Message::builder().capture_changes(false).build();

        executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("loop should complete");

        assert_eq!(counters(&message), vec![Some(0), Some(1)]);
        assert!(
            message.audit_trail.iter().all(|e| e.changes.is_empty()),
            "no diffs captured, but the entries are still there"
        );
    }

    #[tokio::test]
    async fn two_loops_sharing_a_counter_name_do_not_interfere() {
        // Each loop re-initialises the path it owns, so the second starts from
        // its own `init` rather than inheriting where the first stopped.
        let first = r#"{ "id": "a", "name": "a", "priority": 0,
             "loop": {"counter": "i", "max": 2},
             "tasks": [{"id": "t", "name": "t",
                        "function": {"name": "map", "input": {"mappings": []}}}] }"#;
        let second = r#"{ "id": "b", "name": "b", "priority": 1,
             "loop": {"counter": "i", "init": 10, "max": 12},
             "tasks": [{"id": "t", "name": "t",
                        "function": {"name": "map", "input": {"mappings": []}}}] }"#;

        let compiler = LogicCompiler::new();
        let workflows = compiler
            .compile_workflows(vec![
                Workflow::from_json(first).unwrap(),
                Workflow::from_json(second).unwrap(),
            ])
            .expect("should compile");
        let exec = executor(compiler.into_engine());
        let mut message = Message::from_value(&json!({}));

        exec.run_all_borrowed(&workflows, &mut message, None, Utc::now())
            .await
            .expect("both loops should complete");

        let per_workflow: Vec<(&str, Option<i64>)> = message
            .audit_trail
            .iter()
            .map(|e| (e.workflow_id.as_ref(), e.loop_counter))
            .collect();
        assert_eq!(
            per_workflow,
            [
                ("a", Some(0)),
                ("a", Some(1)),
                ("b", Some(10)),
                ("b", Some(11)),
            ]
        );
    }

    #[tokio::test]
    async fn a_looping_workflow_between_sync_workflows_does_not_break_the_sync_run() {
        // Regression guard for the `joins_sync_run` change: a loop workflow is
        // excluded from the shared-arena run, which must split the run around
        // it rather than dropping its neighbours.
        let sync_wf = |id: &str, priority: u32| {
            format!(
                r#"{{ "id": "{id}", "name": "{id}", "priority": {priority},
                      "tasks": [{{"id": "t", "name": "t", "function": {{"name": "map",
                        "input": {{"mappings": [
                          {{"path": "data.{id}", "logic": true}}]}}}}}}] }}"#
            )
        };
        let loop_wf = r#"{ "id": "mid", "name": "mid", "priority": 1,
             "loop": {"counter": "i", "max": 2},
             "tasks": [{"id": "t", "name": "t", "function": {"name": "map",
                "input": {"mappings": [{"path": "data.mid", "logic": true}]}}}] }"#;

        let compiler = LogicCompiler::new();
        let workflows = compiler
            .compile_workflows(vec![
                Workflow::from_json(&sync_wf("before", 0)).unwrap(),
                Workflow::from_json(loop_wf).unwrap(),
                Workflow::from_json(&sync_wf("after", 2)).unwrap(),
            ])
            .expect("should compile");
        // All three are sync-only, but the loop must not join a shared run.
        assert!(workflows.iter().all(|w| w.fully_sync));
        assert!(!joins_sync_run(&workflows[1]));

        let exec = executor(compiler.into_engine());
        let mut message = Message::from_value(&json!({}));

        exec.run_all_borrowed(&workflows, &mut message, None, Utc::now())
            .await
            .expect("all three should run");

        for id in ["before", "mid", "after"] {
            assert_eq!(
                message.context["data"].get(id),
                Some(&dv(json!(true))),
                "workflow {id} must have run"
            );
        }
        let order: Vec<(&str, Option<i64>)> = message
            .audit_trail
            .iter()
            .map(|e| (e.workflow_id.as_ref(), e.loop_counter))
            .collect();
        assert_eq!(
            order,
            [
                ("before", None),
                ("mid", Some(0)),
                ("mid", Some(1)),
                ("after", None),
            ],
            "priority order is preserved across the split"
        );
    }

    #[tokio::test]
    async fn consecutive_non_looping_sync_workflows_still_share_one_run() {
        // The other half of the same regression: without a loop in the way,
        // every fully-sync workflow still groups as it always did.
        let compiler = LogicCompiler::new();
        let workflows = compiler
            .compile_workflows(vec![
                Workflow::from_json(
                    r#"{ "id": "a", "name": "a", "priority": 0, "tasks": [{"id": "t", "name": "t",
                         "function": {"name": "map", "input": {"mappings": [
                           {"path": "data.a", "logic": 1}]}}}] }"#,
                )
                .unwrap(),
                Workflow::from_json(
                    r#"{ "id": "b", "name": "b", "priority": 1,
                         "condition": {"==": [{"var": "data.a"}, 1]},
                         "tasks": [{"id": "t", "name": "t",
                         "function": {"name": "map", "input": {"mappings": [
                           {"path": "data.b", "logic": 2}]}}}] }"#,
                )
                .unwrap(),
            ])
            .expect("should compile");
        assert!(workflows.iter().all(joins_sync_run));

        let exec = executor(compiler.into_engine());
        let mut message = Message::from_value(&json!({}));
        exec.run_all_borrowed(&workflows, &mut message, None, Utc::now())
            .await
            .expect("both should run");

        // `b`'s condition reads what `a` wrote, which only works if the shared
        // arena context was refreshed across the workflow boundary.
        assert_eq!(message.context["data"].get("b"), Some(&dv(json!(2))));
        assert_eq!(counters(&message), vec![None, None]);
    }

    #[tokio::test]
    async fn a_loop_body_can_index_an_array_by_its_counter() {
        // The per-item pattern, using only core operators.
        let (workflow, engine) = compiled(
            r#"{ "id": "w", "name": "w", "loop": {"counter": "i", "max": 3},
                 "tasks": [{"id": "pick", "name": "pick", "function": {"name": "map",
                    "input": {"mappings": [
                       {"path": "data.picked",
                        "logic": {"merge": [{"var": "data.picked"},
                                            [{"val": [["data", "items",
                                                       {"var": "temp_data.i"}]]}]]}}]}}}] }"#,
        );
        let mut message = Message::builder()
            .data(dv(json!({"items": ["a", "b", "c"], "picked": []})))
            .build();

        executor(engine)
            .execute(&workflow, &mut message, Utc::now())
            .await
            .expect("loop should complete");

        assert_eq!(
            serde_json::Value::from(&message.context["data"]["picked"]),
            json!(["a", "b", "c"]),
            "each sweep appended the item at its own index"
        );
    }

    #[tokio::test]
    async fn test_workflow_executor_skip_condition() {
        // Create a workflow with a false condition
        let workflow_json = r#"{
            "id": "test_workflow",
            "name": "Test Workflow",
            "condition": false,
            "tasks": [{
                "id": "dummy_task",
                "name": "Dummy Task",
                "function": {
                    "name": "map",
                    "input": {"mappings": []}
                }
            }]
        }"#;

        let compiler = LogicCompiler::new();
        let mut workflow = Workflow::from_json(workflow_json).unwrap();

        // Compile the workflow condition
        let workflows = compiler.compile_workflows(vec![workflow.clone()]).unwrap();
        if let Some(compiled_workflow) = workflows.iter().find(|w| w.id == "test_workflow") {
            workflow = compiled_workflow.clone();
        }

        let engine = compiler.into_engine();
        let task_executor = Arc::new(TaskExecutor::new(
            Arc::new(HashMap::new()),
            Arc::clone(&engine),
        ));
        let workflow_executor = WorkflowExecutor::new(task_executor, engine);

        let mut message = Message::from_value(&json!({}));

        // Execute workflow - should be skipped due to false condition
        let executed = workflow_executor
            .execute(&workflow, &mut message, Utc::now())
            .await
            .unwrap();
        assert!(!executed);
        assert_eq!(message.audit_trail.len(), 0);
    }

    #[tokio::test]
    async fn test_workflow_executor_execute_success() {
        // Create a workflow with a true condition
        let workflow_json = r#"{
            "id": "test_workflow",
            "name": "Test Workflow",
            "condition": true,
            "tasks": [{
                "id": "dummy_task",
                "name": "Dummy Task",
                "function": {
                    "name": "map",
                    "input": {"mappings": []}
                }
            }]
        }"#;

        let compiler = LogicCompiler::new();
        let mut workflow = Workflow::from_json(workflow_json).unwrap();

        // Compile the workflow
        let workflows = compiler.compile_workflows(vec![workflow.clone()]).unwrap();
        if let Some(compiled_workflow) = workflows.iter().find(|w| w.id == "test_workflow") {
            workflow = compiled_workflow.clone();
        }

        let engine = compiler.into_engine();
        let task_executor = Arc::new(TaskExecutor::new(
            Arc::new(HashMap::new()),
            Arc::clone(&engine),
        ));
        let workflow_executor = WorkflowExecutor::new(task_executor, engine);

        let mut message = Message::from_value(&json!({}));

        // Execute workflow - should succeed with empty task list
        let executed = workflow_executor
            .execute(&workflow, &mut message, Utc::now())
            .await
            .unwrap();
        assert!(executed);
    }
}
