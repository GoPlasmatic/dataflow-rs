//! # Task Execution Module
//!
//! This module handles the execution of individual tasks within workflows.
//! It provides a clean separation of concerns by isolating task execution logic
//! from the main engine orchestration.

use crate::engine::error::{DataflowError, Result};
use crate::engine::executor::InternalExecutor;
use crate::engine::functions::{AsyncFunctionHandler, FunctionConfig};
use crate::engine::message::{Change, Message};
use crate::engine::task::Task;
use datalogic_rs::DataLogic;
use log::{debug, error};
use std::collections::HashMap;
use std::sync::Arc;

/// Handles the execution of tasks with their associated functions
///
/// The `TaskExecutor` is responsible for:
/// - Executing built-in functions (map, validation)
/// - Executing custom async function handlers
/// - Managing function registry
/// - Handling task-level error recovery
pub struct TaskExecutor {
    /// Registry of async function handlers
    task_functions: Arc<HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>>>,
    /// Internal executor for built-in functions
    executor: Arc<InternalExecutor>,
    /// Shared DataLogic instance
    datalogic: Arc<DataLogic>,
}

impl TaskExecutor {
    /// Create a new TaskExecutor
    pub fn new(
        task_functions: Arc<HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>>>,
        executor: Arc<InternalExecutor>,
        datalogic: Arc<DataLogic>,
    ) -> Self {
        Self {
            task_functions,
            executor,
            datalogic,
        }
    }

    /// Execute a single task
    ///
    /// This method:
    /// 1. Determines the function type (built-in or custom)
    /// 2. Executes the appropriate handler
    /// 3. Returns the status code and changes for audit trail
    ///
    /// # Arguments
    /// * `task` - The task to execute
    /// * `message` - The message being processed
    ///
    /// # Returns
    /// * `Result<(usize, Vec<Change>)>` - Status code and changes, or error
    pub async fn execute(
        &self,
        task: &Task,
        message: &mut Message,
    ) -> Result<(usize, Vec<Change>)> {
        debug!(
            "Executing task: {} with function: {:?}",
            task.id,
            task.function.function_name()
        );

        match &task.function {
            FunctionConfig::Map { input, .. } => {
                // Execute built-in map function
                self.executor.execute_map(message, input)
            }
            FunctionConfig::Validation { input, .. } => {
                // Execute built-in validation function
                self.executor.execute_validation(message, input)
            }
            FunctionConfig::ParseJson { input, .. } => {
                // Execute built-in parse_json function
                crate::engine::functions::parse::execute_parse_json(message, input)
            }
            FunctionConfig::ParseXml { input, .. } => {
                // Execute built-in parse_xml function
                crate::engine::functions::parse::execute_parse_xml(message, input)
            }
            FunctionConfig::PublishJson { input, .. } => {
                // Execute built-in publish_json function
                crate::engine::functions::publish::execute_publish_json(message, input)
            }
            FunctionConfig::PublishXml { input, .. } => {
                // Execute built-in publish_xml function
                crate::engine::functions::publish::execute_publish_xml(message, input)
            }
            FunctionConfig::Custom { name, .. } => {
                // Execute custom function handler
                self.execute_custom_function(name, message, &task.function)
                    .await
            }
        }
    }

    /// Execute a custom function handler
    async fn execute_custom_function(
        &self,
        name: &str,
        message: &mut Message,
        config: &FunctionConfig,
    ) -> Result<(usize, Vec<Change>)> {
        if let Some(handler) = self.task_functions.get(name) {
            handler
                .execute(message, config, Arc::clone(&self.datalogic))
                .await
        } else {
            error!("Function handler not found: {}", name);
            Err(DataflowError::FunctionNotFound(name.to_string()))
        }
    }

    /// Check if a function handler exists
    pub fn has_function(&self, name: &str) -> bool {
        match name {
            "map" | "validation" | "validate" | "parse_json" | "parse_xml" | "publish_json"
            | "publish_xml" => true,
            custom_name => self.task_functions.contains_key(custom_name),
        }
    }

    /// Get the count of registered custom functions
    pub fn custom_function_count(&self) -> usize {
        self.task_functions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::compiler::LogicCompiler;

    #[test]
    fn test_has_function() {
        let compiler = LogicCompiler::new();
        let (datalogic, logic_cache) = compiler.into_parts();
        let executor = Arc::new(InternalExecutor::new(datalogic.clone(), logic_cache));
        let task_executor = TaskExecutor::new(Arc::new(HashMap::new()), executor, datalogic);

        // Test built-in functions
        assert!(task_executor.has_function("map"));
        assert!(task_executor.has_function("validation"));
        assert!(task_executor.has_function("validate"));

        // Test non-existent function
        assert!(!task_executor.has_function("nonexistent"));
    }

    #[test]
    fn test_custom_function_count() {
        let mut custom_functions = HashMap::new();
        // Add a dummy custom function (we'll use a mock for testing)
        custom_functions.insert(
            "custom_test".to_string(),
            Box::new(MockAsyncFunction) as Box<dyn AsyncFunctionHandler + Send + Sync>,
        );

        let compiler = LogicCompiler::new();
        let (datalogic, logic_cache) = compiler.into_parts();
        let executor = Arc::new(InternalExecutor::new(datalogic.clone(), logic_cache));
        let task_executor = TaskExecutor::new(Arc::new(custom_functions), executor, datalogic);

        assert_eq!(task_executor.custom_function_count(), 1);
    }

    // Mock async function for testing
    struct MockAsyncFunction;

    #[async_trait::async_trait]
    impl AsyncFunctionHandler for MockAsyncFunction {
        async fn execute(
            &self,
            _message: &mut Message,
            _config: &FunctionConfig,
            _datalogic: Arc<DataLogic>,
        ) -> Result<(usize, Vec<Change>)> {
            Ok((200, vec![]))
        }
    }
}
