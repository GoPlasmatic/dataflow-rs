//! # Workflow Compilation Module
//!
//! This module handles the pre-compilation of JSONLogic expressions used throughout
//! the engine. By compiling all logic at initialization time with DataLogic v4, we achieve:
//!
//! - Zero runtime compilation overhead
//! - Thread-safe compiled logic via Arc
//! - Early validation of logic expressions
//! - Efficient memory sharing across async tasks

use crate::engine::functions::{MapConfig, ValidationConfig};
use crate::engine::{FunctionConfig, Workflow};
use datalogic_rs::{CompiledLogic, DataLogic};
use log::{debug, error};
use serde_json::Value;
use std::collections::HashMap;
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

    /// Compile all workflows and their tasks
    pub fn compile_workflows(&mut self, workflows: Vec<Workflow>) -> HashMap<String, Workflow> {
        let mut workflow_map = HashMap::new();

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

                    workflow_map.insert(workflow.id.clone(), workflow);
                }
                Err(e) => {
                    error!(
                        "Failed to parse condition for workflow {}: {:?}",
                        workflow.id, e
                    );
                }
            }
        }

        workflow_map
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
            _ => {
                // Custom functions don't need pre-compilation
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
