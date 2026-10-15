//! The error type and the wire shape of a recorded error.

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use std::borrow::Cow;
use std::fmt;

/// Why a run, a compile or a function failed.
pub struct Error {
    kind: ErrorKind,
    message: Cow<'static, str>,
    retryable: bool,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
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
    /// No function is registered under the name.
    FunctionNotFound,
    /// A document or payload did not parse.
    Parse,
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

impl Error {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            retryable: kind == ErrorKind::TimedOut,
            kind,
            message: message.into(),
            source: None,
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
        }
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
        self.source = Some(Box::new(source));
        self
    }

    /// May a retry succeed? A timeout says yes unless told otherwise; everything
    /// else says no.
    pub fn is_retryable(&self) -> bool {
        self.retryable
    }

    /// Prefixes the message with `context`, keeping kind, retryability and source.
    #[must_use]
    pub fn wrap(mut self, context: impl fmt::Display) -> Self {
        self.message = Cow::Owned(format!("{context}: {}", self.message));
        self
    }

    /// The recorded code for this error.
    pub fn code(&self) -> &str {
        match &self.kind {
            ErrorKind::Eval => "LOGIC_ERROR",
            ErrorKind::Budget => "BUDGET_EXCEEDED",
            ErrorKind::FunctionNotFound => "FUNCTION_NOT_FOUND",
            ErrorKind::Parse => "DESERIALIZATION_ERROR",
            ErrorKind::TimedOut => "TIMEOUT_ERROR",
            ErrorKind::Cancelled => "CANCELLED",
            ErrorKind::Config => "CONFIG_ERROR",
            ErrorKind::Compile => "COMPILATION_ERROR",
            ErrorKind::Service { code } if code.is_empty() => "TASK_ERROR",
            ErrorKind::Service { code } => code,
        }
    }
}

impl Error {
    /// The text of this error's record: what v3's `Display` prints for the
    /// variant `kind` corresponds to (I77), which the recorder wraps as
    /// "Task {id} error: {text}". A kind v3 has no variant for, and a service
    /// error (v3's display is the bare message), give the message alone.
    pub(crate) fn record_text(&self) -> Cow<'_, str> {
        let prefix = match &self.kind {
            ErrorKind::Eval => "Logic evaluation error: ",
            ErrorKind::Parse => "Deserialization error: ",
            ErrorKind::TimedOut => "Timeout error: ",
            ErrorKind::FunctionNotFound => "Function not found: ",
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
        f.debug_struct("Error")
            .field("kind", &self.kind)
            .field("message", &self.message)
            .field("retryable", &self.is_retryable())
            .finish()
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref().map(|s| s as _)
    }
}

/// The result of a fallible v4 operation.
pub type Result<T> = std::result::Result<T, Error>;

/// v3's `ErrorInfo` on the wire: the same fields, in the same order, with the
/// same serde attributes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ErrorRecord {
    /// The error code.
    pub code: String,
    /// The human-readable message.
    pub message: String,
    /// Where the error happened, if known. Always serialized.
    pub path: Option<String>,
    /// The workflow it happened in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,
    /// The task it happened in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// When it happened.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// Whether a retry was attempted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_attempted: Option<bool>,
    /// How many retries were attempted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_count: Option<u32>,
    /// Operator-only detail; never to be shown to an untrusted caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The fan-out element the error came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_index: Option<usize>,
}

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
            retry_attempted: None,
            retry_count: None,
            detail: None,
            element_index: None,
        }
    }
}

impl From<&Error> for ErrorRecord {
    fn from(error: &Error) -> Self {
        Self::new(error.code(), error.message())
    }
}

/// Writes the error as `{"code", "message", "retryable"}`: [`Error::code`],
/// [`Error::message`] and [`Error::is_retryable`], in that order (I126).
impl Serialize for Error {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut o = s.serialize_struct("Error", 3)?;
        o.serialize_field("code", self.code())?;
        o.serialize_field("message", self.message())?;
        o.serialize_field("retryable", &self.is_retryable())?;
        o.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dataflow_rs_v3::DataflowError as V3;

    #[test]
    fn record_text_is_what_v3_displays_for_the_variant_it_corresponds_to() {
        let m = "the message";
        for (kind, v3) in [
            (ErrorKind::Eval, V3::LogicEvaluation(m.into())),
            (ErrorKind::Budget, V3::BudgetExceeded(m.into())),
            (ErrorKind::Parse, V3::Deserialization(m.into())),
            (ErrorKind::TimedOut, V3::Timeout(m.into())),
            (ErrorKind::FunctionNotFound, V3::FunctionNotFound(m.into())),
            (
                ErrorKind::Service { code: "X".into() },
                V3::service("X", m).build(),
            ),
        ] {
            let expected = v3.to_string();
            assert_eq!(
                Error::new(kind.clone(), m).record_text(),
                expected,
                "{kind:?}"
            );
        }
        // v3 has no variant for these: the message alone.
        for kind in [ErrorKind::Config, ErrorKind::Compile, ErrorKind::Cancelled] {
            assert_eq!(Error::new(kind, m).record_text(), m);
        }
    }

    fn kinds() -> Vec<(ErrorKind, &'static str)> {
        vec![
            (ErrorKind::Eval, "LOGIC_ERROR"),
            (ErrorKind::Budget, "BUDGET_EXCEEDED"),
            (ErrorKind::FunctionNotFound, "FUNCTION_NOT_FOUND"),
            (ErrorKind::Parse, "DESERIALIZATION_ERROR"),
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
    fn a_record_from_an_error_carries_only_code_and_message() {
        let record = ErrorRecord::from(&Error::service("X", "boom"));
        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            r#"{"code":"X","message":"boom","path":null}"#
        );
    }
}
