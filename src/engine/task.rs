//! # Task Module
//!
//! This module defines the `Task` structure, which represents a single
//! processing unit within a workflow. Tasks are the fundamental building
//! blocks of data processing pipelines.

use crate::engine::functions::FunctionConfig;
use datalogic_rs::Logic;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// A contiguous run of tasks sharing one condition, and optionally ending the
/// workflow once the run completes.
///
/// Groups are authored as nested objects inside a workflow's `tasks` list — an
/// element carrying a `tasks` key is a group, one carrying a `function` key is
/// a [`Task`]. The parser flattens the tree into `Workflow::tasks` and records
/// each group's span on the task that opens it ([`Task::group_starts`]), so the
/// executor keeps walking a flat slice.
///
/// The condition is evaluated **once, on entry**. A false result skips every
/// task in the span; the individual tasks' own conditions are not evaluated.
///
/// # Example JSON Definition
///
/// ```json
/// {
///     "id": "have_videos",
///     "condition": {">": [{"length": [{"var": "temp_data.videos"}]}, 0]},
///     "terminal": false,
///     "tasks": [
///         {"id": "rank", "name": "Rank", "function": {"name": "map", "input": {}}},
///         {"id": "trim", "name": "Trim", "function": {"name": "map", "input": {}}}
///     ]
/// }
/// ```
///
/// `#[non_exhaustive]`: groups are produced by the workflow parser, never built
/// by hand — `end` is an index into the *flattened* task list and means nothing
/// on its own. Read the fields freely.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct TaskGroup {
    /// Unique identifier for the group within the workflow. Shares the task
    /// id namespace, so a group cannot reuse a task's id.
    pub id: String,

    /// Human-readable name.
    pub name: Option<String>,

    /// Optional description explaining what the group covers.
    pub description: Option<String>,

    /// JSONLogic condition gating the whole span. Defaults to `true`.
    pub condition: Value,

    /// Engine-internal: pre-compiled JSONLogic for `condition`, populated by
    /// `LogicCompiler`. `None` is treated as "always enter" by the executor.
    /// Not part of the stable API.
    #[doc(hidden)]
    pub compiled_condition: Option<Arc<Logic>>,

    /// Whether reaching the end of this group ends the workflow.
    pub terminal: bool,

    /// Engine-internal: exclusive end of the span, as an index into
    /// `Workflow::tasks`. The start is the index of the task carrying this
    /// entry in its [`Task::group_starts`]. Not part of the stable API.
    #[doc(hidden)]
    pub end: usize,
}

/// When a task's own outcome ends the workflow — the outcome complement of
/// [`Task::terminal`].
///
/// Authored as a string on a task: `"halt_on": "failure"`. Absent means
/// [`Self::Never`], so every workflow written before this existed is unchanged.
///
/// `#[non_exhaustive]`: further modes (a status range, say) would otherwise
/// break every downstream `match`. Match with a `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HaltOn {
    /// Never halt on outcome. The default, and the behaviour of every task that
    /// does not mention `halt_on`.
    #[default]
    Never,

    /// Halt once this task has run and failed — a recorded status of `400` or
    /// above, which covers the `validation` built-in's `400`, any handler
    /// returning [`TaskOutcome::Status`](crate::engine::task_outcome::TaskOutcome::Status)
    /// in that range, and a handler returning `Err` (recorded as `500`).
    ///
    /// [`TaskOutcome::Skip`](crate::engine::task_outcome::TaskOutcome::Skip)
    /// records nothing and never halts, and
    /// [`TaskOutcome::Halt`](crate::engine::task_outcome::TaskOutcome::Halt)
    /// carries `HALT_STATUS_CODE` (`299`), below the threshold — it halts on its
    /// own account, not through this flag.
    ///
    /// There is no `"always"`: [`Task::terminal`] already spells that.
    Failure,
}

/// A single processing unit within a workflow (also known as an Action in rules-engine terminology).
///
/// Tasks execute functions with optional conditions and error handling.
/// They are processed sequentially within a workflow, allowing later tasks
/// to depend on results from earlier ones.
///
/// # Example JSON Definition
///
/// ```json
/// {
///     "id": "validate_user",
///     "name": "Validate User Data",
///     "description": "Ensures user data meets requirements",
///     "condition": {">=": [{"var": "data.order.total"}, 1000]},
///     "function": {
///         "name": "validation",
///         "input": { "rules": [...] }
///     },
///     "continue_on_error": false,
///     "terminal": false,
///     "halt_on": "failure"
/// }
/// ```
/// A single unit of work inside a workflow.
///
/// `#[non_exhaustive]`: construct through [`Task::action`] and assign the
/// public fields you need, or parse a workflow from JSON. Field reads and
/// writes are unaffected, and `..` patterns keep working.
///
/// The attribute exists because three of this struct's fields — `id_arc`,
/// `compiled_condition`, `group_starts` — are engine internals documented as
/// *not part of the stable API*, yet struct-literal construction forced every
/// caller to name them. Field additions had already broken those callers twice
/// (3.3.0, 3.6.0); this is the change that stops it.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct Task {
    /// Unique identifier for the task within the workflow.
    pub id: String,

    /// Engine-internal: `Arc<str>` mirror of `id`, populated by
    /// `LogicCompiler::compile_workflows`. Audit-trail emission clones this
    /// instead of allocating a fresh `Arc<str>`. Public for crate-internal
    /// access from the compiler and tests; not part of the stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub id_arc: Arc<str>,

    /// Human-readable name for the task.
    pub name: String,

    /// Optional description explaining what the task does.
    pub description: Option<String>,

    /// JSONLogic condition that determines if the task should execute.
    /// Conditions can access any context field (`data`, `metadata`, `temp_data`).
    /// Defaults to `true` (always execute).
    #[serde(default = "crate::engine::utils::default_condition")]
    pub condition: Value,

    /// Engine-internal: pre-compiled JSONLogic for `condition`, populated by
    /// `LogicCompiler`. `None` is treated as "always run" by the executor.
    /// Not part of the stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub compiled_condition: Option<Arc<Logic>>,

    /// The function configuration specifying what operation to perform.
    /// Can be a built-in function (map, validation) or a custom function.
    pub function: FunctionConfig,

    /// Whether to continue workflow execution if this task fails.
    /// When `true`, errors are recorded but don't stop the workflow.
    /// Defaults to `false`.
    ///
    /// **"Fails" means a `5xx` outcome or a returned `Err` — not every
    /// unsuccessful task.** A `4xx` is logged as a warning and the workflow
    /// carries on regardless of this flag, so `continue_on_error: false` after a
    /// `validation` task does *not* stop the tasks that follow it: a failing rule
    /// returns `400`. To gate on an outcome, use [`Task::halt_on`]; to reject the
    /// whole message, return an `Err` from a handler.
    #[serde(default)]
    pub continue_on_error: bool,

    /// Whether running this task ends the workflow. Defaults to `false`.
    ///
    /// `terminal` is a statement about *position* — "nothing after this runs" —
    /// not about outcome:
    ///
    /// - a false `condition` means the task never ran, so nothing halts;
    /// - [`TaskOutcome::Skip`](crate::engine::task_outcome::TaskOutcome::Skip)
    ///   does not halt, for the same reason;
    /// - a task that *failed* under `continue_on_error: true` still halts, and
    ///   its error is still recorded on `message.errors()`.
    ///
    /// Halting stops this workflow only; later workflows registered on the same
    /// engine still process the message. Inside a workflow carrying a
    /// [`LoopConfig`](crate::engine::workflow::LoopConfig) it breaks the whole
    /// loop, not one sweep — the same scope as
    /// [`TaskOutcome::Halt`](crate::engine::task_outcome::TaskOutcome::Halt).
    ///
    /// The audit-trail entry keeps the task's *own* status (`200`, `404`, …)
    /// rather than `HALT_STATUS_CODE`: the task did its job, and a `map` that
    /// wrote a 404 response body should not report "a filter halted here".
    ///
    /// For the *outcome* axis — "halt only if this task failed" — see
    /// [`Task::halt_on`].
    #[serde(default)]
    pub terminal: bool,

    /// Halt the workflow based on this task's **outcome**. Defaults to
    /// [`HaltOn::Never`].
    ///
    /// The complement of [`Task::terminal`]: `terminal` is about position and
    /// halts whatever happened, `halt_on` is about what happened and halts only
    /// then. The two combine as `terminal || (halt_on matched)` — `terminal` is
    /// strictly stronger, so setting both is redundant rather than contradictory.
    ///
    /// This is what lets an assertion reject. A `validation` task returns `400`
    /// when a rule fails, which is *not* covered by
    /// [`continue_on_error`](Task::continue_on_error), so without `halt_on` the
    /// tasks after it still run:
    ///
    /// ```json
    /// { "id": "check_state", "halt_on": "failure",
    ///   "function": { "name": "validation", "input": { "rules": [ … ] } } }
    /// ```
    ///
    /// **Failure means a recorded status of `400` or above** — the same
    /// threshold the executor already splits on to warn (4xx) and to record
    /// `TASK_STATUS_ERROR` (5xx) — or a handler returning `Err`, recorded as
    /// `500`. It is deliberately *not* "the task appended to
    /// `message.errors()`": a handler may call
    /// [`TaskContext::add_error`](crate::TaskContext::add_error) and still
    /// return `Success`, and that does not halt.
    ///
    /// | The task … | `terminal: true` | `halt_on: "failure"` |
    /// |---|---|---|
    /// | never ran (its `condition`, or its group's, was false) | no | no |
    /// | returned [`TaskOutcome::Skip`](crate::TaskOutcome::Skip) | no | no |
    /// | returned `Success`, or a 2xx–3xx status | **halts** | no |
    /// | returned [`TaskOutcome::Halt`](crate::TaskOutcome::Halt) | halts already | halts already |
    /// | returned a 4xx status | **halts** | **halts** |
    /// | returned 5xx, `continue_on_error: true` | **halts** | **halts** |
    /// | returned 5xx, `continue_on_error: false` | error propagates | error propagates |
    /// | handler returned `Err`, `continue_on_error: true` | **halts** | **halts** |
    /// | handler returned `Err`, `continue_on_error: false` | error propagates | error propagates |
    /// | called `add_error` but returned `Success` | **halts** | no |
    ///
    /// The two "error propagates" rows are not an omission: the executor returns
    /// `Err` before either flag is consulted, which abandons the rest of this
    /// workflow — everything halting would have done — and additionally reaches
    /// the caller. They differ in exactly one shape: a workflow carrying a
    /// [`loop`](crate::engine::workflow::LoopConfig) whose *own*
    /// `continue_on_error` is `true`, where the error advances to the next sweep
    /// while a halt would break the loop. Set `continue_on_error: false` on the
    /// workflow if the loop must stop.
    ///
    /// Everything `terminal` documents about *scope* applies unchanged: halting
    /// stops this workflow only, later workflows still process the message, it
    /// breaks a whole loop rather than one sweep, and the audit entry keeps the
    /// task's own status — `400`, not `HALT_STATUS_CODE`. **Halting is therefore
    /// not a security control**: to stop a message outright return an `Err`, or
    /// gate the following workflow on
    /// [`EngineBuilder::with_error_context_path`](crate::EngineBuilder::with_error_context_path).
    #[serde(default)]
    pub halt_on: HaltOn,

    /// Engine-internal: groups opening at this task, outermost first. Populated
    /// by the workflow parser; empty for a task in no group. Not part of the
    /// stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub group_starts: Vec<TaskGroup>,
}

impl Task {
    /// Create a task (action) with default settings.
    ///
    /// This is a convenience constructor for the IFTTT-style rules engine pattern,
    /// creating an action that always executes (condition defaults to `true`).
    ///
    /// # Arguments
    /// * `id` - Unique identifier for the action
    /// * `name` - Human-readable name
    /// * `function` - The function configuration to execute
    pub fn action(id: &str, name: &str, function: FunctionConfig) -> Self {
        Self {
            id: id.to_string(),
            id_arc: Arc::from(id),
            name: name.to_string(),
            description: None,
            condition: Value::Bool(true),
            compiled_condition: None,
            function,
            continue_on_error: false,
            terminal: false,
            halt_on: HaltOn::Never,
            group_starts: Vec::new(),
        }
    }
}
