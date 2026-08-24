//! Authoring-time validation: checking a workflow definition *before* it
//! reaches [`Engine::build`](crate::Engine::build).
//!
//! The engine's own enforcement — parse, [`Workflow::validate`], and
//! `LoopConfig::validate` — all fires when the *engine* is built. For a host
//! that stores definitions and builds one engine over many of them, that is the
//! wrong time (one bad row aborts the whole build, at reload, for every
//! workflow in the process), the wrong shape (one stringly error, so an
//! authoring API cannot point a 400 at `tasks[1].tasks[0].id`), and the wrong
//! cardinality (fail-fast, so the author fixes one violation per round trip).
//!
//! [`Workflow::validate_authored`] answers all three, and carries one
//! guarantee:
//!
//! > It returns empty **if and only if** the JSON parses into a [`Workflow`]
//! > and that workflow validates.
//!
//! That biconditional is true *by construction*, not by keeping a rule list in
//! sync — see the stages below.

use crate::engine::steps::{StepKind, walk_authored_steps};
use crate::engine::workflow::Workflow;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;

/// One problem with a workflow definition.
///
/// Shared by [`Workflow::validate_authored`] and, from the registry side,
/// `EngineBuilder::check_workflow`. Both fill what they genuinely know: a
/// definition check always has an authored coordinate and knows the step id
/// when the problem concerns a step; a registry check always has a task id and
/// reports a path relative to that task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowIssue {
    /// Stable machine-readable classification.
    pub code: IssueCode,
    /// Human-readable explanation. Not stable — branch on [`Self::code`].
    pub message: String,
    /// Where the problem is. From [`Workflow::validate_authored`] this is the
    /// coordinate the author typed, rooted at the workflow document:
    /// `tasks[1].tasks[0].id`.
    pub path: Option<String>,
    /// The step this concerns, when it concerns one. Step ids are unique across
    /// tasks *and* groups, so this identifies a step on its own.
    pub task_id: Option<String>,
}

impl WorkflowIssue {
    fn at(code: IssueCode, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            path: Some(path.into()),
            task_id: None,
        }
    }

    fn with_step(mut self, id: Option<&str>) -> Self {
        self.task_id = id.map(str::to_string);
        self
    }
}

impl fmt::Display for WorkflowIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(path) => write!(f, "{path}: {} [{}]", self.message, self.code.as_str()),
            None => write!(f, "{} [{}]", self.message, self.code.as_str()),
        }
    }
}

/// Why a workflow definition is not loadable.
///
/// `#[non_exhaustive]`: a later minor may add a rule, and a host matching on
/// the codes it cares about should keep compiling. Use [`Self::as_str`] to
/// serialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum IssueCode {
    /// `id` is missing or empty.
    EmptyWorkflowId,
    /// `name` is missing or empty.
    EmptyWorkflowName,
    /// `tasks` is missing, not an array, or empty.
    NoTasks,
    /// A step carries no `id`.
    MissingStepId,
    /// Two steps share an id. Groups share the task id namespace.
    DuplicateStepId,
    /// A group's `tasks` is not a non-empty array.
    EmptyGroup,
    /// A group is nested at or beyond
    /// [`MAX_GROUP_DEPTH`](crate::engine::steps::MAX_GROUP_DEPTH).
    GroupTooDeep,
    /// A task carries no `function`.
    MissingFunction,
    /// `function` is not an object, or its `name` is missing or empty.
    InvalidFunctionName,
    /// `terminal` is present but not a boolean.
    InvalidTerminal,
    /// `loop.increment` is below 1 — the counter would never reach `max`.
    LoopIncrementTooSmall,
    /// `loop.max` is not greater than `loop.init` — no sweep could ever run.
    LoopBoundEmpty,
    /// `loop.counter` is not a non-empty dotted path.
    LoopCounterInvalid,
    /// The document does not deserialize into a [`Workflow`]. Carries the
    /// parser's own message, which names the offending field and type.
    ParseFailed,
    /// The document parses but [`Workflow::validate`] rejects it. A backstop:
    /// reaching this means a rule exists that the checks above do not model.
    ValidateFailed,
}

impl IssueCode {
    /// The stable string form, for serializing into an API response.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EmptyWorkflowId => "EMPTY_WORKFLOW_ID",
            Self::EmptyWorkflowName => "EMPTY_WORKFLOW_NAME",
            Self::NoTasks => "NO_TASKS",
            Self::MissingStepId => "MISSING_STEP_ID",
            Self::DuplicateStepId => "DUPLICATE_STEP_ID",
            Self::EmptyGroup => "EMPTY_GROUP",
            Self::GroupTooDeep => "GROUP_TOO_DEEP",
            Self::MissingFunction => "MISSING_FUNCTION",
            Self::InvalidFunctionName => "INVALID_FUNCTION_NAME",
            Self::InvalidTerminal => "INVALID_TERMINAL",
            Self::LoopIncrementTooSmall => "LOOP_INCREMENT_TOO_SMALL",
            Self::LoopBoundEmpty => "LOOP_BOUND_EMPTY",
            Self::LoopCounterInvalid => "LOOP_COUNTER_INVALID",
            Self::ParseFailed => "PARSE_FAILED",
            Self::ValidateFailed => "VALIDATE_FAILED",
        }
    }
}

impl fmt::Display for IssueCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Workflow {
    /// Check authored workflow JSON without building an engine.
    ///
    /// Returns empty **if and only if** the JSON parses into a [`Workflow`] and
    /// that workflow validates — so an empty result is a promise the engine
    /// will accept the definition, not merely that a list of rules was
    /// satisfied.
    ///
    /// # How the guarantee holds
    ///
    /// Three stages. A structural walk collects *every* semantic violation with
    /// the coordinate the author typed; if it finds none, the document is then
    /// actually parsed and validated, and either failure is reported as one
    /// further issue. The second and third stages are what make the promise
    /// true by construction: the crate's serde schema is far larger than any
    /// rule list — a `"priority": "high"` or a `map` task missing `mappings`
    /// breaks no *semantic* rule and still cannot load — and mirroring it here
    /// would recreate the very drift this API exists to remove.
    ///
    /// # Example
    ///
    /// ```
    /// use dataflow_rs::{IssueCode, Workflow};
    /// use serde_json::json;
    ///
    /// let broken = json!({
    ///     "id": "w", "name": "w", "priority": 0,
    ///     "tasks": [
    ///         {"id": "dup", "name": "a", "function": {"name": "map", "input": {"mappings": []}}},
    ///         {"id": "dup", "name": "b", "function": {"name": "map", "input": {"mappings": []}}}
    ///     ]
    /// });
    ///
    /// let issues = Workflow::validate_authored(&broken);
    /// assert_eq!(issues[0].code, IssueCode::DuplicateStepId);
    /// assert_eq!(issues[0].path.as_deref(), Some("tasks[1].id"));
    /// assert_eq!(issues[0].task_id.as_deref(), Some("dup"));
    /// ```
    ///
    /// Every problem is reported, not just the first:
    ///
    /// ```
    /// # use dataflow_rs::{IssueCode, Workflow};
    /// # use serde_json::json;
    /// let issues = Workflow::validate_authored(&json!({
    ///     "id": "", "name": "w", "tasks": [{"id": "t", "name": "t"}]
    /// }));
    ///
    /// let codes: Vec<IssueCode> = issues.iter().map(|i| i.code).collect();
    /// assert!(codes.contains(&IssueCode::EmptyWorkflowId));
    /// assert!(codes.contains(&IssueCode::MissingFunction));
    /// ```
    pub fn validate_authored(json: &Value) -> Vec<WorkflowIssue> {
        let mut issues = check_shape(json);
        if !issues.is_empty() {
            return issues;
        }

        // Stage 2 — the schema is much wider than the rules above, and
        // enumerating it here would be the mirror this API exists to delete.
        let workflow: Workflow = match serde_json::from_value(json.clone()) {
            Ok(w) => w,
            Err(err) => {
                issues.push(WorkflowIssue {
                    code: IssueCode::ParseFailed,
                    message: err.to_string(),
                    path: None,
                    task_id: None,
                });
                return issues;
            }
        };

        // Stage 3 — a backstop. Reaching this means `check_shape` does not
        // model some rule `validate` enforces; the caller still gets a correct
        // answer, and the test suite is what keeps this unreachable.
        if let Err(err) = workflow.validate() {
            issues.push(WorkflowIssue {
                code: IssueCode::ValidateFailed,
                message: err.to_string(),
                path: None,
                task_id: None,
            });
        }
        issues
    }
}

/// Stage 1: every semantic violation, with authored coordinates.
fn check_shape(json: &Value) -> Vec<WorkflowIssue> {
    let mut issues = Vec::new();

    if non_empty_str(json.get("id")).is_none() {
        issues.push(WorkflowIssue::at(
            IssueCode::EmptyWorkflowId,
            "id",
            "workflow id must be a non-empty string",
        ));
    }
    if non_empty_str(json.get("name")).is_none() {
        issues.push(WorkflowIssue::at(
            IssueCode::EmptyWorkflowName,
            "name",
            "workflow name must be a non-empty string",
        ));
    }

    match json.get("tasks").and_then(Value::as_array) {
        Some(tasks) if !tasks.is_empty() => {}
        _ => issues.push(WorkflowIssue::at(
            IssueCode::NoTasks,
            "tasks",
            "workflow must have at least one task",
        )),
    }

    check_steps(json.get("tasks").unwrap_or(&Value::Null), &mut issues);

    if let Some(loop_config) = json.get("loop") {
        check_loop(loop_config, &mut issues);
    }

    issues
}

/// Walk the authored step tree, checking each node and the id namespace.
///
/// Built on [`walk_authored_steps`], so the group test, the traversal order and
/// the depth cap have exactly one definition shared with the parser.
fn check_steps(tasks: &Value, issues: &mut Vec<WorkflowIssue>) {
    // Step id -> the path that first claimed it.
    let mut seen: HashMap<&str, String> = HashMap::new();

    for step in walk_authored_steps(tasks) {
        let id = non_empty_str(step.node.get("id"));

        match id {
            None => issues.push(
                WorkflowIssue::at(
                    IssueCode::MissingStepId,
                    format!("{}.id", step.path),
                    "every step needs a non-empty id",
                )
                .with_step(None),
            ),
            Some(id) => {
                if let Some(first) = seen.get(id) {
                    issues.push(
                        WorkflowIssue::at(
                            IssueCode::DuplicateStepId,
                            format!("{}.id", step.path),
                            format!(
                                "step id '{id}' is already used at {first} — task groups \
                                 share the task id namespace"
                            ),
                        )
                        .with_step(Some(id)),
                    );
                } else {
                    seen.insert(id, step.path.clone());
                }
            }
        }

        if let Some(terminal) = step.node.get("terminal") {
            if !terminal.is_boolean() {
                issues.push(
                    WorkflowIssue::at(
                        IssueCode::InvalidTerminal,
                        format!("{}.terminal", step.path),
                        "terminal must be a boolean",
                    )
                    .with_step(id),
                );
            }
        }

        match step.kind {
            StepKind::Leaf => check_function(&step.path, step.node, id, issues),
            StepKind::Group => {
                // The parser rejects a group whose `tasks` is not a non-empty
                // array; the walker reports the node so we can say which.
                let has_members = step
                    .node
                    .get("tasks")
                    .and_then(Value::as_array)
                    .is_some_and(|members| !members.is_empty());
                if !has_members {
                    issues.push(
                        WorkflowIssue::at(
                            IssueCode::EmptyGroup,
                            format!("{}.tasks", step.path),
                            "a task group's tasks must be a non-empty array — \
                             an empty group can only be a mistake",
                        )
                        .with_step(id),
                    );
                }
            }
            StepKind::TooDeep => issues.push(
                WorkflowIssue::at(
                    IssueCode::GroupTooDeep,
                    step.path.clone(),
                    format!(
                        "task groups nested deeper than {} levels",
                        crate::engine::steps::MAX_GROUP_DEPTH
                    ),
                )
                .with_step(id),
            ),
        }
    }
}

/// A leaf must carry a `function` object with a non-empty `name`.
fn check_function(path: &str, node: &Value, id: Option<&str>, issues: &mut Vec<WorkflowIssue>) {
    let Some(function) = node.get("function") else {
        issues.push(
            WorkflowIssue::at(
                IssueCode::MissingFunction,
                format!("{path}.function"),
                "a task needs a function — an element with neither `function` nor \
                 `tasks` is neither a task nor a group",
            )
            .with_step(id),
        );
        return;
    };

    if !function.is_object() || non_empty_str(function.get("name")).is_none() {
        issues.push(
            WorkflowIssue::at(
                IssueCode::InvalidFunctionName,
                format!("{path}.function.name"),
                "function must be an object with a non-empty name",
            )
            .with_step(id),
        );
    }
}

/// The three `LoopConfig::validate` rules, against the authored JSON.
fn check_loop(config: &Value, issues: &mut Vec<WorkflowIssue>) {
    // Absent fields take their serde defaults, which are valid; only a present
    // field can be wrong here. A non-integer is a *type* error and belongs to
    // stage 2, so it is deliberately not reported twice.
    if let Some(increment) = config.get("increment").and_then(Value::as_i64) {
        if increment < 1 {
            issues.push(WorkflowIssue::at(
                IssueCode::LoopIncrementTooSmall,
                "loop.increment",
                format!(
                    "loop increment must be >= 1, got {increment} \
                     (a non-advancing counter would never reach max)"
                ),
            ));
        }
    }

    let init = config.get("init").and_then(Value::as_i64).unwrap_or(0);
    if let Some(max) = config.get("max").and_then(Value::as_i64) {
        if max <= init {
            issues.push(WorkflowIssue::at(
                IssueCode::LoopBoundEmpty,
                "loop.max",
                format!(
                    "loop max ({max}) must be greater than init ({init}) — \
                     the bound is half-open, so this could never run a sweep"
                ),
            ));
        }
    }

    if let Some(counter) = config.get("counter") {
        if let Some(counter) = counter.as_str() {
            if counter.is_empty() || counter.split('.').any(str::is_empty) {
                issues.push(WorkflowIssue::at(
                    IssueCode::LoopCounterInvalid,
                    "loop.counter",
                    format!(
                        "loop counter must be a non-empty temp_data field path, got {counter:?}"
                    ),
                ));
            }
        }
    }
}

/// The value at `field` as a non-empty string, if it is one.
fn non_empty_str(field: Option<&Value>) -> Option<&str> {
    field.and_then(Value::as_str).filter(|s| !s.is_empty())
}
