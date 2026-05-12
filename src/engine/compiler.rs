//! # Workflow Compilation Module
//!
//! Pre-compiles all JSONLogic expressions used by workflows and tasks at engine
//! initialization. Each compiled `Arc<Logic>` is stored directly on the
//! workflow/task/config struct that owns it — no central `logic_cache`, no
//! index lookup, no bounds check on the hot path. The `Engine` is wrapped in
//! `Arc` and is `Send + Sync` so the entire stack is safe to share across
//! Tokio worker threads.

use crate::engine::error::{DataflowError, Result};
use crate::engine::functions::integration::{EnrichConfig, HttpCallConfig, PublishKafkaConfig};
use crate::engine::functions::{FilterConfig, LogConfig, MapConfig, ValidationConfig};
use crate::engine::{FunctionConfig, Workflow};
use datalogic_rs::{Engine, Logic};
use log::debug;
use serde_json::Value;
use std::sync::Arc;

/// Compiles JSONLogic expressions and stamps them onto workflow/task/config
/// structs as `Option<Arc<Logic>>` slots.
pub struct LogicCompiler {
    /// Shared datalogic Engine used both for compilation and (later) evaluation.
    engine: Arc<Engine>,
}

impl Default for LogicCompiler {
    fn default() -> Self {
        Self::new()
    }
}

impl LogicCompiler {
    /// Create a new LogicCompiler with a fresh datalogic `Engine` configured for
    /// templating mode (preserves object structure in JSONLogic operations).
    pub fn new() -> Self {
        Self {
            engine: Arc::new(Engine::builder().with_templating(true).build()),
        }
    }

    /// Get the Engine instance
    pub fn engine(&self) -> Arc<Engine> {
        Arc::clone(&self.engine)
    }

    /// Consume the compiler and return the shared engine.
    pub fn into_engine(self) -> Arc<Engine> {
        self.engine
    }

    /// Compile all workflows and their tasks, returning them sorted by priority.
    /// Returns `Err` on the first validation or compilation failure — engine
    /// construction is fail-loud so misconfigured workflows can't silently
    /// disappear at runtime.
    pub fn compile_workflows(&self, workflows: Vec<Workflow>) -> Result<Vec<Workflow>> {
        let mut compiled_workflows = Vec::with_capacity(workflows.len());

        for mut workflow in workflows {
            workflow.validate()?;

            // Populate the cached Arc<str> ids so audit emission can refcount-bump
            // rather than reallocate per AuditTrail entry.
            workflow.id_arc = Arc::from(workflow.id.as_str());
            for task in &mut workflow.tasks {
                task.id_arc = Arc::from(task.id.as_str());
            }

            // Compile the workflow condition (required — defaults to `true`).
            let label = format!("workflow {} condition", workflow.id);
            workflow.compiled_condition = Some(self.compile(&workflow.condition, &label)?);
            debug!("Workflow {} condition compiled", workflow.id);

            // Compile task conditions and function-specific logic.
            self.compile_workflow_tasks(&mut workflow)?;

            compiled_workflows.push(workflow);
        }

        // Sort by priority once at construction time
        compiled_workflows.sort_by_key(|w| w.priority);
        Ok(compiled_workflows)
    }

    /// Compile task conditions and function logic for a workflow
    fn compile_workflow_tasks(&self, workflow: &mut Workflow) -> Result<()> {
        for task in &mut workflow.tasks {
            let label = format!("task {} condition (workflow {})", task.id, workflow.id);
            task.compiled_condition = Some(self.compile(&task.condition, &label)?);

            // Compile function-specific logic (map transformations, validation rules, …)
            self.compile_function_logic(&mut task.function, &task.id, &workflow.id)?;
        }
        Ok(())
    }

    /// Compile function-specific logic based on function type
    fn compile_function_logic(
        &self,
        function: &mut FunctionConfig,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        match function {
            FunctionConfig::Map { input, .. } => {
                self.compile_map_logic(input, task_id, workflow_id)
            }
            FunctionConfig::Validation { input, .. } => {
                self.compile_validation_logic(input, task_id, workflow_id)
            }
            FunctionConfig::Filter { input, .. } => {
                self.compile_filter_logic(input, task_id, workflow_id)
            }
            FunctionConfig::Log { input, .. } => {
                self.compile_log_logic(input, task_id, workflow_id)
            }
            FunctionConfig::HttpCall { input, .. } => {
                self.compile_http_call_logic(input, task_id, workflow_id)
            }
            FunctionConfig::Enrich { input, .. } => {
                self.compile_enrich_logic(input, task_id, workflow_id)
            }
            FunctionConfig::PublishKafka { input, .. } => {
                self.compile_publish_kafka_logic(input, task_id, workflow_id)
            }
            // Custom and other functions don't need pre-compilation
            _ => Ok(()),
        }
    }

    /// Compile a JSONLogic expression and return the `Arc<Logic>`. Errors are
    /// surfaced as `DataflowError::LogicEvaluation` with the supplied
    /// context label for debugging.
    fn compile(&self, logic: &Value, ctx_label: &str) -> Result<Arc<Logic>> {
        self.engine.compile_arc(logic).map_err(|e| {
            DataflowError::LogicEvaluation(format!("{}: {}", ctx_label, e))
        })
    }

    /// Compile map transformation logic
    fn compile_map_logic(
        &self,
        config: &mut MapConfig,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        for mapping in &mut config.mappings {
            // Pre-split the dot path so the hot path doesn't re-split per
            // write. The `#` prefix is preserved here — it's the explicit
            // "treat this as an object key, not an array index" hint that
            // `set_nested_value` consumes when deciding container shape; the
            // strip happens at lookup time inside `*_parts` helpers.
            let parts: Vec<Arc<str>> = mapping.path.split('.').map(Arc::from).collect();
            mapping.path_parts = Arc::from(parts.into_boxed_slice());
            mapping.path_arc = Arc::from(mapping.path.as_str());

            let label = format!(
                "map logic for task {} in workflow {} (path {})",
                task_id, workflow_id, mapping.path
            );
            mapping.compiled_logic = Some(self.compile(&mapping.logic, &label)?);
        }
        Ok(())
    }

    /// Compile validation rule logic
    fn compile_validation_logic(
        &self,
        config: &mut ValidationConfig,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        for (idx, rule) in config.rules.iter_mut().enumerate() {
            let label = format!(
                "validation rule {} for task {} in workflow {}",
                idx, task_id, workflow_id
            );
            rule.compiled_logic = Some(self.compile(&rule.logic, &label)?);
        }
        Ok(())
    }

    /// Compile log message and field expressions
    fn compile_log_logic(
        &self,
        config: &mut LogConfig,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        let msg_label = format!(
            "log message for task {} in workflow {}",
            task_id, workflow_id
        );
        config.compiled_message = Some(self.compile(&config.message, &msg_label)?);

        // Compile each field expression. Collect into a fresh Vec, then
        // assign — keeps the immutable borrow of `config.fields` from
        // overlapping with the mutable borrow of `config.compiled_fields`.
        let mut compiled_fields = Vec::with_capacity(config.fields.len());
        for (key, logic) in &config.fields {
            let label = format!(
                "log field '{}' for task {} in workflow {}",
                key, task_id, workflow_id
            );
            compiled_fields.push((key.clone(), Some(self.compile(logic, &label)?)));
        }
        config.compiled_fields = compiled_fields;
        Ok(())
    }

    /// Compile filter condition logic
    fn compile_filter_logic(
        &self,
        config: &mut FilterConfig,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        let label = format!(
            "filter condition for task {} in workflow {}",
            task_id, workflow_id
        );
        config.compiled_condition = Some(self.compile(&config.condition, &label)?);
        Ok(())
    }

    /// Compile http_call JSONLogic expressions (path_logic, body_logic)
    fn compile_http_call_logic(
        &self,
        config: &mut HttpCallConfig,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        if let Some(logic) = &config.path_logic {
            let label = format!(
                "http_call path_logic for task {} in workflow {}",
                task_id, workflow_id
            );
            config.compiled_path_logic = Some(self.compile(logic, &label)?);
        }
        if let Some(logic) = &config.body_logic {
            let label = format!(
                "http_call body_logic for task {} in workflow {}",
                task_id, workflow_id
            );
            config.compiled_body_logic = Some(self.compile(logic, &label)?);
        }
        Ok(())
    }

    /// Compile enrich JSONLogic expressions (path_logic)
    fn compile_enrich_logic(
        &self,
        config: &mut EnrichConfig,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        if let Some(logic) = &config.path_logic {
            let label = format!(
                "enrich path_logic for task {} in workflow {}",
                task_id, workflow_id
            );
            config.compiled_path_logic = Some(self.compile(logic, &label)?);
        }
        Ok(())
    }

    /// Compile publish_kafka JSONLogic expressions (key_logic, value_logic)
    fn compile_publish_kafka_logic(
        &self,
        config: &mut PublishKafkaConfig,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        if let Some(logic) = &config.key_logic {
            let label = format!(
                "publish_kafka key_logic for task {} in workflow {}",
                task_id, workflow_id
            );
            config.compiled_key_logic = Some(self.compile(logic, &label)?);
        }
        if let Some(logic) = &config.value_logic {
            let label = format!(
                "publish_kafka value_logic for task {} in workflow {}",
                task_id, workflow_id
            );
            config.compiled_value_logic = Some(self.compile(logic, &label)?);
        }
        Ok(())
    }
}
