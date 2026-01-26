//! # Workflow Execution Module
//!
//! This module handles the execution of workflows and their associated tasks.
//! It provides a clean separation between workflow orchestration and task execution.

use crate::engine::error::{DataflowError, ErrorInfo, Result};
use crate::engine::executor::InternalExecutor;
use crate::engine::message::{AuditTrail, Change, Message};
use crate::engine::task_executor::TaskExecutor;
use crate::engine::trace::{ExecutionStep, ExecutionTrace};
use crate::engine::workflow::Workflow;
use chrono::Utc;
use log::{debug, error, info, warn};
use serde_json::json;
use std::sync::Arc;

/// Handles the execution of workflows and their tasks
///
/// The `WorkflowExecutor` is responsible for:
/// - Evaluating workflow conditions
/// - Orchestrating task execution within workflows
/// - Managing workflow-level error handling
/// - Recording audit trails
pub struct WorkflowExecutor {
    /// Task executor for executing individual tasks
    task_executor: Arc<TaskExecutor>,
    /// Internal executor for condition evaluation
    internal_executor: Arc<InternalExecutor>,
}

impl WorkflowExecutor {
    /// Create a new WorkflowExecutor
    pub fn new(task_executor: Arc<TaskExecutor>, internal_executor: Arc<InternalExecutor>) -> Self {
        Self {
            task_executor,
            internal_executor,
        }
    }

    /// Execute a workflow if its condition is met
    ///
    /// This method:
    /// 1. Evaluates the workflow condition
    /// 2. Executes tasks sequentially if condition is met
    /// 3. Handles error recovery based on workflow configuration
    /// 4. Updates message metadata and audit trail
    ///
    /// # Arguments
    /// * `workflow` - The workflow to execute
    /// * `message` - The message being processed
    ///
    /// # Returns
    /// * `Result<bool>` - Ok(true) if workflow was executed, Ok(false) if skipped, Err on failure
    pub async fn execute(&self, workflow: &Workflow, message: &mut Message) -> Result<bool> {
        // Use cached context Arc for condition evaluation
        let context_arc = message.get_context_arc();

        // Evaluate workflow condition
        let should_execute = self
            .internal_executor
            .evaluate_condition(workflow.condition_index, context_arc)?;

        if !should_execute {
            debug!("Skipping workflow {} - condition not met", workflow.id);
            return Ok(false);
        }

        // Execute workflow tasks
        match self.execute_tasks(workflow, message).await {
            Ok(_) => {
                info!("Successfully completed workflow: {}", workflow.id);
                Ok(true)
            }
            Err(e) if workflow.continue_on_error => {
                warn!(
                    "Workflow {} encountered error but continuing: {:?}",
                    workflow.id, e
                );
                message.errors.push(
                    ErrorInfo::builder(
                        "WORKFLOW_ERROR",
                        format!("Workflow {} error: {}", workflow.id, e),
                    )
                    .workflow_id(&workflow.id)
                    .build(),
                );
                Ok(true)
            }
            Err(e) => {
                error!("Workflow {} failed: {:?}", workflow.id, e);
                Err(e)
            }
        }
    }

    /// Execute a workflow with step-by-step tracing
    ///
    /// Similar to `execute` but records execution steps for debugging.
    ///
    /// # Arguments
    /// * `workflow` - The workflow to execute
    /// * `message` - The message being processed
    /// * `trace` - The execution trace to record steps to
    ///
    /// # Returns
    /// * `Result<bool>` - Ok(true) if workflow was executed, Ok(false) if skipped, Err on failure
    pub async fn execute_with_trace(
        &self,
        workflow: &Workflow,
        message: &mut Message,
        trace: &mut ExecutionTrace,
    ) -> Result<bool> {
        // Use cached context Arc for condition evaluation
        let context_arc = message.get_context_arc();

        // Evaluate workflow condition
        let should_execute = self
            .internal_executor
            .evaluate_condition(workflow.condition_index, context_arc)?;

        if !should_execute {
            debug!("Skipping workflow {} - condition not met", workflow.id);
            // Record skipped workflow step
            trace.add_step(ExecutionStep::workflow_skipped(&workflow.id));
            return Ok(false);
        }

        // Execute workflow tasks with trace collection
        match self
            .execute_tasks_with_trace(workflow, message, trace)
            .await
        {
            Ok(_) => {
                info!("Successfully completed workflow: {}", workflow.id);
                Ok(true)
            }
            Err(e) if workflow.continue_on_error => {
                warn!(
                    "Workflow {} encountered error but continuing: {:?}",
                    workflow.id, e
                );
                message.errors.push(
                    ErrorInfo::builder(
                        "WORKFLOW_ERROR",
                        format!("Workflow {} error: {}", workflow.id, e),
                    )
                    .workflow_id(&workflow.id)
                    .build(),
                );
                Ok(true)
            }
            Err(e) => {
                error!("Workflow {} failed: {:?}", workflow.id, e);
                Err(e)
            }
        }
    }

    /// Execute all tasks in a workflow
    async fn execute_tasks(&self, workflow: &Workflow, message: &mut Message) -> Result<()> {
        for task in &workflow.tasks {
            // Use cached context Arc - it will be fresh if previous task modified it
            let context_arc = message.get_context_arc();

            // Evaluate task condition
            let should_execute = self
                .internal_executor
                .evaluate_condition(task.condition_index, context_arc)?;

            if !should_execute {
                debug!("Skipping task {} - condition not met", task.id);
                continue;
            }

            // Execute the task
            let result = self.task_executor.execute(task, message).await;

            // Handle task result
            self.handle_task_result(
                result,
                &workflow.id,
                &task.id,
                task.continue_on_error,
                message,
            )?;
        }

        Ok(())
    }

    /// Execute all tasks in a workflow with trace collection
    async fn execute_tasks_with_trace(
        &self,
        workflow: &Workflow,
        message: &mut Message,
        trace: &mut ExecutionTrace,
    ) -> Result<()> {
        for task in &workflow.tasks {
            // Use cached context Arc - it will be fresh if previous task modified it
            let context_arc = message.get_context_arc();

            // Evaluate task condition
            let should_execute = self
                .internal_executor
                .evaluate_condition(task.condition_index, context_arc)?;

            if !should_execute {
                debug!("Skipping task {} - condition not met", task.id);
                // Record skipped task step
                trace.add_step(ExecutionStep::task_skipped(&workflow.id, &task.id));
                continue;
            }

            // Execute the task
            let result = self.task_executor.execute(task, message).await;

            // Handle task result
            self.handle_task_result(
                result,
                &workflow.id,
                &task.id,
                task.continue_on_error,
                message,
            )?;

            // Record executed step with message snapshot
            trace.add_step(ExecutionStep::executed(&workflow.id, &task.id, message));
        }

        Ok(())
    }

    /// Handle the result of a task execution
    fn handle_task_result(
        &self,
        result: Result<(usize, Vec<Change>)>,
        workflow_id: &str,
        task_id: &str,
        continue_on_error: bool,
        message: &mut Message,
    ) -> Result<()> {
        match result {
            Ok((status, changes)) => {
                // Record audit trail
                message.audit_trail.push(AuditTrail {
                    timestamp: Utc::now(),
                    workflow_id: Arc::from(workflow_id),
                    task_id: Arc::from(task_id),
                    status,
                    changes,
                });

                // Update progress metadata for workflow chaining
                if let Some(metadata) = message.context["metadata"].as_object_mut() {
                    // Update existing progress or create new one
                    if let Some(progress) = metadata.get_mut("progress") {
                        if let Some(progress_obj) = progress.as_object_mut() {
                            progress_obj.insert("workflow_id".to_string(), json!(workflow_id));
                            progress_obj.insert("task_id".to_string(), json!(task_id));
                            progress_obj.insert("status_code".to_string(), json!(status));
                        }
                    } else {
                        metadata.insert(
                            "progress".to_string(),
                            json!({
                                "workflow_id": workflow_id,
                                "task_id": task_id,
                                "status_code": status
                            }),
                        );
                    }
                }
                message.invalidate_context_cache();

                // Check status code
                if (400..500).contains(&status) {
                    warn!("Task {} returned client error status: {}", task_id, status);
                } else if status >= 500 {
                    error!("Task {} returned server error status: {}", task_id, status);
                    if !continue_on_error {
                        return Err(DataflowError::Task(format!(
                            "Task {} failed with status {}",
                            task_id, status
                        )));
                    }
                }
                Ok(())
            }
            Err(e) => {
                error!("Task {} failed: {:?}", task_id, e);

                // Record error in audit trail
                message.audit_trail.push(AuditTrail {
                    timestamp: Utc::now(),
                    workflow_id: Arc::from(workflow_id),
                    task_id: Arc::from(task_id),
                    status: 500,
                    changes: vec![],
                });

                // Add error to message
                message.errors.push(
                    ErrorInfo::builder("TASK_ERROR", format!("Task {} error: {}", task_id, e))
                        .workflow_id(workflow_id)
                        .task_id(task_id)
                        .build(),
                );

                if !continue_on_error { Err(e) } else { Ok(()) }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::compiler::LogicCompiler;
    use serde_json::json;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_workflow_executor_skip_condition() {
        // Create a workflow with a false condition
        let workflow_json = r#"{
            "id": "test_workflow",
            "name": "Test Workflow",
            "condition": false,
            "tasks": [{
                "id": "dummy_task",
                "name": "Dummy Task",
                "function": {
                    "name": "map",
                    "input": {"mappings": []}
                }
            }]
        }"#;

        let mut compiler = LogicCompiler::new();
        let mut workflow = Workflow::from_json(workflow_json).unwrap();

        // Compile the workflow condition
        let workflows = compiler.compile_workflows(vec![workflow.clone()]);
        if let Some(compiled_workflow) = workflows.get("test_workflow") {
            workflow = compiled_workflow.clone();
        }

        let (datalogic, logic_cache) = compiler.into_parts();
        let internal_executor = Arc::new(InternalExecutor::new(datalogic.clone(), logic_cache));
        let task_executor = Arc::new(TaskExecutor::new(
            Arc::new(HashMap::new()),
            internal_executor.clone(),
            datalogic,
        ));
        let workflow_executor = WorkflowExecutor::new(task_executor, internal_executor);

        let mut message = Message::from_value(&json!({}));

        // Execute workflow - should be skipped due to false condition
        let executed = workflow_executor
            .execute(&workflow, &mut message)
            .await
            .unwrap();
        assert!(!executed);
        assert_eq!(message.audit_trail.len(), 0);
    }

    #[tokio::test]
    async fn test_workflow_executor_execute_success() {
        // Create a workflow with a true condition
        let workflow_json = r#"{
            "id": "test_workflow",
            "name": "Test Workflow",
            "condition": true,
            "tasks": [{
                "id": "dummy_task",
                "name": "Dummy Task",
                "function": {
                    "name": "map",
                    "input": {"mappings": []}
                }
            }]
        }"#;

        let mut compiler = LogicCompiler::new();
        let mut workflow = Workflow::from_json(workflow_json).unwrap();

        // Compile the workflow
        let workflows = compiler.compile_workflows(vec![workflow.clone()]);
        if let Some(compiled_workflow) = workflows.get("test_workflow") {
            workflow = compiled_workflow.clone();
        }

        let (datalogic, logic_cache) = compiler.into_parts();
        let internal_executor = Arc::new(InternalExecutor::new(datalogic.clone(), logic_cache));
        let task_executor = Arc::new(TaskExecutor::new(
            Arc::new(HashMap::new()),
            internal_executor.clone(),
            datalogic,
        ));
        let workflow_executor = WorkflowExecutor::new(task_executor, internal_executor);

        let mut message = Message::from_value(&json!({}));

        // Execute workflow - should succeed with empty task list
        let executed = workflow_executor
            .execute(&workflow, &mut message)
            .await
            .unwrap();
        assert!(executed);
    }
}
