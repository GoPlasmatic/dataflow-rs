use crate::engine::error::Result;
use crate::engine::message::{Change, Message};
use async_trait::async_trait;
use datalogic_rs::DataLogic;
use std::sync::Arc;

pub mod config;
pub use config::FunctionConfig;

pub mod validation;
pub use validation::{ValidationConfig, ValidationRule};

pub mod map;
pub use map::{MapConfig, MapMapping};

pub mod parse;
pub use parse::ParseConfig;

pub mod publish;
pub use publish::PublishConfig;

// Re-export all built-in functions for easier access
pub mod builtins {
    use super::*;

    // Get all built-in functions with their standard names
    pub fn get_all_functions() -> Vec<(String, Box<dyn AsyncFunctionHandler + Send + Sync>)> {
        // Map and Validate are now internal to the Engine for better performance
        // They can directly access compiled logic cache
        // Add other built-in functions here as needed (HTTP, File I/O, etc.)
        vec![]
    }
}

/// Async interface for task functions that operate on messages
///
/// ## Usage
///
/// Implement this trait for custom processing logic.
/// The function receives:
/// - Mutable access to the message being processed (no cloning needed!)
/// - Pre-parsed function configuration
/// - Reference to the DataLogic instance for JSONLogic evaluation
///
/// ## Performance Note
///
/// This trait works directly with `&mut Message` without any cloning.
/// The message is passed by mutable reference throughout the async execution,
/// ensuring zero-copy operation for optimal performance.
#[async_trait]
pub trait AsyncFunctionHandler: Send + Sync {
    /// Execute the function on a message with pre-parsed configuration
    ///
    /// # Arguments
    ///
    /// * `message` - The message to process (mutable reference, no cloning)
    /// * `config` - Pre-parsed function configuration
    /// * `datalogic` - Reference to DataLogic instance for JSONLogic evaluation
    ///
    /// # Returns
    ///
    /// * `Result<(usize, Vec<Change>)>` - Result containing status code and changes, or error
    async fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
        datalogic: Arc<DataLogic>,
    ) -> Result<(usize, Vec<Change>)>;
}
