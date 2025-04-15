use crate::engine::message::{Change, Message};
use serde_json::Value;

pub mod validation;
pub use validation::ValidationFunction;

pub mod fetch;
pub use fetch::*;

pub mod enrich;
pub use enrich::MapFunction;

// Re-export all built-in functions for easier access
pub mod builtins {
    use super::*;
    use datalogic_rs::DataLogic;
    use once_cell::sync::Lazy;
    use std::sync::{Arc, Mutex};

    // Create a global thread-safe DataLogic instance
    pub static DATA_LOGIC: Lazy<Arc<Mutex<DataLogic>>> =
        Lazy::new(|| Arc::new(Mutex::new(DataLogic::new())));

    // Standard function names used for registering built-ins
    pub const VALIDATION_FUNCTION: &str = "validate";
    pub const MAP_FUNCTION: &str = "map";
    pub const HTTP_FUNCTION: &str = "http";

    // Get all built-in functions with their standard names
    pub fn get_all_functions() -> Vec<(String, Box<dyn FunctionHandler>)> {
        vec![
            // Create validation function
            (
                VALIDATION_FUNCTION.to_string(),
                Box::new(ValidationFunction::new(DATA_LOGIC.clone())),
            ),
            // Create map function
            (
                MAP_FUNCTION.to_string(),
                Box::new(MapFunction::new_with_mutex(DATA_LOGIC.clone())),
            ),
            // Create HTTP function with 30-second timeout
            (
                HTTP_FUNCTION.to_string(),
                Box::new(fetch::HttpFunction::new(30)),
            ),
        ]
    }
}

/// Interface for task functions that operate on messages
///
/// All task functions implement this trait, which defines how they process
/// a message with given input parameters
pub trait FunctionHandler: Send + Sync {
    /// Execute the function on a message with input parameters
    ///
    /// # Arguments
    ///
    /// * `message` - The message to process
    /// * `input` - Function input parameters
    ///
    /// # Returns
    ///
    /// * `Result<(usize, Vec<Change>), String>` - Result containing status code and changes, or error
    fn execute(&self, message: &mut Message, input: &Value)
        -> Result<(usize, Vec<Change>), String>;
}
