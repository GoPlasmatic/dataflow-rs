use crate::engine::error::Result;
use crate::engine::message::{Change, Message};

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
    pub fn get_all_functions() -> Vec<(String, Box<dyn FunctionHandler + Send + Sync>)> {
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
}

/// Interface for task functions that operate on messages
///
/// ## Thread-Safety (v1.0)
///
/// Functions create local DataLogic instances as needed for JSONLogic evaluation,
/// ensuring thread-safety without locks or shared state.
///
/// ## Usage
///
/// Implement this trait for custom processing logic. The function receives:
/// - Mutable access to the message being processed
/// - Pre-parsed function configuration
pub trait FunctionHandler: Send + Sync {
    /// Execute the function on a message with pre-parsed configuration
    ///
    /// # Arguments
    ///
    /// * `message` - The message to process
    /// * `config` - Pre-parsed function configuration
    ///
    /// # Returns
    ///
    /// * `Result<(usize, Vec<Change>)>` - Result containing status code and changes, or error
    fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
    ) -> Result<(usize, Vec<Change>)>;
}
