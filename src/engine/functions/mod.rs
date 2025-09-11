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
/// Implement this trait for custom processing logic that may involve I/O operations.
/// The function receives:
/// - Mutable access to the message being processed
/// - Pre-parsed function configuration
/// - Reference to the DataLogic instance for JSONLogic evaluation
#[async_trait]
pub trait AsyncFunctionHandler: Send + Sync {
    /// Execute the function on a message with pre-parsed configuration
    ///
    /// # Arguments
    ///
    /// * `message` - The message to process
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

/// Legacy synchronous function handler trait (for backward compatibility)
///
/// ## Migration Note
///
/// This trait is maintained for backward compatibility. New implementations should
/// use `AsyncFunctionHandler` instead. Synchronous handlers can be wrapped using
/// the `SyncFunctionWrapper` adapter.
pub trait FunctionHandler: Send + Sync {
    /// Execute the function on a message with pre-parsed configuration
    ///
    /// # Arguments
    ///
    /// * `message` - The message to process
    /// * `config` - Pre-parsed function configuration
    /// * `datalogic` - Reference to DataLogic instance for JSONLogic evaluation
    ///
    /// # Returns
    ///
    /// * `Result<(usize, Vec<Change>)>` - Result containing status code and changes, or error
    fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
        datalogic: &DataLogic,
    ) -> Result<(usize, Vec<Change>)>;
}

/// Wrapper to adapt synchronous FunctionHandler to AsyncFunctionHandler
pub struct SyncFunctionWrapper {
    handler: Arc<Box<dyn FunctionHandler + Send + Sync>>,
}

impl SyncFunctionWrapper {
    pub fn new(handler: Box<dyn FunctionHandler + Send + Sync>) -> Self {
        Self {
            handler: Arc::new(handler),
        }
    }
}

#[async_trait]
impl AsyncFunctionHandler for SyncFunctionWrapper {
    async fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
        datalogic: Arc<DataLogic>,
    ) -> Result<(usize, Vec<Change>)> {
        // Clone what we need for the blocking task
        let message_clone = message.clone();
        let config_clone = config.clone();
        let datalogic_clone = Arc::clone(&datalogic);
        let handler_clone = Arc::clone(&self.handler);

        // Execute synchronous handler in a blocking task
        let (result, updated_message) = tokio::task::spawn_blocking(move || {
            let mut message_for_handler = message_clone;
            let result =
                handler_clone.execute(&mut message_for_handler, &config_clone, &datalogic_clone);
            (result, message_for_handler)
        })
        .await
        .map_err(|e| {
            crate::engine::error::DataflowError::Task(format!("Task join error: {}", e))
        })?;

        // Apply changes back to the original message
        *message = updated_message;
        result
    }
}
