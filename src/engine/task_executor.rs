//! # Task Execution Module
//!
//! Dispatches a single `Task` to its function implementation. Built-in
//! function variants of `FunctionConfig` are dispatched directly to the
//! config's `execute(...)` (sync) or to the registered async handler under
//! the matching name (async).

use crate::engine::error::{DataflowError, Result};
use crate::engine::functions::{AsyncFunctionHandler, FunctionConfig};
use crate::engine::message::{Change, Message};
use crate::engine::task::Task;
use datalogic_rs::Engine;
use log::{debug, error};
use std::collections::HashMap;
use std::sync::Arc;

/// Handles the execution of tasks with their associated functions
///
/// The `TaskExecutor` is responsible for:
/// - Dispatching built-in functions (map, validation, …) directly to their config
/// - Routing async functions (http_call, enrich, publish_kafka, custom) to
///   the matching registered `AsyncFunctionHandler`
/// - Owning the function registry
pub struct TaskExecutor {
    /// Registry of async function handlers
    task_functions: Arc<HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>>>,
    /// Shared datalogic Engine (Send + Sync; Arc-shared across tasks)
    engine: Arc<Engine>,
}

impl TaskExecutor {
    /// Create a new TaskExecutor
    pub fn new(
        task_functions: Arc<HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>>>,
        engine: Arc<Engine>,
    ) -> Self {
        Self {
            task_functions,
            engine,
        }
    }

    /// Execute a single task. Built-in variants run on the calling thread;
    /// async variants are awaited.
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
            FunctionConfig::Map { input, .. } => input.execute(message, &self.engine),
            FunctionConfig::Validation { input, .. } => input.execute(message, &self.engine),
            FunctionConfig::ParseJson { input, .. } => {
                crate::engine::functions::parse::execute_parse_json(message, input)
            }
            FunctionConfig::ParseXml { input, .. } => {
                crate::engine::functions::parse::execute_parse_xml(message, input)
            }
            FunctionConfig::PublishJson { input, .. } => {
                crate::engine::functions::publish::execute_publish_json(message, input)
            }
            FunctionConfig::PublishXml { input, .. } => {
                crate::engine::functions::publish::execute_publish_xml(message, input)
            }
            FunctionConfig::Filter { input, .. } => input.execute(message, &self.engine),
            FunctionConfig::Log { input, .. } => input.execute(message, &self.engine),
            FunctionConfig::HttpCall { .. } => {
                self.execute_custom_function("http_call", message, &task.function)
                    .await
            }
            FunctionConfig::Enrich { .. } => {
                self.execute_custom_function("enrich", message, &task.function)
                    .await
            }
            FunctionConfig::PublishKafka { .. } => {
                self.execute_custom_function("publish_kafka", message, &task.function)
                    .await
            }
            FunctionConfig::Custom { name, .. } => {
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
                .execute(message, config, Arc::clone(&self.engine))
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
            | "publish_xml" | "filter" | "log" | "http_call" | "enrich" | "publish_kafka" => true,
            custom_name => self.task_functions.contains_key(custom_name),
        }
    }

    /// Get a clone of the task_functions Arc for reuse in new engines
    pub fn task_functions(
        &self,
    ) -> Arc<HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>>> {
        Arc::clone(&self.task_functions)
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
        let engine = LogicCompiler::new().into_engine();
        let task_executor = TaskExecutor::new(Arc::new(HashMap::new()), engine);

        // Built-in functions
        assert!(task_executor.has_function("map"));
        assert!(task_executor.has_function("validation"));
        assert!(task_executor.has_function("validate"));

        // Non-existent function
        assert!(!task_executor.has_function("nonexistent"));
    }

    #[test]
    fn test_custom_function_count() {
        let mut custom_functions = HashMap::new();
        custom_functions.insert(
            "custom_test".to_string(),
            Box::new(MockAsyncFunction) as Box<dyn AsyncFunctionHandler + Send + Sync>,
        );

        let engine = LogicCompiler::new().into_engine();
        let task_executor = TaskExecutor::new(Arc::new(custom_functions), engine);

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
            _engine: Arc<Engine>,
        ) -> Result<(usize, Vec<Change>)> {
            Ok((200, vec![]))
        }
    }
}
