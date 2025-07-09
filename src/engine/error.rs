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
            DataflowError::Unknown(_) => false,
        }
    }
}

/// Type alias for Result with DataflowError
pub type Result<T> = std::result::Result<T, DataflowError>;

/// Structured error information for error tracking in messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorInfo {
    /// ID of the workflow where the error occurred (if available)
    pub workflow_id: Option<String>,

    /// ID of the task where the error occurred (if available)
    pub task_id: Option<String>,

    /// Timestamp when the error occurred
    pub timestamp: String,

    /// The actual error
    pub error_message: String,

    /// Whether a retry was attempted
    pub retry_attempted: bool,

    /// Number of retries attempted
    pub retry_count: u32,
}

impl ErrorInfo {
    /// Create a new error info entry
    pub fn new(workflow_id: Option<String>, task_id: Option<String>, error: DataflowError) -> Self {
        Self {
            workflow_id,
            task_id,
            timestamp: Utc::now().to_rfc3339(),
            error_message: error.to_string(),
            retry_attempted: false,
            retry_count: 0,
        }
    }

    /// Mark that a retry was attempted
    pub fn with_retry(mut self) -> Self {
        self.retry_attempted = true;
        self.retry_count += 1;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retryable_errors() {
        // Test retryable errors
        assert!(DataflowError::Http {
            status: 500,
            message: "Internal Server Error".to_string()
        }
        .retryable());
        assert!(DataflowError::Http {
            status: 502,
            message: "Bad Gateway".to_string()
        }
        .retryable());
        assert!(DataflowError::Http {
            status: 503,
            message: "Service Unavailable".to_string()
        }
        .retryable());
        assert!(DataflowError::Http {
            status: 429,
            message: "Too Many Requests".to_string()
        }
        .retryable());
        assert!(DataflowError::Http {
            status: 408,
            message: "Request Timeout".to_string()
        }
        .retryable());
        assert!(DataflowError::Http {
            status: 0,
            message: "Connection Error".to_string()
        }
        .retryable());
        assert!(DataflowError::Timeout("Connection timeout".to_string()).retryable());
        assert!(DataflowError::Io("Network error".to_string()).retryable());
    }

    #[test]
    fn test_non_retryable_errors() {
        // Test non-retryable errors
        assert!(!DataflowError::Http {
            status: 400,
            message: "Bad Request".to_string()
        }
        .retryable());
        assert!(!DataflowError::Http {
            status: 401,
            message: "Unauthorized".to_string()
        }
        .retryable());
        assert!(!DataflowError::Http {
            status: 403,
            message: "Forbidden".to_string()
        }
        .retryable());
        assert!(!DataflowError::Http {
            status: 404,
            message: "Not Found".to_string()
        }
        .retryable());
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
}
