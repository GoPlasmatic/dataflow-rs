use chrono::Utc;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Main error type for the dataflow engine
#[derive(Debug, Error, Clone, Serialize, Deserialize)]
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
        }
    }
}

/// Type alias for Result with DataflowError
pub type Result<T> = std::result::Result<T, DataflowError>;

/// Structured error information for error tracking in messages
#[derive(Debug, Clone, Serialize, Deserialize)]
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
            },
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
pub struct ErrorInfoBuilder {
    code: String,
    message: String,
    path: Option<String>,
    workflow_id: Option<String>,
    task_id: Option<String>,
    timestamp: Option<String>,
    retry_attempted: Option<bool>,
    retry_count: Option<u32>,
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
}
