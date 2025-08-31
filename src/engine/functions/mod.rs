use crate::engine::error::Result;
use crate::engine::message::{Change, Message};
use async_trait::async_trait;
use datalogic_rs::DataLogic;

pub mod config;
pub use config::FunctionConfig;

pub mod validation;
pub use validation::{ValidationConfig, ValidationFunction};

pub mod map;
pub use map::{MapConfig, MapFunction};

// Re-export all built-in functions for easier access
pub mod builtins {
    use super::*;

    // Standard function names used for registering built-ins
    pub const VALIDATION_FUNCTION: &str = "validate";
    pub const MAP_FUNCTION: &str = "map";
    pub const HTTP_FUNCTION: &str = "http";

    // Get all built-in functions with their standard names
    pub fn get_all_functions() -> Vec<(String, Box<dyn AsyncFunctionHandler + Send + Sync>)> {
        vec![
            // Create validation function
            (
                VALIDATION_FUNCTION.to_string(),
                Box::new(ValidationFunction::new()),
            ),
            // Create map function
            (MAP_FUNCTION.to_string(), Box::new(MapFunction::new())),
        ]
    }

    // Get all built-in async functions with their standard names
    // This is the same as get_all_functions but separated to maintain
    // API compatibility for AsyncEngine
    pub fn get_all_async_functions() -> Vec<(String, Box<dyn AsyncFunctionHandler + Send + Sync>)> {
        get_all_functions()
    }
}

/// Thread-safe async interface for task functions that operate on messages
///
/// ## Thread-Safety (v1.0)
///
/// Functions now receive a DataLogic instance as a parameter, enabling thread-safe
/// JSONLogic evaluation without internal locking. Each message gets exclusive access
/// to a DataLogic instance for its entire workflow execution.
///
/// ## Usage
///
/// Implement this trait for custom async processing logic. The function receives:
/// - Mutable access to the message being processed
/// - Pre-parsed function configuration
/// - A DataLogic instance for JSONLogic evaluation
///
/// Perfect for IO-bound operations like HTTP requests, database queries, and file operations.
#[async_trait]
pub trait AsyncFunctionHandler: Send + Sync {
    /// Execute the function asynchronously on a message with pre-parsed configuration
    ///
    /// # Arguments
    ///
    /// * `message` - The message to process
    /// * `config` - Pre-parsed function configuration
    /// * `data_logic` - DataLogic instance for JSONLogic evaluation
    ///
    /// # Returns
    ///
    /// * `Result<(usize, Vec<Change>)>` - Result containing status code and changes, or error
    async fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
        data_logic: &mut DataLogic,
    ) -> Result<(usize, Vec<Change>)>;
}
