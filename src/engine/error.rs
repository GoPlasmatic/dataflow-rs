use chrono::Utc;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Main error type for the dataflow engine
///
/// `#[non_exhaustive]`: a downstream `match` needs a wildcard arm. Adding it here
/// makes every *future* variant additive rather than a breaking change, which
/// matters because this enum is the crate's error channel and will keep growing.
#[derive(Debug, Error, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DataflowError {
    /// Validation errors occurring during rule evaluation
    #[error("Validation error: {0}")]
    Validation(String),

    /// Errors during function execution
    #[error("Function execution error: {context}")]
    FunctionExecution {
        context: String,
        #[source]
        #[serde(skip)]
        source: Option<Box<DataflowError>>,
    },

    /// Workflow-related errors
    #[error("Workflow error: {0}")]
    Workflow(String),

    /// Task-related errors
    #[error("Task error: {0}")]
    Task(String),

    /// Function not found errors
    #[error("Function not found: {0}")]
    FunctionNotFound(String),

    /// JSON serialization/deserialization errors
    #[error("Deserialization error: {0}")]
    Deserialization(String),

    /// I/O errors (file reading, etc.)
    #[error("IO error: {0}")]
    Io(String),

    /// JSONLogic/DataLogic evaluation errors
    #[error("Logic evaluation error: {0}")]
    LogicEvaluation(String),

    /// HTTP request errors
    #[error("HTTP error: {status} - {message}")]
    Http { status: u16, message: String },

    /// Timeout errors
    #[error("Timeout error: {0}")]
    Timeout(String),

    /// Any other errors
    #[error("Unknown error: {0}")]
    Unknown(String),

    /// A failure the service layer classifies itself.
    ///
    /// The engine never interprets `kind`: it carries it to [`ErrorInfo::code`]
    /// and otherwise treats this exactly like any other error —
    /// `continue_on_error`, the audit-trail entry and the `Result::Err`
    /// short-circuit are unchanged. No built-in returns this variant.
    ///
    /// `Display` renders `message` alone, so `to_string()` is always safe to hand
    /// to an untrusted caller. `detail` is reachable only through `Debug`,
    /// [`DataflowError::detail`] and [`ErrorInfo::detail`].
    ///
    /// Build it with [`DataflowError::service`].
    #[error("{message}")]
    Service {
        /// Stable, service-owned classification, e.g. `"circuit_open"`.
        kind: String,
        /// Caller-safe text.
        message: String,
        /// Operator-only text: logged and kept on the trace, not intended for an
        /// untrusted caller.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        /// Retryability, declared by the service rather than inferred from the
        /// variant.
        retryable: bool,
    },
}

impl DataflowError {
    /// Creates a new function execution error with context
    pub fn function_execution<S: Into<String>>(context: S, source: Option<DataflowError>) -> Self {
        DataflowError::FunctionExecution {
            context: context.into(),
            source: source.map(Box::new),
        }
    }

    /// Creates a new HTTP error
    pub fn http<S: Into<String>>(status: u16, message: S) -> Self {
        DataflowError::Http {
            status,
            message: message.into(),
        }
    }

    /// Convert from std::io::Error
    pub fn from_io(err: std::io::Error) -> Self {
        DataflowError::Io(err.to_string())
    }

    /// Convert from serde_json::Error
    pub fn from_serde(err: serde_json::Error) -> Self {
        DataflowError::Deserialization(err.to_string())
    }

    /// Determines if this error is retryable (worth retrying)
    ///
    /// Retryable errors are typically transient infrastructure failures that might succeed on retry.
    /// Non-retryable errors are typically data validation, logic, or configuration errors that
    /// will consistently fail on retry.
    pub fn retryable(&self) -> bool {
        match self {
            // Retryable errors - infrastructure/transient failures
            DataflowError::Http { status, .. } => {
                // Retry on server errors (5xx) and specific client errors that might be transient
                *status >= 500 || *status == 429 || *status == 408 || *status == 0
                // 0 means connection error
            }
            DataflowError::Timeout(_) => true,
            DataflowError::Io(_) => true,
            DataflowError::FunctionExecution { source, .. } => {
                // Inherit retryability from the source error if present
                source.as_ref().map(|e| e.retryable()).unwrap_or(false)
            }

            // Non-retryable errors - data/logic/configuration issues
            DataflowError::Validation(_) => false,
            DataflowError::LogicEvaluation(_) => false,
            DataflowError::Deserialization(_) => false,
            DataflowError::Workflow(_) => false,
            DataflowError::Task(_) => false,
            DataflowError::FunctionNotFound(_) => false,
            DataflowError::Unknown(_) => false,

            // Declared by the service rather than inferred from the variant.
            DataflowError::Service { retryable, .. } => *retryable,
        }
    }

    /// The service-owned classification, or `None` for every engine-owned
    /// variant.
    ///
    /// This is the whole point of [`DataflowError::Service`]: a handler can
    /// classify its own failures with a stable code the engine never interprets.
    pub fn kind(&self) -> Option<&str> {
        match self {
            DataflowError::Service { kind, .. } => Some(kind),
            _ => None,
        }
    }

    /// Operator-only detail, if this is a [`DataflowError::Service`] carrying one.
    ///
    /// Never included in `Display`, so `to_string()` stays safe to hand to an
    /// untrusted caller. Reachable through `Debug`, this method, and
    /// [`ErrorInfo::detail`].
    pub fn detail(&self) -> Option<&str> {
        match self {
            DataflowError::Service { detail, .. } => detail.as_deref(),
            _ => None,
        }
    }

    /// Start building a service-classified error.
    ///
    /// `kind` is the stable code the service will switch on; `message` is the
    /// caller-safe text. Defaults: no detail, `retryable: false`.
    ///
    /// ```
    /// use dataflow_rs::DataflowError;
    ///
    /// let e = DataflowError::service("circuit_open", "upstream unavailable")
    ///     .detail("connector 'billing' breaker open")
    ///     .retryable(true)
    ///     .build();
    ///
    /// assert_eq!(e.kind(), Some("circuit_open"));
    /// assert_eq!(e.detail(), Some("connector 'billing' breaker open"));
    /// assert!(e.retryable());
    /// // Display carries only the caller-safe text.
    /// assert_eq!(e.to_string(), "upstream unavailable");
    /// ```
    pub fn service(kind: impl Into<String>, message: impl Into<String>) -> ServiceErrorBuilder {
        ServiceErrorBuilder {
            kind: kind.into(),
            message: message.into(),
            detail: None,
            retryable: false,
        }
    }
}

/// Builder for [`DataflowError::Service`]. Mirrors [`ErrorInfoBuilder`].
#[must_use = "ServiceErrorBuilder must be `.build()` to produce a DataflowError"]
pub struct ServiceErrorBuilder {
    kind: String,
    message: String,
    detail: Option<String>,
    retryable: bool,
}

impl ServiceErrorBuilder {
    /// Attach operator-only detail — logged and kept on the trace, not intended
    /// for an untrusted caller.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Declare retryability. Defaults to `false`.
    ///
    /// Carriage for consumers: no engine code path acts on `retryable()`.
    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    pub fn build(self) -> DataflowError {
        DataflowError::Service {
            kind: self.kind,
            message: self.message,
            detail: self.detail,
            retryable: self.retryable,
        }
    }
}

/// Type alias for Result with DataflowError
pub type Result<T> = std::result::Result<T, DataflowError>;

/// Structured error information for error tracking in messages
///
/// `#[non_exhaustive]`: construct through [`ErrorInfo::builder`],
/// [`ErrorInfo::new`], [`ErrorInfo::simple`] or [`ErrorInfo::simple_ref`], which
/// are the documented paths. Field reads and `..` patterns are unaffected, and
/// future field additions stay non-breaking.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ErrorInfo {
    /// Error code (e.g., "WORKFLOW_ERROR", "TASK_ERROR", "VALIDATION_ERROR")
    pub code: String,

    /// Human-readable error message
    pub message: String,

    /// Optional path to the error location (e.g., "workflow.id", "task.id", "data.field")
    pub path: Option<String>,

    /// ID of the workflow where the error occurred (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,

    /// ID of the task where the error occurred (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,

    /// Timestamp when the error occurred
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,

    /// Whether a retry was attempted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_attempted: Option<bool>,

    /// Number of retries attempted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_count: Option<u32>,

    /// Operator-only detail lifted from [`DataflowError::Service`].
    ///
    /// Present on the persisted trace; a service must **not** surface it to an
    /// untrusted caller. `None` for every engine-owned error, and omitted from
    /// the serialized form when absent, so the JSON shape is unchanged unless a
    /// `Service` error carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Code recorded for a [`DataflowError::Service`] — also the historical
/// `TASK_ERROR` fallback for every other variant. Shared with
/// `workflow_executor`'s per-task error recording, which needs the identical
/// "Service contributes its own kind verbatim; everything else stays
/// `TASK_ERROR`" rule.
///
/// `kind` is passed through **verbatim** rather than upper-cased: the service owns
/// it and will switch on the recorded `code`, so making the two different strings
/// would force every consumer to know the transform. An empty `kind` falls back to
/// the historical `TASK_ERROR` so no `ErrorInfo` ever carries an empty code.
pub(crate) fn service_error_code(error: &DataflowError) -> String {
    match error.kind() {
        Some(kind) if !kind.is_empty() => kind.to_string(),
        _ => "TASK_ERROR".to_string(),
    }
}

impl ErrorInfo {
    /// Create a new error info entry with all fields
    pub fn new(workflow_id: Option<String>, task_id: Option<String>, error: DataflowError) -> Self {
        Self {
            code: match &error {
                DataflowError::Validation(_) => "VALIDATION_ERROR".to_string(),
                DataflowError::Workflow(_) => "WORKFLOW_ERROR".to_string(),
                DataflowError::Task(_) => "TASK_ERROR".to_string(),
                DataflowError::FunctionNotFound(_) => "FUNCTION_NOT_FOUND".to_string(),
                DataflowError::FunctionExecution { .. } => "FUNCTION_ERROR".to_string(),
                DataflowError::LogicEvaluation(_) => "LOGIC_ERROR".to_string(),
                DataflowError::Http { .. } => "HTTP_ERROR".to_string(),
                DataflowError::Timeout(_) => "TIMEOUT_ERROR".to_string(),
                DataflowError::Io(_) => "IO_ERROR".to_string(),
                DataflowError::Deserialization(_) => "DESERIALIZATION_ERROR".to_string(),
                DataflowError::Unknown(_) => "UNKNOWN_ERROR".to_string(),
                DataflowError::Service { .. } => service_error_code(&error),
            },
            detail: error.detail().map(str::to_string),
            message: error.to_string(),
            path: None,
            workflow_id,
            task_id,
            timestamp: Some(Utc::now().to_rfc3339()),
            retry_attempted: Some(false),
            retry_count: Some(0),
        }
    }

    /// Create a simple error info with just code, message, and optional path
    pub fn simple(code: String, message: String, path: Option<String>) -> Self {
        Self {
            code,
            message,
            path,
            workflow_id: None,
            task_id: None,
            timestamp: Some(Utc::now().to_rfc3339()),
            retry_attempted: None,
            retry_count: None,
            detail: None,
        }
    }

    /// Create a simple error info from references (avoids cloning when possible)
    pub fn simple_ref(code: &str, message: &str, path: Option<&str>) -> Self {
        Self {
            code: code.to_string(),
            message: message.to_string(),
            path: path.map(|s| s.to_string()),
            workflow_id: None,
            task_id: None,
            timestamp: Some(Utc::now().to_rfc3339()),
            retry_attempted: None,
            retry_count: None,
            detail: None,
        }
    }

    /// Mark that a retry was attempted
    pub fn with_retry(mut self) -> Self {
        self.retry_attempted = Some(true);
        self.retry_count = Some(self.retry_count.unwrap_or(0) + 1);
        self
    }

    /// Create a builder for ErrorInfo
    pub fn builder(code: impl Into<String>, message: impl Into<String>) -> ErrorInfoBuilder {
        ErrorInfoBuilder::new(code, message)
    }
}

/// Builder for creating ErrorInfo instances with a fluent API
#[must_use = "ErrorInfoBuilder must be `.build()` to produce an ErrorInfo"]
pub struct ErrorInfoBuilder {
    code: String,
    message: String,
    path: Option<String>,
    workflow_id: Option<String>,
    task_id: Option<String>,
    timestamp: Option<String>,
    retry_attempted: Option<bool>,
    retry_count: Option<u32>,
    detail: Option<String>,
}

impl ErrorInfoBuilder {
    /// Create a new ErrorInfoBuilder with required fields
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            path: None,
            workflow_id: None,
            task_id: None,
            timestamp: Some(Utc::now().to_rfc3339()),
            retry_attempted: None,
            retry_count: None,
            detail: None,
        }
    }

    /// Set the error path
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Set the workflow ID
    pub fn workflow_id(mut self, id: impl Into<String>) -> Self {
        self.workflow_id = Some(id.into());
        self
    }

    /// Set the task ID
    pub fn task_id(mut self, id: impl Into<String>) -> Self {
        self.task_id = Some(id.into());
        self
    }

    /// Set custom timestamp (defaults to now if not set)
    pub fn timestamp(mut self, timestamp: impl Into<String>) -> Self {
        self.timestamp = Some(timestamp.into());
        self
    }

    /// Mark as retry attempted
    pub fn retry_attempted(mut self, attempted: bool) -> Self {
        self.retry_attempted = Some(attempted);
        self
    }

    /// Set retry count
    pub fn retry_count(mut self, count: u32) -> Self {
        self.retry_count = Some(count);
        self
    }

    /// Attach operator-only detail. Not surfaced by `Display` anywhere; a service
    /// must not pass it to an untrusted caller.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Build the ErrorInfo instance
    pub fn build(self) -> ErrorInfo {
        ErrorInfo {
            code: self.code,
            message: self.message,
            path: self.path,
            workflow_id: self.workflow_id,
            task_id: self.task_id,
            timestamp: self.timestamp,
            retry_attempted: self.retry_attempted,
            retry_count: self.retry_count,
            detail: self.detail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retryable_errors() {
        // Test retryable errors
        assert!(
            DataflowError::Http {
                status: 500,
                message: "Internal Server Error".to_string()
            }
            .retryable()
        );
        assert!(
            DataflowError::Http {
                status: 502,
                message: "Bad Gateway".to_string()
            }
            .retryable()
        );
        assert!(
            DataflowError::Http {
                status: 503,
                message: "Service Unavailable".to_string()
            }
            .retryable()
        );
        assert!(
            DataflowError::Http {
                status: 429,
                message: "Too Many Requests".to_string()
            }
            .retryable()
        );
        assert!(
            DataflowError::Http {
                status: 408,
                message: "Request Timeout".to_string()
            }
            .retryable()
        );
        assert!(
            DataflowError::Http {
                status: 0,
                message: "Connection Error".to_string()
            }
            .retryable()
        );
        assert!(DataflowError::Timeout("Connection timeout".to_string()).retryable());
        assert!(DataflowError::Io("Network error".to_string()).retryable());
    }

    #[test]
    fn test_non_retryable_errors() {
        // Test non-retryable errors
        assert!(
            !DataflowError::Http {
                status: 400,
                message: "Bad Request".to_string()
            }
            .retryable()
        );
        assert!(
            !DataflowError::Http {
                status: 401,
                message: "Unauthorized".to_string()
            }
            .retryable()
        );
        assert!(
            !DataflowError::Http {
                status: 403,
                message: "Forbidden".to_string()
            }
            .retryable()
        );
        assert!(
            !DataflowError::Http {
                status: 404,
                message: "Not Found".to_string()
            }
            .retryable()
        );
        assert!(!DataflowError::Validation("Invalid input".to_string()).retryable());
        assert!(!DataflowError::LogicEvaluation("Invalid logic".to_string()).retryable());
        assert!(!DataflowError::Deserialization("Invalid JSON".to_string()).retryable());
        assert!(!DataflowError::Workflow("Invalid workflow".to_string()).retryable());
        assert!(!DataflowError::Unknown("Unknown error".to_string()).retryable());
    }

    #[test]
    fn test_function_execution_error_retryability() {
        // Test that function execution errors inherit retryability from source
        let retryable_source = DataflowError::Http {
            status: 500,
            message: "Server Error".to_string(),
        };
        let non_retryable_source = DataflowError::Validation("Invalid data".to_string());

        let retryable_func_error =
            DataflowError::function_execution("HTTP call failed", Some(retryable_source));
        let non_retryable_func_error =
            DataflowError::function_execution("Validation failed", Some(non_retryable_source));
        let no_source_func_error = DataflowError::function_execution("Unknown failure", None);

        assert!(retryable_func_error.retryable());
        assert!(!non_retryable_func_error.retryable());
        assert!(!no_source_func_error.retryable());
    }

    #[test]
    fn test_error_info_builder() {
        // Test basic builder
        let error = ErrorInfo::builder("TEST_ERROR", "Test message").build();
        assert_eq!(error.code, "TEST_ERROR");
        assert_eq!(error.message, "Test message");
        assert!(error.timestamp.is_some());
        assert!(error.path.is_none());

        // Test full builder
        let error = ErrorInfo::builder("VALIDATION_ERROR", "Field validation failed")
            .path("data.email")
            .workflow_id("workflow_1")
            .task_id("validate_email")
            .retry_attempted(true)
            .retry_count(2)
            .build();

        assert_eq!(error.code, "VALIDATION_ERROR");
        assert_eq!(error.message, "Field validation failed");
        assert_eq!(error.path, Some("data.email".to_string()));
        assert_eq!(error.workflow_id, Some("workflow_1".to_string()));
        assert_eq!(error.task_id, Some("validate_email".to_string()));
        assert_eq!(error.retry_attempted, Some(true));
        assert_eq!(error.retry_count, Some(2));
    }

    #[test]
    fn test_error_info_new_from_dataflow_error() {
        // Test ErrorInfo::new generates correct codes for each error type
        let test_cases = vec![
            (
                DataflowError::Validation("test".to_string()),
                "VALIDATION_ERROR",
            ),
            (
                DataflowError::Workflow("test".to_string()),
                "WORKFLOW_ERROR",
            ),
            (DataflowError::Task("test".to_string()), "TASK_ERROR"),
            (
                DataflowError::FunctionNotFound("test".to_string()),
                "FUNCTION_NOT_FOUND",
            ),
            (
                DataflowError::function_execution("test", None),
                "FUNCTION_ERROR",
            ),
            (
                DataflowError::LogicEvaluation("test".to_string()),
                "LOGIC_ERROR",
            ),
            (DataflowError::http(404, "Not Found"), "HTTP_ERROR"),
            (DataflowError::Timeout("test".to_string()), "TIMEOUT_ERROR"),
            (DataflowError::Io("test".to_string()), "IO_ERROR"),
            (
                DataflowError::Deserialization("test".to_string()),
                "DESERIALIZATION_ERROR",
            ),
            (DataflowError::Unknown("test".to_string()), "UNKNOWN_ERROR"),
        ];

        for (error, expected_code) in test_cases {
            let info = ErrorInfo::new(
                Some("workflow_1".to_string()),
                Some("task_1".to_string()),
                error,
            );
            assert_eq!(info.code, expected_code);
            assert_eq!(info.workflow_id, Some("workflow_1".to_string()));
            assert_eq!(info.task_id, Some("task_1".to_string()));
            assert!(info.timestamp.is_some());
            assert_eq!(info.retry_attempted, Some(false));
            assert_eq!(info.retry_count, Some(0));
        }
    }

    #[test]
    fn test_error_info_simple_constructors() {
        // Test simple constructor
        let error = ErrorInfo::simple(
            "CUSTOM_ERROR".to_string(),
            "Custom message".to_string(),
            Some("data.field".to_string()),
        );
        assert_eq!(error.code, "CUSTOM_ERROR");
        assert_eq!(error.message, "Custom message");
        assert_eq!(error.path, Some("data.field".to_string()));
        assert!(error.workflow_id.is_none());
        assert!(error.task_id.is_none());
        assert!(error.timestamp.is_some());

        // Test simple_ref constructor
        let error = ErrorInfo::simple_ref("REF_ERROR", "Ref message", Some("data.path"));
        assert_eq!(error.code, "REF_ERROR");
        assert_eq!(error.message, "Ref message");
        assert_eq!(error.path, Some("data.path".to_string()));

        // Test simple_ref with None path
        let error = ErrorInfo::simple_ref("NO_PATH", "No path message", None);
        assert!(error.path.is_none());
    }

    #[test]
    fn test_error_info_with_retry() {
        let error = ErrorInfo::simple_ref("TEST", "Test", None);
        assert!(error.retry_attempted.is_none());
        assert!(error.retry_count.is_none());

        let error = error.with_retry();
        assert_eq!(error.retry_attempted, Some(true));
        assert_eq!(error.retry_count, Some(1));

        let error = error.with_retry();
        assert_eq!(error.retry_attempted, Some(true));
        assert_eq!(error.retry_count, Some(2));
    }

    #[test]
    fn test_error_display_messages() {
        // Test that error display messages are correct
        assert_eq!(
            DataflowError::Validation("test".to_string()).to_string(),
            "Validation error: test"
        );
        assert_eq!(
            DataflowError::Workflow("test".to_string()).to_string(),
            "Workflow error: test"
        );
        assert_eq!(
            DataflowError::Task("test".to_string()).to_string(),
            "Task error: test"
        );
        assert_eq!(
            DataflowError::FunctionNotFound("test".to_string()).to_string(),
            "Function not found: test"
        );
        assert_eq!(
            DataflowError::http(404, "Not Found").to_string(),
            "HTTP error: 404 - Not Found"
        );
        assert_eq!(
            DataflowError::Timeout("test".to_string()).to_string(),
            "Timeout error: test"
        );
    }

    #[test]
    fn test_error_conversions() {
        // Test from_serde (we can't easily create a real serde error, but we can test the conversion works)
        let json_str = "invalid json";
        let serde_result: std::result::Result<serde_json::Value, _> =
            serde_json::from_str(json_str);
        if let Err(e) = serde_result {
            let dataflow_err = DataflowError::from_serde(e);
            assert!(matches!(dataflow_err, DataflowError::Deserialization(_)));
        }
    }

    #[test]
    fn service_error_carries_kind_detail_and_declared_retryability() {
        let e = DataflowError::service("circuit_open", "upstream unavailable")
            .detail("connector 'billing' breaker open")
            .retryable(true)
            .build();

        assert_eq!(e.kind(), Some("circuit_open"));
        assert_eq!(e.detail(), Some("connector 'billing' breaker open"));
        assert!(e.retryable());
    }

    #[test]
    fn service_display_hides_the_detail_but_debug_shows_it() {
        let e = DataflowError::service("circuit_open", "upstream unavailable")
            .detail("SECRET-TOPOLOGY")
            .build();

        // `to_string()` is always safe to hand to an untrusted caller.
        assert_eq!(e.to_string(), "upstream unavailable");
        assert!(!e.to_string().contains("SECRET-TOPOLOGY"));
        // The operator channel is reachable through Debug.
        assert!(format!("{e:?}").contains("SECRET-TOPOLOGY"));
    }

    #[test]
    fn service_retryability_is_independent_of_every_other_field() {
        let yes = DataflowError::service("k", "m").retryable(true).build();
        let no = DataflowError::service("k", "m").retryable(false).build();
        assert!(yes.retryable());
        assert!(!no.retryable());
        // Defaults to false.
        assert!(!DataflowError::service("k", "m").build().retryable());
    }

    #[test]
    fn kind_and_detail_are_none_for_every_engine_owned_variant() {
        let variants = [
            DataflowError::Validation("v".into()),
            DataflowError::FunctionExecution {
                context: "c".into(),
                source: None,
            },
            DataflowError::LogicEvaluation("l".into()),
            DataflowError::Deserialization("d".into()),
            DataflowError::Workflow("w".into()),
            DataflowError::Task("t".into()),
            DataflowError::FunctionNotFound("f".into()),
            DataflowError::Http {
                status: 500,
                message: "h".into(),
            },
            DataflowError::Timeout("to".into()),
            DataflowError::Io("io".into()),
            DataflowError::Unknown("u".into()),
        ];
        assert_eq!(variants.len(), 11, "one assertion per engine-owned variant");
        for v in &variants {
            assert_eq!(v.kind(), None, "kind() for {v:?}");
            assert_eq!(v.detail(), None, "detail() for {v:?}");
        }
    }

    #[test]
    fn service_error_code_passes_kind_through_verbatim() {
        // The recorded decision: verbatim, not upper-cased. A service switching
        // on the recorded `code` should not have to know a transform.
        let e = DataflowError::service("circuit_open", "m").build();
        let info = ErrorInfo::new(None, None, e);
        assert_eq!(info.code, "circuit_open");
    }

    #[test]
    fn an_empty_kind_falls_back_rather_than_recording_an_empty_code() {
        let e = DataflowError::service("", "m").build();
        let info = ErrorInfo::new(None, None, e);
        assert_eq!(info.code, "TASK_ERROR");
        assert!(!info.code.is_empty());
    }

    #[test]
    fn a_non_ascii_kind_is_neither_panicked_on_nor_mangled() {
        let e = DataflowError::service("limite_dépassé", "m").build();
        let info = ErrorInfo::new(None, None, e);
        // Verbatim, so the exact string round-trips including the accent.
        assert_eq!(info.code, "limite_dépassé");
    }

    #[test]
    fn error_info_lifts_the_detail_and_omits_it_when_absent() {
        let with = ErrorInfo::new(
            None,
            None,
            DataflowError::service("k", "m").detail("op only").build(),
        );
        assert_eq!(with.detail.as_deref(), Some("op only"));
        assert!(serde_json::to_string(&with).unwrap().contains("detail"));

        // Absent on a Service without one, and on every engine-owned variant.
        let without = ErrorInfo::new(None, None, DataflowError::service("k", "m").build());
        assert_eq!(without.detail, None);
        assert!(!serde_json::to_string(&without).unwrap().contains("detail"));

        let engine_owned = ErrorInfo::new(None, None, DataflowError::Task("t".into()));
        assert_eq!(engine_owned.detail, None);
        assert!(
            !serde_json::to_string(&engine_owned)
                .unwrap()
                .contains("detail"),
            "the JSON shape is unchanged for every pre-existing error"
        );
    }

    #[test]
    fn a_service_error_round_trips_through_serde_with_and_without_detail() {
        for e in [
            DataflowError::service("k", "m")
                .detail("d")
                .retryable(true)
                .build(),
            DataflowError::service("k", "m").build(),
        ] {
            let json = serde_json::to_string(&e).unwrap();
            let back: DataflowError = serde_json::from_str(&json).unwrap();
            assert_eq!(back.kind(), e.kind());
            assert_eq!(back.detail(), e.detail());
            assert_eq!(back.retryable(), e.retryable());
        }

        // `detail: None` emits no key.
        let bare = DataflowError::service("k", "m").build();
        assert!(!serde_json::to_string(&bare).unwrap().contains("detail"));
    }
}
