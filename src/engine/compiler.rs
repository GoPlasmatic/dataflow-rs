//! # Workflow Compilation Module
//!
//! Pre-compiles all JSONLogic expressions used by workflows and tasks at engine
//! initialization. Compiled logic is stored as `Arc<Logic>` for zero-copy
//! sharing across async tasks; the `Engine` is wrapped in `Arc` and is `Send + Sync`
//! so the entire stack is safe to share across Tokio worker threads.

use crate::engine::functions::integration::{EnrichConfig, HttpCallConfig, PublishKafkaConfig};
use crate::engine::functions::{FilterConfig, LogConfig, MapConfig, ValidationConfig};
use crate::engine::{FunctionConfig, Workflow};
use datalogic_rs::{Engine, Logic};
use log::{debug, error};
use serde_json::Value;
use std::sync::Arc;

/// Compiles and caches JSONLogic expressions for optimal runtime performance.
pub struct LogicCompiler {
    /// Shared datalogic Engine used both for compilation and (later) evaluation.
    engine: Arc<Engine>,
    /// Cache of compiled logic expressions indexed by their position.
    logic_cache: Vec<Arc<Logic>>,
}

impl Default for LogicCompiler {
    fn default() -> Self {
        Self::new()
    }
}

impl LogicCompiler {
    /// Create a new LogicCompiler with a fresh v5 Engine configured for templating
    /// mode (the v5 successor of v4's `with_preserve_structure`).
    pub fn new() -> Self {
        Self {
            engine: Arc::new(Engine::builder().with_templating(true).build()),
            logic_cache: Vec::new(),
        }
    }

    /// Get the Engine instance
    pub fn engine(&self) -> Arc<Engine> {
        Arc::clone(&self.engine)
    }

    /// Get the logic cache
    pub fn logic_cache(&self) -> &Vec<Arc<Logic>> {
        &self.logic_cache
    }

    /// Consume the compiler and return its components
    pub fn into_parts(self) -> (Arc<Engine>, Vec<Arc<Logic>>) {
        (self.engine, self.logic_cache)
    }

    /// Compile all workflows and their tasks, returning them sorted by priority
    pub fn compile_workflows(&mut self, workflows: Vec<Workflow>) -> Vec<Workflow> {
        let mut compiled_workflows = Vec::new();

        for mut workflow in workflows {
            if let Err(e) = workflow.validate() {
                error!("Invalid workflow {}: {:?}", workflow.id, e);
                continue;
            }

            // Populate the cached Arc<str> ids so audit emission can refcount-bump
            // rather than reallocate per AuditTrail entry.
            workflow.id_arc = Arc::from(workflow.id.as_str());
            for task in &mut workflow.tasks {
                task.id_arc = Arc::from(task.id.as_str());
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
            // Pre-split the dot path so the hot path doesn't re-split per
            // write. The `#` prefix is preserved here — it's the explicit
            // "treat this as an object key, not an array index" hint that
            // `set_nested_value` consumes when deciding container shape; the
            // strip happens at lookup time inside `*_parts` helpers.
            let parts: Vec<Arc<str>> = mapping
                .path
                .split('.')
                .map(Arc::from)
                .collect();
            mapping.path_parts = Arc::from(parts.into_boxed_slice());
            mapping.path_arc = Arc::from(mapping.path.as_str());

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
        // v5: `compile_arc` returns `Arc<Logic>` directly, matching the v4
        // `Arc<CompiledLogic>` shape used by downstream evaluation paths.
        match self.engine.compile_arc(logic) {
            Ok(compiled) => {
                let index = self.logic_cache.len();
                self.logic_cache.push(compiled);
                Ok(Some(index))
            }
            Err(e) => Err(format!("Failed to compile logic: {}", e)),
        }
    }
}
