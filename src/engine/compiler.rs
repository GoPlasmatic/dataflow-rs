use crate::engine::functions::{MapConfig, ValidationConfig};
use crate::engine::{FunctionConfig, Workflow};
use datalogic_rs::{DataLogic, Logic};
use log::{debug, error};
use serde_json::Value;
use std::collections::HashMap;

/// Handles compilation of JSONLogic expressions for workflows and tasks
pub struct LogicCompiler {
    datalogic: DataLogic<'static>,
    logic_cache: Vec<Logic<'static>>,
}

impl Default for LogicCompiler {
    fn default() -> Self {
        Self::new()
    }
}

impl LogicCompiler {
    /// Create a new LogicCompiler
    pub fn new() -> Self {
        Self {
            datalogic: DataLogic::with_preserve_structure(),
            logic_cache: Vec::new(),
        }
    }

    /// Get the DataLogic instance
    pub fn datalogic(&self) -> &DataLogic<'static> {
        &self.datalogic
    }

    /// Get the logic cache
    pub fn logic_cache(&self) -> &Vec<Logic<'static>> {
        &self.logic_cache
    }

    /// Consume the compiler and return its components
    pub fn into_parts(self) -> (DataLogic<'static>, Vec<Logic<'static>>) {
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

    /// Compile tasks within a workflow
    fn compile_workflow_tasks(&mut self, workflow: &mut Workflow) {
        for task in &mut workflow.tasks {
            // Skip simple boolean conditions
            if !matches!(task.condition, Value::Bool(_)) {
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
            }

            // Compile logic for map and validation functions
            self.compile_function_logic(&mut task.function, &task.id, &workflow.id);
        }
    }

    /// Compile logic within function configurations
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
                // Custom functions don't have logic to compile
            }
        }
    }

    /// Compile map function logic
    fn compile_map_logic(&mut self, config: &mut MapConfig, task_id: &str, workflow_id: &str) {
        for mapping in &mut config.mappings {
            // Skip if logic is a simple value
            if !mapping.logic.is_object() && !mapping.logic.is_array() {
                continue;
            }

            debug!(
                "Compiling map logic for task {} in workflow {}: {:?}",
                task_id, workflow_id, mapping.logic
            );
            match self.compile_logic(&mapping.logic) {
                Ok(index) => {
                    mapping.logic_index = index;
                    debug!("Map logic compiled at index {:?}", index);
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

    /// Compile validation function logic
    fn compile_validation_logic(
        &mut self,
        config: &mut ValidationConfig,
        task_id: &str,
        workflow_id: &str,
    ) {
        for rule in &mut config.rules {
            // Skip if logic is a simple value
            if !rule.logic.is_object() && !rule.logic.is_array() {
                continue;
            }

            debug!(
                "Compiling validation logic for task {} in workflow {}: {:?}",
                task_id, workflow_id, rule.logic
            );
            match self.compile_logic(&rule.logic) {
                Ok(index) => {
                    rule.logic_index = index;
                    debug!("Validation logic compiled at index {:?}", index);
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

    /// Compile a single logic expression and add it to the cache
    fn compile_logic(&mut self, logic: &Value) -> Result<Option<usize>, String> {
        match self.datalogic.parse_logic_json(logic) {
            Ok(compiled) => {
                let index = self.logic_cache.len();
                self.logic_cache.push(compiled);
                Ok(Some(index))
            }
            Err(e) => Err(format!("Failed to compile logic: {}", e)),
        }
    }
}
