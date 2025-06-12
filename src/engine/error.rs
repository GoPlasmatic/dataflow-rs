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
