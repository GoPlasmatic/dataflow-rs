//! The error type and the wire shape of a recorded error.

use super::AuthoredPath;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

mod error_code_serde {
    use super::{Cow, Deserialize, Deserializer, FromStr, Serializer};
    use datalogic_rs::ErrorCode;

    pub(super) fn serialize<S: Serializer>(code: &ErrorCode, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(code.as_str())
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<ErrorCode, D::Error> {
        let text = Cow::<str>::deserialize(d)?;
        ErrorCode::from_str(&text).map_err(serde::de::Error::custom)
    }
}

/// One step in a compiled expression's structural evaluation path, from
/// root to failing node.
///
/// Mirrors [`datalogic_rs::PathStep`] with `PartialEq + Eq` so a host can
/// persist, compare, or inspect where inside a rule an evaluation failed
/// without re-parsing a display string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ExprPathStep {
    /// Compile-time node id, matching [`datalogic_rs::Error::node_ids`].
    pub node_id: u32,
    /// Operator name at this node, when one applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    /// Zero-based position within the parent node's argument list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg_index: Option<u32>,
    /// JSONLogic-flavoured pointer from the root to this node (e.g. `"/if/0/>/0"`).
    pub json_pointer: String,
}

impl From<datalogic_rs::PathStep> for ExprPathStep {
    fn from(step: datalogic_rs::PathStep) -> Self {
        Self {
            node_id: step.node_id,
            operator: step.operator,
            arg_index: step.arg_index,
            json_pointer: step.json_pointer,
        }
    }
}

impl From<&datalogic_rs::PathStep> for ExprPathStep {
    fn from(step: &datalogic_rs::PathStep) -> Self {
        Self {
            node_id: step.node_id,
            operator: step.operator.clone(),
            arg_index: step.arg_index,
            json_pointer: step.json_pointer.clone(),
        }
    }
}

/// Structured budget exhaustion details from expression evaluation or run
/// task accounting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BudgetCause {
    /// The operation or task count when the ceiling was crossed.
    pub used: u64,
    /// The configured ceiling.
    pub limit: u64,
}

impl BudgetCause {
    /// Budget exhaustion where `used` crossed `limit`.
    pub const fn new(used: u64, limit: u64) -> Self {
        Self { used, limit }
    }
}

/// Structured diagnostic details for an expression compilation, evaluation
/// or budget failure.
///
/// Carried on [`Error::expr_cause`] and [`ErrorRecord::cause`] for host
/// diagnostics without changing the sanitized wire serialization of [`Error`]
/// or [`ErrorRecord`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ExprCause {
    /// Machine-stable error code from the expression engine.
    #[serde(with = "error_code_serde")]
    pub code: datalogic_rs::ErrorCode,
    /// The operator that raised the error, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    /// RFC 6901 JSON Pointer to the failing sub-expression in the authored
    /// rule, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expr_path: Option<String>,
    /// Structural steps from the root of the compiled expression to the
    /// failing node.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<ExprPathStep>,
    /// Budget exhaustion details when `code` is
    /// [`ErrorCode::BudgetExceeded`](datalogic_rs::ErrorCode::BudgetExceeded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetCause>,
    /// Operations spent by this evaluation before failing, when metered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops: Option<u64>,
}

impl ExprCause {
    /// Operations spent by this evaluation before failing, when metered.
    pub const fn ops_spent(&self) -> Option<u64> {
        self.ops
    }

    /// Extracts structured cause metadata from a [`datalogic_rs::Error`] and,
    /// when available, the compiled [`datalogic_rs::Logic`] it failed in.
    pub(crate) fn from_datalogic(
        e: &datalogic_rs::Error,
        logic: Option<&datalogic_rs::Logic>,
        ops: Option<u64>,
    ) -> Self {
        let (expr_path, steps) = match logic {
            Some(logic) => {
                let resolved = e.resolve_path(logic);
                let expr_path = resolved.last().map(|s| s.json_pointer.clone());
                let steps = resolved.into_iter().map(ExprPathStep::from).collect();
                (expr_path, steps)
            }
            None => (None, Vec::new()),
        };
        let budget = match e.kind {
            datalogic_rs::ErrorKind::BudgetExceeded { budget, spent } => {
                Some(BudgetCause::new(spent, budget))
            }
            _ => None,
        };
        Self {
            code: e.code(),
            operator: e.operator().map(str::to_owned),
            expr_path,
            steps,
            budget,
            ops: ops.or_else(|| budget.as_ref().map(|b| b.used)),
        }
    }

    /// Cause for an expression whose evaluated value did not convert to its
    /// target type `T`.
    pub(crate) fn type_mismatch(ops: Option<u64>) -> Self {
        Self {
            code: datalogic_rs::ErrorCode::TypeError,
            operator: None,
            expr_path: Some(String::new()),
            steps: Vec::new(),
            budget: None,
            ops,
        }
    }

    /// Cause for a run-level operation or task budget exceeded outside a
    /// single expression's own ceiling.
    pub(crate) fn budget_exceeded(used: u64, limit: u64, ops: Option<u64>) -> Self {
        Self {
            code: datalogic_rs::ErrorCode::BudgetExceeded,
            operator: None,
            expr_path: None,
            steps: Vec::new(),
            budget: Some(BudgetCause::new(used, limit)),
            ops,
        }
    }
}

/// Which workflow execution site produced an [`Error`] or [`ErrorRecord`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SiteKind {
    /// A workflow's top-level `condition`.
    WorkflowCondition,
    /// A task group's `condition`.
    GroupCondition,
    /// A task's `condition`.
    TaskCondition,
    /// A `filter` built-in's `condition`.
    FilterCondition,
    /// A `validation` built-in's rule `logic`.
    ValidationRule,
    /// A `validation` built-in's rule `message`.
    ValidationMessage,
    /// A `map` built-in's mapping destination `path`.
    MappingPath,
    /// A `map` built-in's mapping `logic`.
    MappingLogic,
    /// A workflow's `loop.over`.
    LoopOver,
    /// A task's `for_each.over`.
    ForEachOver,
    /// A task's function input or execution.
    Task,
}

/// Where in a workflow a runtime [`Error`] or [`ErrorRecord`] originated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ErrorSite {
    /// Which kind of site failed.
    pub kind: SiteKind,
    /// The workflow's id.
    pub workflow_id: Arc<str>,
    /// The task's id, when the site belongs to a task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<Arc<str>>,
    /// The group's id, when the site is a group condition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<Arc<str>>,
    /// Zero-based rule index when the site is a `validation` rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_index: Option<usize>,
    /// Zero-based mapping index when the site is a `map` mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mapping_index: Option<usize>,
    /// Target context path when the site is a `map` mapping with a known destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_path: Option<Arc<str>>,
    /// Where the site sits in the authored workflow definition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authored_path: Option<AuthoredPath>,
    /// The loop sweep counter, inside a workflow's loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_counter: Option<i64>,
}

impl ErrorSite {
    /// A site of `kind` in `workflow_id`.
    pub fn new(kind: SiteKind, workflow_id: Arc<str>) -> Self {
        Self {
            kind,
            workflow_id,
            task_id: None,
            group_id: None,
            rule_index: None,
            mapping_index: None,
            target_path: None,
            authored_path: None,
            loop_counter: None,
        }
    }

    /// Sets the task id on this site.
    #[must_use]
    pub fn with_task(mut self, task_id: Arc<str>) -> Self {
        self.task_id = Some(task_id);
        self
    }

    /// Sets the group id on this site.
    #[must_use]
    pub fn with_group(mut self, group_id: Arc<str>) -> Self {
        self.group_id = Some(group_id);
        self
    }

    /// Sets the authored path on this site.
    #[must_use]
    pub fn with_path(mut self, path: AuthoredPath) -> Self {
        self.authored_path = Some(path);
        self
    }

    /// Sets the loop sweep counter on this site.
    #[must_use]
    pub fn with_loop_counter(mut self, loop_counter: Option<i64>) -> Self {
        self.loop_counter = loop_counter;
        self
    }
}

/// Why a run, a compile or a function failed.
#[derive(Clone)]
pub struct Error {
    kind: ErrorKind,
    message: Cow<'static, str>,
    retryable: bool,
    source: Option<Arc<dyn std::error::Error + Send + Sync>>,
    expr_cause: Option<Arc<ExprCause>>,
    site: Option<Arc<ErrorSite>>,
}

/// The class of an [`Error`]. It decides the recorded code.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// A configuration is invalid.
    Config,
    /// An expression or definition did not compile.
    Compile,
    /// An expression failed to evaluate.
    Eval,
    /// The operation budget was exceeded.
    Budget,
    /// A deadline passed.
    TimedOut,
    /// The run was cancelled.
    Cancelled,
    /// A function reported its own code.
    Service {
        /// The code the function chose.
        code: Cow<'static, str>,
    },
}

/// The code of a task's own failure: a `Service` error with no code, and a
/// 5xx status's failure.
pub(crate) const TASK_ERROR: &str = "TASK_ERROR";
/// The record a 5xx status adds.
pub(crate) const TASK_STATUS_ERROR: &str = "TASK_STATUS_ERROR";
/// The record of a failed validation rule, and a built-in's refused input.
pub(crate) const VALIDATION_ERROR: &str = "VALIDATION_ERROR";
/// The record of a `map` mapping that failed, with its destination as the
/// record's `path`.
pub(crate) const MAPPING_ERROR: &str = "MAPPING_ERROR";
/// The record of a built-in's expression that failed to evaluate: a
/// `validation` rule, a `filter` condition.
pub(crate) const EVALUATION_ERROR: &str = "EVALUATION_ERROR";

impl Error {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            retryable: kind == ErrorKind::TimedOut,
            kind,
            message: message.into(),
            source: None,
            expr_cause: None,
            site: None,
        }
    }

    /// An error carrying a function's own code.
    pub fn service(
        code: impl Into<Cow<'static, str>>,
        message: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            kind: ErrorKind::Service { code: code.into() },
            message: message.into(),
            retryable: false,
            source: None,
            expr_cause: None,
            site: None,
        }
    }

    /// An error indicating an operation, task, or custom run budget was exceeded.
    pub fn budget(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorKind::Budget, message)
    }

    /// The class of the error.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// The human-readable message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Overrides whether a retry may succeed.
    #[must_use]
    pub fn with_retryable(mut self, yes: bool) -> Self {
        self.retryable = yes;
        self
    }

    /// Attaches the error that caused this one.
    #[must_use]
    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Arc::new(source));
        self
    }

    /// The underlying [`datalogic_rs::Error`], when this error came from
    /// expression compilation or evaluation.
    pub fn datalogic_error(&self) -> Option<&datalogic_rs::Error> {
        self.source
            .as_deref()
            .and_then(|s| s.downcast_ref::<datalogic_rs::Error>())
    }

    /// Attaches structured expression diagnostics to this error.
    #[must_use]
    pub fn with_expr_cause(mut self, cause: ExprCause) -> Self {
        self.expr_cause = Some(Arc::new(cause));
        self
    }

    /// Structured expression diagnostics, when this error came from an
    /// expression or a budget check.
    pub fn expr_cause(&self) -> Option<&ExprCause> {
        self.expr_cause.as_deref()
    }

    pub(crate) fn expr_cause_arc(&self) -> Option<Arc<ExprCause>> {
        self.expr_cause.clone()
    }

    /// Attaches the workflow site where this error happened.
    #[must_use]
    pub fn with_site(mut self, site: ErrorSite) -> Self {
        self.site = Some(Arc::new(site));
        self
    }

    /// Where in the workflow this error happened, when known.
    pub fn site(&self) -> Option<&ErrorSite> {
        self.site.as_deref()
    }

    pub(crate) fn site_arc(&self) -> Option<Arc<ErrorSite>> {
        self.site.clone()
    }

    /// May a retry succeed? A timeout says yes unless told otherwise; everything
    /// else says no.
    pub fn is_retryable(&self) -> bool {
        self.retryable
    }

    /// Prefixes the message with `context`, keeping kind, retryability,
    /// source, structured cause and workflow site.
    #[must_use]
    pub fn wrap(mut self, context: impl fmt::Display) -> Self {
        self.message = Cow::Owned(format!("{context}: {}", self.message));
        self
    }

    /// Replaces the human-readable message while keeping kind, retryability,
    /// source, structured cause and workflow site.
    pub(crate) fn with_message(mut self, message: impl Into<Cow<'static, str>>) -> Self {
        self.message = message.into();
        self
    }

    /// The recorded code for this error.
    pub fn code(&self) -> &str {
        match &self.kind {
            ErrorKind::Service { code } if !code.is_empty() => code,
            _ => self.code_static(),
        }
    }

    /// The static code for any non-service error, or for a service error with
    /// an empty code.
    pub(crate) fn code_static(&self) -> &'static str {
        match &self.kind {
            ErrorKind::Eval => "LOGIC_ERROR",
            ErrorKind::Budget => "BUDGET_EXCEEDED",
            ErrorKind::TimedOut => "TIMEOUT_ERROR",
            ErrorKind::Cancelled => "CANCELLED",
            ErrorKind::Config => "CONFIG_ERROR",
            ErrorKind::Compile => "COMPILATION_ERROR",
            ErrorKind::Service { .. } => TASK_ERROR,
        }
    }

    /// The text of this error's record: what v3's `Display` prints for the
    /// variant `kind` corresponds to, which the recorder wraps as
    /// "Task {id} error: {text}". A kind v3 has no variant for, and a service
    /// error (v3's display is the bare message), give the message alone.
    pub(crate) fn record_text(&self) -> Cow<'_, str> {
        let prefix = match &self.kind {
            ErrorKind::Eval => "Logic evaluation error: ",
            ErrorKind::TimedOut => "Timeout error: ",
            ErrorKind::Budget
            | ErrorKind::Config
            | ErrorKind::Compile
            | ErrorKind::Cancelled
            | ErrorKind::Service { .. } => return Cow::Borrowed(&self.message),
        };
        Cow::Owned(format!("{prefix}{}", self.message))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("Error");
        s.field("kind", &self.kind)
            .field("message", &self.message)
            .field("retryable", &self.is_retryable())
            .field("source", &self.source);
        if let Some(cause) = &self.expr_cause {
            s.field("expr_cause", cause);
        }
        if let Some(site) = &self.site {
            s.field("site", site);
        }
        s.finish_non_exhaustive()
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref().map(|s| s as _)
    }
}

/// The result of a fallible v4 operation.
pub type Result<T> = std::result::Result<T, Error>;

/// One error record, written as v3 wrote one: the same keys, in the same
/// order, each optional one absent when unset. The engine stamps every
/// record a task adds with the workflow's and the task's ids and the run's
/// timestamp, keeping any the record already carries; a workflow's own
/// failure carries no task id.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[non_exhaustive]
pub struct ErrorRecord {
    /// The error code.
    pub code: String,
    /// The human-readable message.
    pub message: String,
    /// Where the error happened; `null` when not known, and never absent.
    pub path: Option<String>,
    /// The workflow it happened in, shared with the program's own id and
    /// the audit entry's, so a run allocates it once.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub workflow_id: Option<Arc<str>>,
    /// The task it happened in, shared as `workflow_id` is.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub task_id: Option<Arc<str>>,
    /// When it happened, shared with every record of the run.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub timestamp: Option<Arc<str>>,
    /// Operator-only detail; never to be shown to an untrusted caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub detail: Option<String>,
    /// The fan-out element the error came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub element_index: Option<usize>,
    /// Structured expression diagnostics, carried in-memory for host reports
    /// and omitted from the sanitized wire representation.
    #[serde(skip)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub cause: Option<Arc<ExprCause>>,
    /// Where in the workflow the error happened, carried in-memory for host
    /// reports and omitted from the sanitized wire representation.
    #[serde(skip)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub site: Option<Arc<ErrorSite>>,
}

impl PartialEq for ErrorRecord {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code
            && self.message == other.message
            && self.path == other.path
            && self.workflow_id == other.workflow_id
            && self.task_id == other.task_id
            && self.timestamp == other.timestamp
            && self.detail == other.detail
            && self.element_index == other.element_index
    }
}

impl Eq for ErrorRecord {}

impl ErrorRecord {
    /// A record with a code and a message and nothing else.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            path: None,
            workflow_id: None,
            task_id: None,
            timestamp: None,
            detail: None,
            element_index: None,
            cause: None,
            site: None,
        }
    }

    /// Structured expression diagnostics, when this record came from an
    /// expression or a budget failure.
    pub fn cause(&self) -> Option<&ExprCause> {
        self.cause.as_deref()
    }

    /// Attaches structured expression diagnostics to this record.
    #[must_use]
    pub fn with_cause(mut self, cause: ExprCause) -> Self {
        self.cause = Some(Arc::new(cause));
        self
    }

    /// Where in the workflow this error happened, when known.
    pub fn site(&self) -> Option<&ErrorSite> {
        self.site.as_deref()
    }

    /// Attaches the workflow site where this error happened.
    #[must_use]
    pub fn with_site(mut self, site: ErrorSite) -> Self {
        self.site = Some(Arc::new(site));
        self
    }

    /// Fills what the record leaves unset: the workflow's id, the task's id
    /// (`None` for a workflow's own failure), the run's timestamp `now`,
    /// and in a fan-out the element's index. Every record the engine keeps
    /// is stamped here, so a consumer can group records by task. The ids and
    /// the timestamp are the program's and the run's own `Arc<str>`, shared
    /// rather than copied.
    pub(crate) fn stamp(
        &mut self,
        workflow_id: &Arc<str>,
        task_id: Option<&Arc<str>>,
        now: &Arc<str>,
        element_index: Option<usize>,
    ) {
        if self.workflow_id.is_none() {
            self.workflow_id = Some(Arc::clone(workflow_id));
        }
        if self.task_id.is_none() {
            self.task_id = task_id.map(Arc::clone);
        }
        if self.timestamp.is_none() {
            self.timestamp = Some(Arc::clone(now));
        }
        if self.element_index.is_none() {
            self.element_index = element_index;
        }
    }
}

impl From<&Error> for ErrorRecord {
    fn from(error: &Error) -> Self {
        let mut record = Self::new(error.code(), error.message());
        record.cause = error.expr_cause_arc();
        record.site = error.site_arc();
        record
    }
}

/// Writes the error as `{"code", "message", "retryable"}`: [`Error::code`],
/// [`Error::message`] and [`Error::is_retryable`], in that order.
impl Serialize for Error {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("Error", 3)?;
        o.serialize_field("code", self.code())?;
        o.serialize_field("message", self.message())?;
        o.serialize_field("retryable", &self.is_retryable())?;
        o.end()
    }
}

// `DataflowError`, not `Error`: a TypeScript `Error` would shadow the
// global one wherever it is imported.
#[cfg(feature = "ts")]
crate::base::ts::ts_decl!(
    Error,
    "DataflowError",
    [
        " An error as JSON: its code, its message, and whether a retry may",
        " succeed.",
    ],
    |_cfg| {
        use crate::base::ts::{Prop, object};
        object(&[
            Prop::new("code", "string", &[" The recorded code."]),
            Prop::new("message", "string", &[" The human-readable message."]),
            Prop::new("retryable", "boolean", &[" May a retry succeed?"]),
        ])
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_text_prefixes_two_kinds_and_is_the_message_otherwise() {
        // The record texts are 3.x's, byte for byte.
        let m = "the message";
        for (kind, expected) in [
            (ErrorKind::Eval, "Logic evaluation error: the message"),
            (ErrorKind::TimedOut, "Timeout error: the message"),
            (ErrorKind::Budget, m),
            (ErrorKind::Service { code: "X".into() }, m),
            (ErrorKind::Config, m),
            (ErrorKind::Compile, m),
            (ErrorKind::Cancelled, m),
        ] {
            assert_eq!(
                Error::new(kind.clone(), m).record_text(),
                expected,
                "{kind:?}"
            );
        }
    }

    fn kinds() -> Vec<(ErrorKind, &'static str)> {
        vec![
            (ErrorKind::Eval, "LOGIC_ERROR"),
            (ErrorKind::Budget, "BUDGET_EXCEEDED"),
            (ErrorKind::TimedOut, "TIMEOUT_ERROR"),
            (ErrorKind::Cancelled, "CANCELLED"),
            (ErrorKind::Config, "CONFIG_ERROR"),
            (ErrorKind::Compile, "COMPILATION_ERROR"),
            (
                ErrorKind::Service {
                    code: "X_FAIL".into(),
                },
                "X_FAIL",
            ),
            (ErrorKind::Service { code: "".into() }, "TASK_ERROR"),
        ]
    }

    #[test]
    fn code_gives_each_row_of_the_table() {
        for (kind, code) in kinds() {
            assert_eq!(Error::new(kind, "m").code(), code);
        }
    }

    #[test]
    fn only_a_timeout_is_retryable_by_default() {
        for (kind, _) in kinds() {
            let timed_out = kind == ErrorKind::TimedOut;
            assert_eq!(Error::new(kind, "m").is_retryable(), timed_out);
        }
        assert!(Error::service("A", "m").with_retryable(true).is_retryable());
        assert!(
            !Error::new(ErrorKind::TimedOut, "m")
                .with_retryable(false)
                .is_retryable()
        );
    }

    #[test]
    fn wrap_keeps_kind_retryable_and_source() {
        let source = std::io::Error::other("disk");
        let error = Error::new(ErrorKind::Eval, "bad")
            .with_retryable(true)
            .with_source(source)
            .wrap("task t");
        assert_eq!(error.to_string(), "task t: bad");
        assert_eq!(error.kind(), &ErrorKind::Eval);
        assert!(error.is_retryable());
        let source = std::error::Error::source(&error).expect("source kept");
        assert_eq!(source.to_string(), "disk");
    }

    #[test]
    fn debug_shows_kind_message_and_retryable() {
        let text = format!("{:?}", Error::new(ErrorKind::Budget, "over"));
        assert!(text.contains("Budget") && text.contains("over") && text.contains("retryable"));
    }

    #[test]
    fn a_stored_record_with_3x_retry_keys_reads_and_writes_without_them() {
        let stored =
            r#"{"code":"X","message":"m","path":null,"retry_attempted":true,"retry_count":2}"#;
        let record: ErrorRecord = serde_json::from_str(stored).unwrap();
        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            r#"{"code":"X","message":"m","path":null}"#
        );
    }

    #[test]
    fn stamp_fills_only_what_the_record_leaves_unset() {
        let mut record = ErrorRecord::new("X", "m");
        record.task_id = Some(Arc::from("own"));
        record.stamp(
            &Arc::from("w"),
            Some(&Arc::from("t")),
            &Arc::from("now"),
            Some(3),
        );
        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            r#"{"code":"X","message":"m","path":null,"workflow_id":"w","task_id":"own","timestamp":"now","element_index":3}"#
        );
        let mut workflow_own = ErrorRecord::new("X", "m");
        workflow_own.stamp(&Arc::from("w"), None, &Arc::from("now"), None);
        assert_eq!(workflow_own.task_id, None);
    }

    #[test]
    fn a_record_from_an_error_carries_only_code_and_message() {
        let record = ErrorRecord::from(&Error::service("X", "boom"));
        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            r#"{"code":"X","message":"boom","path":null}"#
        );
    }
}
