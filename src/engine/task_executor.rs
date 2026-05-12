//! # Task Execution Module
//!
//! Dispatches a single `Task` to its function implementation. Built-in sync
//! variants of `FunctionConfig` are dispatched in `workflow_executor`'s sync
//! stretch via [`FunctionConfig::try_execute_in_arena`]; this module owns
//! the async path — `HttpCall`, `Enrich`, `PublishKafka`, and `Custom` —
//! routed to the matching registered handler.

use crate::engine::error::{DataflowError, Result};
use crate::engine::functions::{BoxedFunctionHandler, FunctionConfig};
use crate::engine::message::{Change, Message};
use crate::engine::task::Task;
use crate::engine::task_context::TaskContext;
use crate::engine::task_outcome::TaskOutcome;
use datalogic_rs::Engine;
use log::{debug, error};
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

/// Handles the execution of tasks with their associated functions.
///
/// The `TaskExecutor` is responsible for:
/// - Routing async functions (http_call, enrich, publish_kafka, custom) to
///   the matching registered handler via [`crate::engine::functions::DynAsyncFunctionHandler`]
/// - Owning the function registry
///
/// Sync built-ins are *not* routed through `execute` — `workflow_executor`
/// calls [`FunctionConfig::try_execute_in_arena`] inside its sync stretch
/// for those, sharing one arena across consecutive sync tasks.
pub struct TaskExecutor {
    /// Registry of async function handlers
    task_functions: Arc<HashMap<String, BoxedFunctionHandler>>,
    /// Shared datalogic Engine (Send + Sync; Arc-shared across tasks)
    engine: Arc<Engine>,
}

impl TaskExecutor {
    /// Create a new TaskExecutor
    pub fn new(
        task_functions: Arc<HashMap<String, BoxedFunctionHandler>>,
        engine: Arc<Engine>,
    ) -> Self {
        Self {
            task_functions,
            engine,
        }
    }

    /// Execute a single task. Sync built-ins reach here only when called from
    /// outside the workflow executor's sync-stretch path — they fall back to
    /// their `execute()` methods (which open a fresh thread-local arena).
    pub async fn execute(
        &self,
        task: &Task,
        message: &mut Message,
    ) -> Result<(TaskOutcome, Vec<Change>)> {
        debug!(
            "Executing task: {} with function: {:?}",
            task.id,
            task.function.function_name()
        );

        match &task.function {
            // Sync built-ins — only hit here when called outside the workflow
            // sync stretch (test harness, direct `TaskExecutor::execute`).
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
            // Async / user-registered handlers
            FunctionConfig::HttpCall { input, .. } => {
                self.dispatch_handler("http_call", message, input).await
            }
            FunctionConfig::Enrich { input, .. } => {
                self.dispatch_handler("enrich", message, input).await
            }
            FunctionConfig::PublishKafka { input, .. } => {
                self.dispatch_handler("publish_kafka", message, input).await
            }
            FunctionConfig::Custom {
                name,
                compiled_input,
                ..
            } => {
                let any_input = compiled_input.as_ref().ok_or_else(|| {
                    DataflowError::Validation(format!(
                        "Custom function '{}' has no precompiled input — \
                         was the workflow built outside Engine::new?",
                        name
                    ))
                })?;
                self.dispatch_handler_any(name, message, any_input.as_any())
                    .await
            }
        }
    }

    /// Generic-Input flavour: takes any `T: Any + Send + Sync`, hands it to
    /// the registered handler as `&dyn Any`. Used by the built-in async
    /// dispatch (`HttpCallConfig`, `EnrichConfig`, `PublishKafkaConfig`)
    /// where the typed config is already on the `FunctionConfig` enum.
    async fn dispatch_handler<T>(
        &self,
        name: &str,
        message: &mut Message,
        input: &T,
    ) -> Result<(TaskOutcome, Vec<Change>)>
    where
        T: Any + Send + Sync,
    {
        let any_input: &(dyn Any + Send + Sync) = input;
        self.dispatch_handler_any(name, message, any_input).await
    }

    /// Inner dispatch: build a `TaskContext`, invoke the handler, drain the
    /// accumulated `Change` buffer.
    async fn dispatch_handler_any(
        &self,
        name: &str,
        message: &mut Message,
        any_input: &(dyn Any + Send + Sync),
    ) -> Result<(TaskOutcome, Vec<Change>)> {
        let handler = self.task_functions.get(name).ok_or_else(|| {
            error!("Function handler not found: {}", name);
            DataflowError::FunctionNotFound(name.to_string())
        })?;
        let mut ctx = TaskContext::new(message, &self.engine);
        let outcome = handler.dyn_execute(&mut ctx, any_input).await?;
        let changes = ctx.into_changes();
        Ok((outcome, changes))
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
    pub fn task_functions(&self) -> Arc<HashMap<String, BoxedFunctionHandler>> {
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
    use crate::engine::AsyncFunctionHandler;
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
        let mut custom_functions: HashMap<String, BoxedFunctionHandler> = HashMap::new();
        custom_functions.insert("custom_test".to_string(), Box::new(MockAsyncFunction));

        let engine = LogicCompiler::new().into_engine();
        let task_executor = TaskExecutor::new(Arc::new(custom_functions), engine);

        assert_eq!(task_executor.custom_function_count(), 1);
    }

    // Mock async function for testing
    struct MockAsyncFunction;

    #[async_trait::async_trait]
    impl AsyncFunctionHandler for MockAsyncFunction {
        type Input = serde_json::Value;

        async fn execute(
            &self,
            _ctx: &mut TaskContext<'_>,
            _input: &serde_json::Value,
        ) -> Result<TaskOutcome> {
            Ok(TaskOutcome::Success)
        }
    }
}
