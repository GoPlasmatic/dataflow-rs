//! # Workflow Compilation Module
//!
//! This module handles the pre-compilation of JSONLogic expressions used throughout
//! the engine. By compiling all logic at initialization time with DataLogic v4, we achieve:
//!
//! - Zero runtime compilation overhead
//! - Thread-safe compiled logic via Arc
//! - Early validation of logic expressions
//! - Efficient memory sharing across async tasks

use crate::engine::functions::integration::{EnrichConfig, HttpCallConfig, PublishKafkaConfig};
use crate::engine::functions::{FilterConfig, LogConfig, MapConfig, ValidationConfig};
use crate::engine::{FunctionConfig, Workflow};
use datalogic_rs::{CompiledLogic, DataLogic};
use log::{debug, error};
use serde_json::Value;
use std::sync::Arc;

/// Compiles and caches JSONLogic expressions for optimal runtime performance.
///
/// The `LogicCompiler` is responsible for:
/// - Pre-compiling all workflow conditions using DataLogic v4
/// - Pre-compiling task-specific logic (map transformations, validation rules)
/// - Maintaining Arc-wrapped compiled logic for thread-safe sharing
/// - Providing early validation of logic expressions
pub struct LogicCompiler {
    /// Shared DataLogic instance for compilation
    datalogic: Arc<DataLogic>,
    /// Cache of compiled logic expressions indexed by their position
    logic_cache: Vec<Arc<CompiledLogic>>,
}

impl Default for LogicCompiler {
    fn default() -> Self {
        Self::new()
    }
}

impl LogicCompiler {
    /// Create a new LogicCompiler with DataLogic v4
    pub fn new() -> Self {
        Self {
            datalogic: Arc::new(DataLogic::with_preserve_structure()),
            logic_cache: Vec::new(),
        }
    }

    /// Get the DataLogic instance
    pub fn datalogic(&self) -> Arc<DataLogic> {
        Arc::clone(&self.datalogic)
    }

    /// Get the logic cache
    pub fn logic_cache(&self) -> &Vec<Arc<CompiledLogic>> {
        &self.logic_cache
    }

    /// Consume the compiler and return its components
    pub fn into_parts(self) -> (Arc<DataLogic>, Vec<Arc<CompiledLogic>>) {
        (self.datalogic, self.logic_cache)
    }

    /// Compile all workflows and their tasks, returning them sorted by priority
    pub fn compile_workflows(&mut self, workflows: Vec<Workflow>) -> Vec<Workflow> {
        let mut compiled_workflows = Vec::new();

        for mut workflow in workflows {
            if let Err(e) = workflow.validate() {
                error!("Invalid workflow {}: {:?}", workflow.id, e);
                continue;
            }

            // Parse and cache workflow condition
            debug!(
                "Compiling condition for workflow {}: {:?}",
                workflow.id, workflow.condition
            );
            match self.compile_logic(&workflow.condition) {
                Ok(index) => {
                    workflow.condition_index = index;
                    debug!(
                        "Workflow {} condition compiled at index {:?}",
                        workflow.id, index
                    );

                    // Compile task conditions and function logic
                    self.compile_workflow_tasks(&mut workflow);

                    compiled_workflows.push(workflow);
                }
                Err(e) => {
                    error!(
                        "Failed to parse condition for workflow {}: {:?}",
                        workflow.id, e
                    );
                }
            }
        }

        // Sort by priority once at construction time
        compiled_workflows.sort_by_key(|w| w.priority);
        compiled_workflows
    }

    /// Compile task conditions and function logic for a workflow
    fn compile_workflow_tasks(&mut self, workflow: &mut Workflow) {
        for task in &mut workflow.tasks {
            // Compile task condition
            debug!(
                "Compiling condition for task {} in workflow {}: {:?}",
                task.id, workflow.id, task.condition
            );
            match self.compile_logic(&task.condition) {
                Ok(index) => {
                    task.condition_index = index;
                    debug!("Task {} condition compiled at index {:?}", task.id, index);
                }
                Err(e) => {
                    error!(
                        "Failed to parse condition for task {} in workflow {}: {:?}",
                        task.id, workflow.id, e
                    );
                }
            }

            // Compile function-specific logic (map transformations, validation rules)
            self.compile_function_logic(&mut task.function, &task.id, &workflow.id);
        }
    }

    /// Compile function-specific logic based on function type
    fn compile_function_logic(
        &mut self,
        function: &mut FunctionConfig,
        task_id: &str,
        workflow_id: &str,
    ) {
        match function {
            FunctionConfig::Map { input, .. } => {
                self.compile_map_logic(input, task_id, workflow_id);
            }
            FunctionConfig::Validation { input, .. } => {
                self.compile_validation_logic(input, task_id, workflow_id);
            }
            FunctionConfig::Filter { input, .. } => {
                self.compile_filter_logic(input, task_id, workflow_id);
            }
            FunctionConfig::Log { input, .. } => {
                self.compile_log_logic(input, task_id, workflow_id);
            }
            FunctionConfig::HttpCall { input, .. } => {
                self.compile_http_call_logic(input, task_id, workflow_id);
            }
            FunctionConfig::Enrich { input, .. } => {
                self.compile_enrich_logic(input, task_id, workflow_id);
            }
            FunctionConfig::PublishKafka { input, .. } => {
                self.compile_publish_kafka_logic(input, task_id, workflow_id);
            }
            _ => {
                // Custom and other functions don't need pre-compilation
            }
        }
    }

    /// Compile map transformation logic
    fn compile_map_logic(&mut self, config: &mut MapConfig, task_id: &str, workflow_id: &str) {
        for mapping in &mut config.mappings {
            debug!(
                "Compiling map logic for task {} in workflow {}: {:?}",
                task_id, workflow_id, mapping.logic
            );
            match self.compile_logic(&mapping.logic) {
                Ok(index) => {
                    mapping.logic_index = index;
                    debug!(
                        "Map logic for task {} compiled at index {:?}",
                        task_id, index
                    );
                }
                Err(e) => {
                    error!(
                        "Failed to parse map logic for task {} in workflow {}: {:?}",
                        task_id, workflow_id, e
                    );
                }
            }
        }
    }

    /// Compile validation rule logic
    fn compile_validation_logic(
        &mut self,
        config: &mut ValidationConfig,
        task_id: &str,
        workflow_id: &str,
    ) {
        for rule in &mut config.rules {
            debug!(
                "Compiling validation logic for task {} in workflow {}: {:?}",
                task_id, workflow_id, rule.logic
            );
            match self.compile_logic(&rule.logic) {
                Ok(index) => {
                    rule.logic_index = index;
                    debug!(
                        "Validation logic for task {} compiled at index {:?}",
                        task_id, index
                    );
                }
                Err(e) => {
                    error!(
                        "Failed to parse validation logic for task {} in workflow {}: {:?}",
                        task_id, workflow_id, e
                    );
                }
            }
        }
    }

    /// Compile log message and field expressions
    fn compile_log_logic(&mut self, config: &mut LogConfig, task_id: &str, workflow_id: &str) {
        // Compile the message expression
        debug!(
            "Compiling log message for task {} in workflow {}: {:?}",
            task_id, workflow_id, config.message
        );
        match self.compile_logic(&config.message) {
            Ok(index) => {
                config.message_index = index;
                debug!(
                    "Log message for task {} compiled at index {:?}",
                    task_id, index
                );
            }
            Err(e) => {
                error!(
                    "Failed to compile log message for task {} in workflow {}: {:?}",
                    task_id, workflow_id, e
                );
            }
        }

        // Compile each field expression
        config.field_indices = config
            .fields
            .iter()
            .map(|(key, logic)| {
                let idx = match self.compile_logic(logic) {
                    Ok(index) => {
                        debug!(
                            "Log field '{}' for task {} compiled at index {:?}",
                            key, task_id, index
                        );
                        index
                    }
                    Err(e) => {
                        error!(
                            "Failed to compile log field '{}' for task {} in workflow {}: {:?}",
                            key, task_id, workflow_id, e
                        );
                        None
                    }
                };
                (key.clone(), idx)
            })
            .collect();
    }

    /// Compile filter condition logic
    fn compile_filter_logic(
        &mut self,
        config: &mut FilterConfig,
        task_id: &str,
        workflow_id: &str,
    ) {
        debug!(
            "Compiling filter condition for task {} in workflow {}: {:?}",
            task_id, workflow_id, config.condition
        );
        match self.compile_logic(&config.condition) {
            Ok(index) => {
                config.condition_index = index;
                debug!(
                    "Filter condition for task {} compiled at index {:?}",
                    task_id, index
                );
            }
            Err(e) => {
                error!(
                    "Failed to compile filter condition for task {} in workflow {}: {:?}",
                    task_id, workflow_id, e
                );
            }
        }
    }

    /// Compile http_call JSONLogic expressions (path_logic, body_logic)
    fn compile_http_call_logic(
        &mut self,
        config: &mut HttpCallConfig,
        task_id: &str,
        workflow_id: &str,
    ) {
        if let Some(ref logic) = config.path_logic.clone() {
            match self.compile_logic(logic) {
                Ok(index) => config.path_logic_index = index,
                Err(e) => error!(
                    "Failed to compile http_call path_logic for task {} in workflow {}: {:?}",
                    task_id, workflow_id, e
                ),
            }
        }
        if let Some(ref logic) = config.body_logic.clone() {
            match self.compile_logic(logic) {
                Ok(index) => config.body_logic_index = index,
                Err(e) => error!(
                    "Failed to compile http_call body_logic for task {} in workflow {}: {:?}",
                    task_id, workflow_id, e
                ),
            }
        }
    }

    /// Compile enrich JSONLogic expressions (path_logic)
    fn compile_enrich_logic(
        &mut self,
        config: &mut EnrichConfig,
        task_id: &str,
        workflow_id: &str,
    ) {
        if let Some(ref logic) = config.path_logic.clone() {
            match self.compile_logic(logic) {
                Ok(index) => config.path_logic_index = index,
                Err(e) => error!(
                    "Failed to compile enrich path_logic for task {} in workflow {}: {:?}",
                    task_id, workflow_id, e
                ),
            }
        }
    }

    /// Compile publish_kafka JSONLogic expressions (key_logic, value_logic)
    fn compile_publish_kafka_logic(
        &mut self,
        config: &mut PublishKafkaConfig,
        task_id: &str,
        workflow_id: &str,
    ) {
        if let Some(ref logic) = config.key_logic.clone() {
            match self.compile_logic(logic) {
                Ok(index) => config.key_logic_index = index,
                Err(e) => error!(
                    "Failed to compile publish_kafka key_logic for task {} in workflow {}: {:?}",
                    task_id, workflow_id, e
                ),
            }
        }
        if let Some(ref logic) = config.value_logic.clone() {
            match self.compile_logic(logic) {
                Ok(index) => config.value_logic_index = index,
                Err(e) => error!(
                    "Failed to compile publish_kafka value_logic for task {} in workflow {}: {:?}",
                    task_id, workflow_id, e
                ),
            }
        }
    }

    /// Compile a logic expression and cache it
    fn compile_logic(&mut self, logic: &Value) -> Result<Option<usize>, String> {
        // DataLogic v4: compile returns Arc<CompiledLogic>
        match self.datalogic.compile(logic) {
            Ok(compiled) => {
                let index = self.logic_cache.len();
                self.logic_cache.push(compiled);
                Ok(Some(index))
            }
            Err(e) => Err(format!("Failed to compile logic: {}", e)),
        }
    }
}
