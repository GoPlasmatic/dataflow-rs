//! Authoring-time validation: checking a workflow definition *before* it
//! reaches [`EngineBuilder::build`](crate::EngineBuilder::build).
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

use crate::engine::compiler::TEMPLATE_KEY_ESCAPE;
use crate::engine::functions::config::{BuiltinKind, builtin_function_kind, can_dispatch_in};
use crate::engine::functions::{BoxedFunctionHandler, FunctionConfig, TemplateCompiler};
use crate::engine::secrets::{SECRET_OPERATOR, Secrets};
use crate::engine::steps::{StepKind, walk_authored_steps};
use crate::engine::task::HaltOn;
use crate::engine::workflow::Workflow;
use serde::Deserialize;
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
    /// When this issue bites — [`IssueCode::severity`] of [`Self::code`].
    ///
    /// Screening before a build is the point: everything but
    /// [`Severity::Advisory`] means the workflow will not do what it says.
    pub fn severity(&self) -> Severity {
        self.code.severity()
    }

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

/// What a check found wrong with a workflow definition.
///
/// Not every code is a refusal: [`Self::severity`] says whether the definition
/// is rejected, builds and then fails every message, or merely carries a shape
/// worth an author's attention. Ask it rather than keeping a list — see
/// [`Severity`].
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
    /// A `validation` task whose failure cannot stop anything: it is not
    /// `terminal`, has no `halt_on`, and every task after it in the workflow is
    /// unguarded. A failing rule returns `400`, which `continue_on_error` does
    /// not cover, so the pipeline proceeds as if it had passed.
    ///
    /// [`Severity::Advisory`] — reported by
    /// [`crate::EngineBuilder::check_workflow`] and never by
    /// [`crate::EngineBuilder::build`]. Validating purely to record errors is a
    /// legitimate shape; this only says the assertion is not acting as a gate.
    UnguardedValidation,
    /// `halt_on` is present and is not one of the accepted strings, or is
    /// present on a *group* — a group has no outcome of its own, so the
    /// executor could not honour it.
    InvalidHaltOn,
    /// A task group carries `continue_on_error`, which the engine drops. Error
    /// handling is per task and per workflow; a group only gates a span.
    ///
    /// [`Severity::Advisory`] — reported by
    /// [`crate::EngineBuilder::check_workflow`] and never by
    /// [`crate::EngineBuilder::build`]. Unlike [`Self::InvalidHaltOn`], which
    /// the parser refuses outright, this key is real at the other two levels
    /// and may already sit on a host's group nodes; refusing it would abort
    /// every workflow in the build.
    GroupContinueOnError,
    /// `loop.increment` is below 1 — the counter would never reach `max`.
    LoopIncrementTooSmall,
    /// `loop.max` is not greater than `loop.init` — no sweep could ever run.
    LoopBoundEmpty,
    /// `loop.counter` is not a non-empty dotted path.
    LoopCounterInvalid,
    /// No handler will dispatch this function name, and it is not a built-in.
    /// Usually a typo or a handler the host forgot to register.
    UnknownFunction,
    /// The name *is* a built-in, but one that ships as a config schema only
    /// (`http_call`, `enrich`, `publish_kafka`) and no handler is registered
    /// under it. The workflow builds cleanly and then fails every message.
    MissingHandler,
    /// A custom task's `input` does not deserialize into its handler's declared
    /// `Input` type.
    InputParse,
    /// A `Template` field of a custom task's input does not compile.
    TemplateCompile,
    /// An expression reads `{"secret": "name"}` and no secret of that name is
    /// declared on the engine (`EngineBuilder::with_secrets`). Only literal
    /// names are checked; a dynamic name fails at evaluation instead.
    UnknownSecret,
    /// An expression whose result the engine writes to the message or emits to
    /// a log reads a secret — a `map` mapping, or a `log` message or field. The
    /// store exists so a value is never recorded; an expression that would
    /// record it is refused outright, derived or not. Compute derived values in
    /// a custom handler.
    SecretInMessageWrite,
    /// The store passed to
    /// [`EngineBuilder::with_secrets`](crate::EngineBuilder::with_secrets) is
    /// not a JSON object, so no name resolves and
    /// [`EngineBuilder::build`](crate::EngineBuilder::build) will fail.
    /// Reported by
    /// [`EngineBuilder::check_workflow`](crate::EngineBuilder::check_workflow)
    /// *instead of* the [`Self::UnknownSecret`] issues every literal name would
    /// otherwise produce — the workflow is not what is wrong.
    ///
    /// [`Severity::Rejected`], but the odd one out: quarantining the workflow
    /// will not make the build succeed, because the store is what is broken.
    InvalidSecretStore,
    /// Two keys in one template object collapse to the same name once the
    /// template-key escape is stripped — `{"$a": 1, "a": 2}` emits `a` twice.
    /// The context is a `Vec` of pairs, so both survive: a later read sees only
    /// the first while serialization emits both. Always a bug, so
    /// [`crate::EngineBuilder::build`] refuses it.
    DuplicateTemplateKey,
    /// A template key carries the escape prefix, so it is emitted with one
    /// prefix stripped: `$type` emits `type`. [`Severity::Advisory`] — reported
    /// by [`crate::Engine::check_workflow`] and never by
    /// [`crate::EngineBuilder::build`].
    ///
    /// Exists for migration. The escape strips uniformly from every template
    /// key, so a workflow written before 3.9 that emits genuinely `$`-prefixed
    /// keys — MongoDB's `$set`/`$oid`, JSON Schema's `$schema`/`$ref` — changes
    /// what it produces, silently. This lists every one so the audit is
    /// mechanical rather than archaeological.
    EscapedTemplateKey,
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
            Self::InvalidHaltOn => "INVALID_HALT_ON",
            Self::GroupContinueOnError => "GROUP_CONTINUE_ON_ERROR",
            Self::UnguardedValidation => "UNGUARDED_VALIDATION",
            Self::LoopIncrementTooSmall => "LOOP_INCREMENT_TOO_SMALL",
            Self::LoopBoundEmpty => "LOOP_BOUND_EMPTY",
            Self::LoopCounterInvalid => "LOOP_COUNTER_INVALID",
            Self::UnknownFunction => "UNKNOWN_FUNCTION",
            Self::MissingHandler => "MISSING_HANDLER",
            Self::InputParse => "INPUT_PARSE",
            Self::TemplateCompile => "TEMPLATE_COMPILE",
            Self::UnknownSecret => "UNKNOWN_SECRET",
            Self::SecretInMessageWrite => "SECRET_IN_MESSAGE_WRITE",
            Self::InvalidSecretStore => "INVALID_SECRET_STORE",
            Self::DuplicateTemplateKey => "DUPLICATE_TEMPLATE_KEY",
            Self::EscapedTemplateKey => "ESCAPED_TEMPLATE_KEY",
            Self::ParseFailed => "PARSE_FAILED",
            Self::ValidateFailed => "VALIDATE_FAILED",
        }
    }

    /// When an issue carrying this code bites — see [`Severity`].
    ///
    /// The match below has **no wildcard arm**, and must never gain one. That
    /// is the whole mechanism: a code added in a later minor stops this
    /// compiling until it is deliberately classified, so a new advisory code
    /// cannot arrive as a silent `Rejected` in a host's pre-build screen — the
    /// direction that turns a screen into a quarantine.
    pub fn severity(self) -> Severity {
        match self {
            // Reported for an author's attention. `build` accepts all three
            // and the workflow runs exactly as written.
            Self::UnguardedValidation | Self::GroupContinueOnError | Self::EscapedTemplateKey => {
                Severity::Advisory
            }

            // Builds cleanly, then fails every message. The config parses into
            // a typed built-in variant, so `precompile_custom_inputs` never
            // looks at it — which is why the builder cannot catch this one.
            Self::MissingHandler => Severity::Defect,

            // Refused by `refuse_authoring_issues`, before compilation — and
            // `InvalidSecretStore` earlier still, by `build` itself.
            Self::UnknownSecret
            | Self::SecretInMessageWrite
            | Self::DuplicateTemplateKey
            | Self::InvalidSecretStore => Severity::Rejected,

            // Refused by `precompile_custom_inputs`.
            Self::UnknownFunction | Self::InputParse | Self::TemplateCompile => Severity::Rejected,

            // Never parses into a `Workflow`, or fails `Workflow::validate`
            // when `compile_workflows` runs it.
            Self::EmptyWorkflowId
            | Self::EmptyWorkflowName
            | Self::NoTasks
            | Self::MissingStepId
            | Self::DuplicateStepId
            | Self::EmptyGroup
            | Self::GroupTooDeep
            | Self::MissingFunction
            | Self::InvalidFunctionName
            | Self::InvalidTerminal
            | Self::InvalidHaltOn
            | Self::LoopIncrementTooSmall
            | Self::LoopBoundEmpty
            | Self::LoopCounterInvalid
            | Self::ParseFailed
            | Self::ValidateFailed => Severity::Rejected,
        }
    }
}

impl fmt::Display for IssueCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// When an issue bites: at [`build`](crate::EngineBuilder::build), at the first
/// message, or never.
///
/// This is what a host screening definitions *before* building needs in order
/// to quarantine one workflow rather than fail the whole engine. Not every code
/// `check_workflow` reports is a refusal, and the set that is has grown apart
/// from the set that is reported — [`Self::Advisory`] is the only class such a
/// host may safely ignore.
///
/// Unlike [`IssueCode`], this is **not** `#[non_exhaustive]`. The axis is
/// *when* a definition goes wrong, and build time / first message / never
/// exhausts that by construction, so a host may `match` it and stay correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// The definition never runs: [`build`](crate::EngineBuilder::build)
    /// refuses it, or the parser rejected it before a [`Workflow`] existed.
    Rejected,
    /// It builds cleanly and then fails every message it processes — a real
    /// defect, but one the builder cannot catch. Only
    /// [`IssueCode::MissingHandler`] today.
    Defect,
    /// It loads and runs. Names a shape worth an author's attention, nothing
    /// more.
    Advisory,
}

impl Severity {
    /// The stable string form, for serializing into an API response.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rejected => "REJECTED",
            Self::Defect => "DEFECT",
            Self::Advisory => "ADVISORY",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Workflow {
    /// Check authored workflow JSON without building an engine.
    ///
    /// Returns empty **if and only if** the JSON parses into a [`Workflow`] and
    /// that workflow validates.
    ///
    /// That is the *shape* question, and it is the whole of it. It is not the
    /// same as "this engine can run it": [`EngineBuilder::build`](crate::EngineBuilder::build)
    /// also resolves every task to a handler and parses custom inputs, so a
    /// structurally perfect definition naming an unregistered function still
    /// aborts a build. [`Engine::check_workflow`](crate::Engine::check_workflow)
    /// answers that half; run both.
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
        let workflow: Self = match serde_json::from_value(json.clone()) {
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

/// Check a parsed workflow against a handler registry.
///
/// Shared by [`crate::EngineBuilder::check_workflow`] and
/// [`crate::Engine::check_workflow`] so the two cannot answer differently, and
/// run against the crate's *real* `TemplateCompiler` rather than a host's
/// reconstruction of one.
///
/// `workflow.tasks` is already flattened, so iterating it covers members of
/// task groups without any extra traversal.
pub(crate) fn check_against_registry(
    workflow: &Workflow,
    registry: &std::collections::HashMap<String, BoxedFunctionHandler>,
    template_compiler: &TemplateCompiler,
    secrets: &Secrets,
) -> Vec<WorkflowIssue> {
    let mut issues = check_secrets(workflow, secrets);
    // All three template-key findings, including the two `build()` does not
    // refuse — this surface is where a host looks before activating a
    // definition, and the migration audit is the point of reporting them.
    issues.extend(check_template_keys(workflow));
    check_unguarded_validation(workflow, &mut issues);
    check_group_continue_on_error(workflow, &mut issues);

    for task in &workflow.tasks {
        let name = task.function.function_name();

        if !can_dispatch_in(registry, name) {
            // Distinguish the two reasons, because the fixes differ: a
            // `RequiresHandler` built-in is a real name awaiting a
            // registration, while anything else is likely a typo.
            let (code, message) = match builtin_function_kind(name) {
                Some(BuiltinKind::RequiresHandler) => (
                    IssueCode::MissingHandler,
                    format!(
                        "'{name}' ships as a config schema only — register a handler under \
                         that name, or this workflow will build cleanly and fail every message"
                    ),
                ),
                _ => (
                    IssueCode::UnknownFunction,
                    format!("no handler is registered for '{name}', and it is not a built-in"),
                ),
            };
            issues.push(WorkflowIssue {
                code,
                message,
                path: Some("function.name".to_string()),
                task_id: Some(task.id.clone()),
            });
            continue;
        }

        // Only `Custom` inputs are still raw at this point: the built-in
        // variants were typed by serde when the workflow parsed.
        let FunctionConfig::Custom { name, input, .. } = &task.function else {
            continue;
        };
        let Some(handler) = registry.get(name) else {
            continue;
        };

        let mut parsed = match handler.parse_input_box(input) {
            Ok(parsed) => parsed,
            Err(err) => {
                issues.push(WorkflowIssue {
                    code: IssueCode::InputParse,
                    message: format!("input does not match the handler's Input type: {err}"),
                    path: Some("function.input".to_string()),
                    task_id: Some(task.id.clone()),
                });
                continue;
            }
        };

        if let Err(err) = handler.compile_input_box(&mut *parsed, template_compiler) {
            issues.push(WorkflowIssue {
                code: IssueCode::TemplateCompile,
                message: format!("a template field does not compile: {err}"),
                path: Some("function.input".to_string()),
                task_id: Some(task.id.clone()),
            });
        }
    }

    issues
}

/// Report a `validation` task whose failure cannot stop anything.
///
/// A failing rule returns `400`, and the engine treats `4xx` as "warn and carry
/// on" — `continue_on_error` governs `5xx` and `Err` only. So a `validation`
/// followed by unguarded tasks is a *decorative* assertion: it records an error
/// on the message and the pipeline proceeds exactly as if it had passed. That is
/// how a CSRF check ships that never rejects anything.
///
/// **Informational.** Reported here, never by
/// [`EngineBuilder::build`](crate::EngineBuilder::build) — the workflow is
/// legal, and plenty of them validate for reporting rather than for control
/// flow.
///
/// Deliberately blunt: it asks "is there *any* gate after this assertion?", not
/// "is the gate correct". A guard's condition can read anything — a
/// host-configured error-context path included — so deciding whether an
/// arbitrary expression actually tests the failure is not statically knowable.
/// Anything that could plausibly gate silences it, which keeps false positives
/// near zero at the cost of missing an author who wrote an unrelated condition.
fn check_unguarded_validation(workflow: &Workflow, issues: &mut Vec<WorkflowIssue>) {
    /// A condition that is not the literal `true` default might gate on the
    /// failure, so it counts as a guard.
    fn gates(condition: &Value) -> bool {
        condition != &Value::Bool(true)
    }

    for (i, task) in workflow.tasks.iter().enumerate() {
        if !matches!(task.function, FunctionConfig::Validation { .. }) {
            continue;
        }
        // The task already stops the workflow on its own account.
        if task.terminal || task.halt_on != HaltOn::Never {
            continue;
        }
        // Nothing follows it *in this workflow*, so its `400` survives in
        // `metadata.progress` for the next workflow to gate on.
        let Some(following) = workflow.tasks.get(i + 1..).filter(|t| !t.is_empty()) else {
            continue;
        };
        let guarded = following.iter().any(|next| {
            gates(&next.condition)
                || matches!(next.function, FunctionConfig::Filter { .. })
                || next.group_starts.iter().any(|g| gates(&g.condition))
        });
        if guarded {
            continue;
        }

        issues.push(WorkflowIssue {
            code: IssueCode::UnguardedValidation,
            message: format!(
                "a failing rule here records status 400 and task '{}' still runs — \
                 `continue_on_error` covers 5xx and Err only. Add `\"halt_on\": \"failure\"` \
                 to stop the workflow, or gate what follows on \
                 `metadata.progress.status_code`",
                following[0].id
            ),
            path: Some("halt_on".to_string()),
            task_id: Some(task.id.clone()),
        });
    }
}

/// Report a task group carrying `continue_on_error`, which the engine drops.
///
/// The key is real on a [`Task`](crate::Task) and on a
/// [`Workflow`](crate::Workflow), and that is exactly what makes a group the
/// one place it looks like it should work and does not: `TaskGroup` has no such
/// field, so the parser discards it like any other unknown key and the
/// definition looks correct in review.
///
/// **Informational.** The sibling mistake — `halt_on` on a group — is refused
/// at parse time instead, because it was a new key with no installed base.
/// This one is old enough that a host may already carry it on group nodes, and
/// refusing it would fail `Engine::build`, which aborts every workflow in that
/// build. Honouring it is not the answer either: propagating it to the member
/// tasks would change the error semantics of definitions that load today,
/// silently.
///
/// Only a literal `true` is reported. `continue_on_error: false` on a group
/// already describes what happens, so nothing the author wanted is defeated and
/// reporting it would be noise.
fn check_group_continue_on_error(workflow: &Workflow, issues: &mut Vec<WorkflowIssue>) {
    // Each group is recorded once, on the task that opens its span, so this
    // visits every group exactly once — outermost first where several open
    // together.
    for task in &workflow.tasks {
        for group in task.group_starts.iter().filter(|g| g.continue_on_error) {
            issues.push(WorkflowIssue {
                code: IssueCode::GroupContinueOnError,
                message: format!(
                    "group '{}' carries continue_on_error, which the engine does not \
                     honour — error handling is per task and per workflow, and a group \
                     only gates a span. Put it on the tasks inside the group, or on the \
                     workflow",
                    group.id
                ),
                path: Some("continue_on_error".to_string()),
                task_id: Some(group.id.clone()),
            });
        }
    }
}

/// Where an expression's result ends up — what decides whether it may read a
/// secret.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sink {
    /// Collapses to a bool the engine acts on: workflow, group and task
    /// conditions, `validation` rules, `filter`. Nothing of the value survives.
    Bool,
    /// Handed to a handler: a custom handler's `Template` fields and every
    /// `http_call` / `enrich` / `publish_kafka` parameter. What happens next is
    /// the handler's business.
    Handler,
    /// A custom task's whole raw input, handed to the handler untyped. The
    /// same rule as [`Sink::Handler`]; it differs only in that the document
    /// is not one expression, so an issue carries the deep path to the
    /// reference rather than the field.
    Input,
    /// Written to the message or emitted to a log by the engine itself: a
    /// `map` mapping's value *and* its destination, a `validation` rule's
    /// `message`, `log` message and fields, and the `source` / `target` /
    /// `root_element` a `parse_*` or `publish_*` names. Recorded by
    /// construction — a destination lands in `Change.path` and the audit trail.
    Message,
}

impl Sink {
    /// Whether the engine itself records this expression's result. Such an
    /// expression may not read a secret *at all* — the store exists so a value
    /// is never recorded, and there is no static line between a copy and a
    /// derived value.
    ///
    /// The four variants are a map of where an expression's result goes;
    /// [`Sink::Bool`] and [`Sink::Handler`] answer this question the same way
    /// today and are kept apart because the reasons differ — nothing survives
    /// versus the handler owns what happens next.
    fn records(self) -> bool {
        matches!(self, Self::Message)
    }

    /// Whether an issue points at the reference's own deep path rather than at
    /// the field. True only where the field is not itself a single expression,
    /// so the field name alone would not locate the reference.
    fn points_at_reference(self) -> bool {
        matches!(self, Self::Input)
    }
}

/// Check every expression in `workflow` for `{"secret": …}` references.
///
/// One implementation for two callers: `Engine::build` refuses a workflow that
/// produces any issue here, and `check_workflow` reports the same issues, so the
/// two cannot disagree about what is loadable.
///
/// A reference is any single-key object whose key is the reserved operator
/// name — datalogic compiles exactly that shape as an operator call in
/// templating mode, at any depth. A string (or one-element array) argument is
/// a literal name and is checked against `secrets`; anything else is dynamic
/// and only fails at evaluation. Both forms are refused in a
/// [`Sink::Message`] expression.
pub(crate) fn check_secrets(workflow: &Workflow, secrets: &Secrets) -> Vec<WorkflowIssue> {
    let mut issues = Vec::new();
    for_each_expression(workflow, &mut |value, field, task_id, sink| {
        check_expression(value, field, task_id, sink, secrets, &mut issues);
    });
    issues
}

/// A map-valued config field's keys, in name order.
///
/// The maps `for_each_expression` walks (`log.fields`, `http_call.headers`) are
/// `HashMap`s, so their iteration order is arbitrary — but issue order is what a
/// host logs, diffs, or asserts on, and what `refuse_authoring_issues` joins into a
/// `build()` error message. Sorting is what makes that order reproducible.
fn names_in_order<V>(map: &HashMap<String, V>) -> Vec<&String> {
    let mut names: Vec<&String> = map.keys().collect();
    names.sort_unstable();
    names
}

/// Visit every JSONLogic expression in `workflow`, with where it lives and
/// where its result goes.
///
/// The single enumeration of "which config fields are expressions". Both
/// [`check_secrets`] and [`check_template_keys`] walk it, so a parameter added
/// to a built-in cannot be checked by one and silently skipped by the other.
fn for_each_expression(
    workflow: &Workflow,
    check: &mut impl FnMut(&Value, &str, Option<&str>, Sink),
) {
    check(&workflow.condition, "condition", None, Sink::Bool);

    for task in &workflow.tasks {
        // Groups opening at this task, outermost first — compiled alongside
        // the task condition, so checked alongside it.
        for group in &task.group_starts {
            let id = Some(group.id.as_str());
            check(&group.condition, "condition", id, Sink::Bool);
        }

        let id = Some(task.id.as_str());
        check(&task.condition, "condition", id, Sink::Bool);

        match &task.function {
            FunctionConfig::Map { input, .. } => {
                for (i, mapping) in input.mappings.iter().enumerate() {
                    let field = format!("function.input.mappings[{i}].logic");
                    check(&mapping.logic, &field, id, Sink::Message);
                    // The destination is itself recorded — in `Change.path` and
                    // on the audit trail — so it may not read a secret either.
                    let field = format!("function.input.mappings[{i}].path");
                    check(mapping.path.as_json(), &field, id, Sink::Message);
                }
            }
            FunctionConfig::Validation { input, .. } => {
                for (i, rule) in input.rules.iter().enumerate() {
                    let field = format!("function.input.rules[{i}].logic");
                    check(&rule.logic, &field, id, Sink::Bool);
                    // The message is `Sink::Message`, not `Sink::Bool`: it is
                    // recorded in `Message::errors`, which is serialized. A
                    // rule may *test* a secret; it may not *report* one.
                    let field = format!("function.input.rules[{i}].message");
                    check(rule.message.as_json(), &field, id, Sink::Message);
                }
            }
            FunctionConfig::Filter { input, .. } => {
                check(&input.condition, "function.input.condition", id, Sink::Bool);
            }
            FunctionConfig::Log { input, .. } => {
                check(&input.message, "function.input.message", id, Sink::Message);
                for name in names_in_order(&input.fields) {
                    let field = format!("function.input.fields.{name}");
                    check(&input.fields[name], &field, id, Sink::Message);
                }
            }
            FunctionConfig::HttpCall { input, .. } => {
                // Everything here is handed to the host's handler, so a secret
                // is allowed — an `Authorization` header reading
                // `{"secret": "api_token"}` is the reason headers became
                // expressions at all. What the handler does with it from there
                // is the handler's business.
                check(
                    input.connector.as_json(),
                    "function.input.connector",
                    id,
                    Sink::Handler,
                );
                check(
                    input.timeout_ms.as_json(),
                    "function.input.timeout_ms",
                    id,
                    Sink::Handler,
                );
                for name in names_in_order(&input.headers) {
                    let field = format!("function.input.headers.{name}");
                    check(input.headers[name].as_json(), &field, id, Sink::Handler);
                }
                for (name, template) in [
                    ("path", &input.path),
                    ("body", &input.body),
                    ("body_format", &input.body_format),
                    ("response_path", &input.response_path),
                    ("response_format", &input.response_format),
                ] {
                    if let Some(t) = template {
                        let field = format!("function.input.{name}");
                        check(t.as_json(), &field, id, Sink::Handler);
                    }
                }
            }
            FunctionConfig::Enrich { input, .. } => {
                for (name, template) in [
                    ("connector", &input.connector),
                    ("merge_path", &input.merge_path),
                    ("timeout_ms", &input.timeout_ms),
                ] {
                    let field = format!("function.input.{name}");
                    check(template.as_json(), &field, id, Sink::Handler);
                }
                if let Some(t) = &input.path {
                    check(t.as_json(), "function.input.path", id, Sink::Handler);
                }
            }
            FunctionConfig::PublishKafka { input, .. } => {
                for (name, template) in [("connector", &input.connector), ("topic", &input.topic)] {
                    let field = format!("function.input.{name}");
                    check(template.as_json(), &field, id, Sink::Handler);
                }
                for (name, template) in [("key", &input.key), ("value", &input.value)] {
                    if let Some(t) = template {
                        let field = format!("function.input.{name}");
                        check(t.as_json(), &field, id, Sink::Handler);
                    }
                }
            }
            FunctionConfig::Custom { input, .. } => {
                check(input, "function.input", id, Sink::Input);
            }
            // `Sink::Message`, not `Sink::Handler`: these expressions name
            // where the engine itself writes, and the destination is recorded
            // in `Change.path` and the audit trail. `root_element` goes further
            // — it is written into the serialized document that lands in
            // `data.{target}`.
            FunctionConfig::ParseJson { input, .. } | FunctionConfig::ParseXml { input, .. } => {
                check(
                    input.source.as_json(),
                    "function.input.source",
                    id,
                    Sink::Message,
                );
                check(
                    input.target.as_json(),
                    "function.input.target",
                    id,
                    Sink::Message,
                );
            }
            FunctionConfig::PublishJson { input, .. }
            | FunctionConfig::PublishXml { input, .. } => {
                check(
                    input.source.as_json(),
                    "function.input.source",
                    id,
                    Sink::Message,
                );
                check(
                    input.target.as_json(),
                    "function.input.target",
                    id,
                    Sink::Message,
                );
                check(
                    input.root_element.as_json(),
                    "function.input.root_element",
                    id,
                    Sink::Message,
                );
            }
        }
    }
}

/// Check every expression in `workflow` for object keys the template-key
/// escape makes newly significant.
///
/// Three findings, and only the first is fatal:
///
/// - [`IssueCode::DuplicateTemplateKey`] — two keys in one object that collapse
///   to the same name after the escape is stripped. Always a bug, so
///   `Engine::build` refuses it.
/// - [`IssueCode::EscapedTemplateKey`] — a `$`-prefixed key, reported so a host
///   migrating to 3.9 can audit every place the escape changed what a template
///   emits. Informational: after migration these are deliberate.
///
/// A third check — flagging a single-key object whose key names no live
/// operator — was designed and then dropped, because it cannot be made
/// precise. In templating mode an unrecognised single key is *not* inert: it
/// evaluates its argument and emits a structured object, so
/// `{"result": {"var": "x"}}` yields `{"result": 5}`. That is the ordinary
/// single-key output template and the most common shape in a `map` mapping,
/// indistinguishable from a misspelled `lenght`. Flagging it would fire on
/// almost every correct workflow.
pub(crate) fn check_template_keys(workflow: &Workflow) -> Vec<WorkflowIssue> {
    let mut issues = Vec::new();
    for_each_expression(workflow, &mut |value, field, task_id, sink| {
        // A custom task's `input` is a config document, not an expression: only
        // the `Template` fields inside it are JSONLogic, and which those are is
        // the handler's business. Treating the whole document as a template
        // would flag ordinary config keys — and `DuplicateTemplateKey` is
        // fatal, so a false positive there refuses a valid workflow.
        if sink != Sink::Input {
            walk_template_keys(value, field, task_id, &mut issues);
        }
    });
    issues
}

/// The [`Severity::Rejected`] subset of [`check_template_keys`] — what
/// `Engine::build` refuses.
///
/// Filtered on the severity rather than on the one code that has it today, so a
/// template-key code added later is refused exactly when it is classified as a
/// rejection. A classification nothing reads is one that drifts.
pub(crate) fn refusing_template_key_issues(workflow: &Workflow) -> Vec<WorkflowIssue> {
    let mut issues = check_template_keys(workflow);
    issues.retain(|i| i.severity() == Severity::Rejected);
    issues
}

/// Recursive half of [`check_template_keys`].
fn walk_template_keys(
    value: &Value,
    path: &str,
    task_id: Option<&str>,
    issues: &mut Vec<WorkflowIssue>,
) {
    match value {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                walk_template_keys(item, &format!("{path}[{i}]"), task_id, issues);
            }
        }
        Value::Object(map) => {
            report_escaped_and_duplicate_keys(map, path, task_id, issues);
            for (key, child) in map {
                walk_template_keys(child, &format!("{path}.{key}"), task_id, issues);
            }
        }
        _ => {}
    }
}

/// The two key-level findings for one template object: escaped keys, and keys
/// that collide once the escape is stripped.
fn report_escaped_and_duplicate_keys(
    map: &serde_json::Map<String, Value>,
    path: &str,
    task_id: Option<&str>,
    issues: &mut Vec<WorkflowIssue>,
) {
    let mut emitted: HashMap<String, &str> = HashMap::new();
    for key in map.keys() {
        if let Some(stripped) = key.strip_prefix(TEMPLATE_KEY_ESCAPE) {
            issues.push(
                WorkflowIssue::at(
                    IssueCode::EscapedTemplateKey,
                    format!("{path}.{key}"),
                    format!(
                        "'{key}' is emitted as '{stripped}' — one \
                         '{TEMPLATE_KEY_ESCAPE}' is stripped from every template key. Double it \
                         to '{TEMPLATE_KEY_ESCAPE}{key}' to emit '{key}' itself"
                    ),
                )
                .with_step(task_id),
            );
        }
        // What this key actually emits, which is what can collide.
        let out = key
            .strip_prefix(TEMPLATE_KEY_ESCAPE)
            .unwrap_or(key)
            .to_string();
        if let Some(other) = emitted.insert(out.clone(), key) {
            issues.push(
                WorkflowIssue::at(
                    IssueCode::DuplicateTemplateKey,
                    format!("{path}.{key}"),
                    format!(
                        "'{other}' and '{key}' both emit the key '{out}', so this object would \
                         carry it twice — later reads see only the first while serialization \
                         emits both"
                    ),
                )
                .with_step(task_id),
            );
        }
    }
}

/// One expression's worth of [`check_secrets`].
fn check_expression(
    value: &Value,
    field: &str,
    task_id: Option<&str>,
    sink: Sink,
    secrets: &Secrets,
    issues: &mut Vec<WorkflowIssue>,
) {
    let mut refs = Vec::new();
    collect_secret_refs(value, field, &mut refs);
    if refs.is_empty() {
        return;
    }
    if sink.records() {
        issues.push(
            WorkflowIssue::at(
                IssueCode::SecretInMessageWrite,
                field,
                "reads a secret, and the engine records this expression's result — \
                 compute derived values in a custom handler instead",
            )
            .with_step(task_id),
        );
        return;
    }
    for (path, key) in &refs {
        let Some(key) = key else { continue };
        if secrets.get(key).is_some() {
            continue;
        }
        let at = if sink.points_at_reference() {
            path.as_str()
        } else {
            field
        };
        issues.push(
            WorkflowIssue::at(
                IssueCode::UnknownSecret,
                at,
                format!("secret '{key}' is not declared on the engine"),
            )
            .with_step(task_id),
        );
    }
}

/// Collect every `{"secret": …}` reference under `value` as
/// `(path, literal name)` — `None` for a dynamic name. Descends into the
/// argument too, since a dynamic name may itself contain a reference.
fn collect_secret_refs<'v>(value: &'v Value, path: &str, out: &mut Vec<(String, Option<&'v str>)>) {
    match value {
        Value::Object(map) => {
            if map.len() == 1 {
                if let Some(arg) = map.get(SECRET_OPERATOR) {
                    let literal = match arg {
                        Value::String(s) => Some(s.as_str()),
                        Value::Array(items) if items.len() == 1 => items[0].as_str(),
                        _ => None,
                    };
                    out.push((path.to_string(), literal));
                    collect_secret_refs(arg, &format!("{path}.{SECRET_OPERATOR}"), out);
                    return;
                }
            }
            for (key, child) in map {
                collect_secret_refs(child, &format!("{path}.{key}"), out);
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                collect_secret_refs(child, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
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
                // `"on_failure"` is the natural wrong guess once `halt_on`
                // exists — answer it rather than reporting a bare type error.
                let message = if terminal.as_str() == Some("on_failure") {
                    "terminal must be a boolean — for \"halt if this task failed\" \
                     use `\"halt_on\": \"failure\"`, which is the outcome axis"
                } else {
                    "terminal must be a boolean"
                };
                issues.push(
                    WorkflowIssue::at(
                        IssueCode::InvalidTerminal,
                        format!("{}.terminal", step.path),
                        message,
                    )
                    .with_step(id),
                );
            }
        }

        // Asking serde rather than keeping a list of accepted spellings, so a
        // future `HaltOn` variant needs no edit here.
        if let Some(halt_on) = step.node.get("halt_on") {
            let problem = if matches!(step.kind, StepKind::Group) {
                Some(
                    "halt_on is a per-task outcome rule and a group has no outcome \
                     of its own — put it on the task that can fail",
                )
            } else {
                HaltOn::deserialize(halt_on)
                    .err()
                    .map(|_| "halt_on must be one of \"never\", \"failure\"")
            };
            if let Some(message) = problem {
                issues.push(
                    WorkflowIssue::at(
                        IssueCode::InvalidHaltOn,
                        format!("{}.halt_on", step.path),
                        message,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every [`IssueCode`], for the checks below that must cover the whole
    /// vocabulary rather than a hand-picked subset. Kept complete by
    /// [`ordinal`] — see [`all_codes_lists_every_variant`].
    const ALL_CODES: [IssueCode; 27] = [
        IssueCode::EmptyWorkflowId,
        IssueCode::EmptyWorkflowName,
        IssueCode::NoTasks,
        IssueCode::MissingStepId,
        IssueCode::DuplicateStepId,
        IssueCode::EmptyGroup,
        IssueCode::GroupTooDeep,
        IssueCode::MissingFunction,
        IssueCode::InvalidFunctionName,
        IssueCode::InvalidTerminal,
        IssueCode::UnguardedValidation,
        IssueCode::InvalidHaltOn,
        IssueCode::GroupContinueOnError,
        IssueCode::LoopIncrementTooSmall,
        IssueCode::LoopBoundEmpty,
        IssueCode::LoopCounterInvalid,
        IssueCode::UnknownFunction,
        IssueCode::MissingHandler,
        IssueCode::InputParse,
        IssueCode::TemplateCompile,
        IssueCode::UnknownSecret,
        IssueCode::SecretInMessageWrite,
        IssueCode::InvalidSecretStore,
        IssueCode::DuplicateTemplateKey,
        IssueCode::EscapedTemplateKey,
        IssueCode::ParseFailed,
        IssueCode::ValidateFailed,
    ];

    /// A distinct index per variant, with **no wildcard arm**. Adding an
    /// [`IssueCode`] stops this compiling until it is given one.
    fn ordinal(code: IssueCode) -> usize {
        match code {
            IssueCode::EmptyWorkflowId => 0,
            IssueCode::EmptyWorkflowName => 1,
            IssueCode::NoTasks => 2,
            IssueCode::MissingStepId => 3,
            IssueCode::DuplicateStepId => 4,
            IssueCode::EmptyGroup => 5,
            IssueCode::GroupTooDeep => 6,
            IssueCode::MissingFunction => 7,
            IssueCode::InvalidFunctionName => 8,
            IssueCode::InvalidTerminal => 9,
            IssueCode::UnguardedValidation => 10,
            IssueCode::InvalidHaltOn => 11,
            IssueCode::GroupContinueOnError => 12,
            IssueCode::LoopIncrementTooSmall => 13,
            IssueCode::LoopBoundEmpty => 14,
            IssueCode::LoopCounterInvalid => 15,
            IssueCode::UnknownFunction => 16,
            IssueCode::MissingHandler => 17,
            IssueCode::InputParse => 18,
            IssueCode::TemplateCompile => 19,
            IssueCode::UnknownSecret => 20,
            IssueCode::SecretInMessageWrite => 21,
            IssueCode::InvalidSecretStore => 22,
            IssueCode::DuplicateTemplateKey => 23,
            IssueCode::EscapedTemplateKey => 24,
            IssueCode::ParseFailed => 25,
            IssueCode::ValidateFailed => 26,
        }
    }

    /// `ALL_CODES` is the whole vocabulary, and stays that way.
    ///
    /// Two halves, and both are needed. A new variant fails to compile at
    /// [`ordinal`]; giving it an index there but forgetting `ALL_CODES` breaks
    /// the bijection below. Without this the fix for a list that could only be
    /// wrong in one direction would itself ship such a list.
    #[test]
    fn all_codes_lists_every_variant() {
        let indices: HashSet<usize> = ALL_CODES.iter().copied().map(ordinal).collect();
        assert_eq!(
            indices,
            (0..ALL_CODES.len()).collect::<HashSet<_>>(),
            "ALL_CODES is not a bijection onto the variants `ordinal` knows"
        );
    }

    /// The severity table, stated as sets rather than read off the match.
    ///
    /// This is the tripwire the issue asks for: a code added in a later minor
    /// is classified by the compiler-forced match in [`IssueCode::severity`],
    /// but *promoting* one to advisory has to be a deliberate edit here.
    #[test]
    fn the_advisory_and_defect_sets_are_exactly_these() {
        let by = |want: Severity| -> HashSet<&'static str> {
            ALL_CODES
                .iter()
                .filter(|c| c.severity() == want)
                .map(IssueCode::as_str)
                .collect()
        };

        assert_eq!(
            by(Severity::Advisory),
            HashSet::from([
                "UNGUARDED_VALIDATION",
                "GROUP_CONTINUE_ON_ERROR",
                "ESCAPED_TEMPLATE_KEY",
            ]),
            "the advisory set is what a host screens on — changing it is a breaking change"
        );
        assert_eq!(
            by(Severity::Defect),
            HashSet::from(["MISSING_HANDLER"]),
            "only the config-only integrations build clean and then fail every message"
        );
        assert_eq!(
            by(Severity::Rejected).len(),
            ALL_CODES.len() - 4,
            "every remaining code is a rejection"
        );
    }

    /// The string forms are an API surface: unique, stable, and shouted.
    #[test]
    fn issue_codes_have_distinct_stable_strings() {
        let mut seen = HashSet::new();
        for code in ALL_CODES {
            assert!(seen.insert(code.as_str()), "duplicate string for {code:?}");
            assert_eq!(code.to_string(), code.as_str(), "Display matches as_str");
            assert!(
                code.as_str()
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == '_'),
                "{code:?} is not SCREAMING_SNAKE"
            );
        }
    }

    #[test]
    fn severities_have_distinct_stable_strings() {
        let all = [Severity::Rejected, Severity::Defect, Severity::Advisory];
        let mut seen = HashSet::new();
        for severity in all {
            assert!(seen.insert(severity.as_str()), "duplicate {severity:?}");
            assert_eq!(severity.to_string(), severity.as_str());
            assert!(
                severity
                    .as_str()
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == '_'),
                "{severity:?} is not SCREAMING_SNAKE"
            );
        }
    }

    /// The convenience method is the code's answer, not a second opinion.
    #[test]
    fn a_workflow_issue_reports_its_codes_severity() {
        for code in ALL_CODES {
            let issue = WorkflowIssue::at(code, "tasks[0]", "message");
            assert_eq!(issue.severity(), code.severity(), "{code:?}");
        }
    }
}
