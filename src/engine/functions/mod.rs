use crate::engine::error::Result;
use crate::engine::message::{Change, Message};
use async_trait::async_trait;
use serde_json::Value;
use std::cell::RefCell;

pub mod validation;
pub use validation::ValidationFunction;

pub mod http;
pub use http::*;

pub mod map;
pub use map::MapFunction;

// Thread-local DataLogic instance to avoid mutex contention
thread_local! {
    pub static FUNCTION_DATA_LOGIC: RefCell<datalogic_rs::DataLogic> = 
        RefCell::new(datalogic_rs::DataLogic::new());
}

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
            // Create validation function with thread-local DataLogic
            (
                VALIDATION_FUNCTION.to_string(),
                Box::new(ValidationFunction::new()),
            ),
            // Create map function with thread-local DataLogic
            (
                MAP_FUNCTION.to_string(),
                Box::new(MapFunction::new()),
            ),
            // Create HTTP function with 30-second timeout
            (HTTP_FUNCTION.to_string(), Box::new(HttpFunction::new(30))),
        ]
    }

    // Get all built-in async functions with their standard names
    // This is the same as get_all_functions but separated to maintain
    // API compatibility for AsyncEngine
    pub fn get_all_async_functions() -> Vec<(String, Box<dyn AsyncFunctionHandler + Send + Sync>)> {
        get_all_functions()
    }
}

/// Async interface for task functions that operate on messages
///
/// This trait defines how async task functions process a message with given
/// input parameters. It is particularly useful for IO-bound operations
/// like HTTP requests, file operations, and database queries.
#[async_trait]
pub trait AsyncFunctionHandler: Send + Sync {
    /// Execute the function asynchronously on a message with input parameters
    ///
    /// # Arguments
    ///
    /// * `message` - The message to process
    /// * `input` - Function input parameters
    ///
    /// # Returns
    ///
    /// * `Result<(usize, Vec<Change>)>` - Result containing status code and changes, or error
    async fn execute(&self, message: &mut Message, input: &Value) -> Result<(usize, Vec<Change>)>;
}
