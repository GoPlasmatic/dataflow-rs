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
///     "terminal": false
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
    #[serde(default)]
    pub terminal: bool,

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
            group_starts: Vec::new(),
        }
    }
}
