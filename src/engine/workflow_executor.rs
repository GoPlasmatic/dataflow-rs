//! # Workflow Execution Module
//!
//! This module handles the execution of workflows and their associated tasks.
//! It provides a clean separation between workflow orchestration and task execution.

use crate::engine::error::{DataflowError, ErrorInfo, Result};
use crate::engine::executor::{with_arena, ArenaContext, InternalExecutor};
use crate::engine::functions::{
    AsyncFunctionHandler, FunctionConfig, FILTER_STATUS_HALT, FILTER_STATUS_SKIP,
};
use crate::engine::message::{AuditTrail, Change, Message};
use crate::engine::task::Task;
use crate::engine::task_executor::TaskExecutor;
use crate::engine::trace::{ExecutionStep, ExecutionTrace};
use crate::engine::utils::set_nested_value;
use crate::engine::workflow::Workflow;
use chrono::{DateTime, Utc};
use datavalue::OwnedDataValue;
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::sync::Arc;

/// Result of handling a task, including possible control flow signals
enum TaskControlFlow {
    /// Continue executing the next task
    Continue,
    /// Stop executing further tasks in this workflow (filter halt)
    HaltWorkflow,
}

/// Return the index of the first task at or after `start` that is *not* a
/// synchronous built-in. Used to chunk `workflow.tasks` into sync-only
/// stretches that can share a single `ArenaContext`.
fn next_async_boundary(tasks: &[Task], start: usize) -> usize {
    let mut i = start;
    while i < tasks.len() && tasks[i].function.is_sync_builtin() {
        i += 1;
    }
    i
}

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

    /// Get a clone of the task_functions Arc for reuse in new engines
    pub fn task_functions(
        &self,
    ) -> Arc<HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>>> {
        self.task_executor.task_functions()
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
    pub async fn execute(
        &self,
        workflow: &Workflow,
        message: &mut Message,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        // Evaluate workflow condition directly against the OwnedDataValue context
        let should_execute = self
            .internal_executor
            .evaluate_condition(workflow.condition_index, &message.context)?;

        if !should_execute {
            debug!("Skipping workflow {} - condition not met", workflow.id);
            return Ok(false);
        }

        // Execute workflow tasks
        match self.execute_tasks(workflow, message, now).await {
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
        now: DateTime<Utc>,
    ) -> Result<bool> {
        // Evaluate workflow condition directly against the OwnedDataValue context
        let should_execute = self
            .internal_executor
            .evaluate_condition(workflow.condition_index, &message.context)?;

        if !should_execute {
            debug!("Skipping workflow {} - condition not met", workflow.id);
            // Record skipped workflow step
            trace.add_step(ExecutionStep::workflow_skipped(&workflow.id));
            return Ok(false);
        }

        // Execute workflow tasks with trace collection
        match self
            .execute_tasks_with_trace(workflow, message, trace, now)
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

    /// Execute all tasks in a workflow.
    ///
    /// Groups consecutive synchronous built-in tasks into a single
    /// `with_arena` scope so the arena form of `message.context` is built
    /// once at the start of the stretch and reused across `parse_json`,
    /// `map`, `validation`, `log`, and `filter`. Async tasks (HTTP, Kafka,
    /// custom handlers) break the stretch — the arena flushes any pending
    /// state back to `OwnedDataValue` automatically (since each sync task
    /// already mutates `message.context` in place) and the next stretch
    /// rebuilds the arena form.
    async fn execute_tasks(
        &self,
        workflow: &Workflow,
        message: &mut Message,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let tasks = &workflow.tasks;
        let mut idx = 0;
        while idx < tasks.len() {
            let stretch_end = next_async_boundary(tasks, idx);

            if stretch_end > idx {
                // Run [idx, stretch_end) as a sync stretch inside one arena.
                let halt = self.run_sync_stretch(
                    &tasks[idx..stretch_end],
                    workflow,
                    message,
                    now,
                )?;
                if halt {
                    return Ok(());
                }
                idx = stretch_end;
            }

            if idx < tasks.len() {
                // Single async task (or non-sync-builtin) at `idx`.
                let task = &tasks[idx];
                let should_execute = self
                    .internal_executor
                    .evaluate_condition(task.condition_index, &message.context)?;

                if !should_execute {
                    debug!("Skipping task {} - condition not met", task.id);
                    idx += 1;
                    continue;
                }

                let result = self.task_executor.execute(task, message).await;
                match self.handle_task_result(
                    result,
                    &workflow.id_arc,
                    &task.id_arc,
                    task.continue_on_error,
                    message,
                    now,
                )? {
                    TaskControlFlow::HaltWorkflow => return Ok(()),
                    TaskControlFlow::Continue => {}
                }
                idx += 1;
            }
        }

        Ok(())
    }

    /// Execute a contiguous run of sync-builtin tasks inside one
    /// `with_arena` scope. The arena context is built once at the start and
    /// refreshed in place after each mutating task. Returns `Ok(true)` if a
    /// filter task halted the workflow.
    fn run_sync_stretch(
        &self,
        tasks: &[Task],
        workflow: &Workflow,
        message: &mut Message,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let outcome = with_arena(|arena| -> Result<bool> {
            let mut arena_ctx = ArenaContext::from_owned(&message.context, arena);

            for task in tasks {
                // Task condition — evaluate against the arena form so we don't
                // re-borrow the thread-local `RefCell`.
                let ctx_av = arena_ctx.as_data_value();
                let should_execute = self
                    .internal_executor
                    .evaluate_condition_in_arena(task.condition_index, ctx_av, arena)?;

                if !should_execute {
                    debug!("Skipping task {} - condition not met", task.id);
                    continue;
                }

                let result = self.execute_sync_task_in_arena(task, message, &mut arena_ctx);

                let control_flow = self.handle_task_result(
                    result,
                    &workflow.id_arc,
                    &task.id_arc,
                    task.continue_on_error,
                    message,
                    now,
                )?;

                // The audit-trail / progress-metadata writes performed by
                // `handle_task_result` mutate `message.context`. Refresh the
                // arena cache so the next task in the stretch sees them.
                arena_ctx.refresh_for_path(&message.context, "metadata");

                if matches!(control_flow, TaskControlFlow::HaltWorkflow) {
                    return Ok(true);
                }
            }
            Ok(false)
        })?;
        Ok(outcome)
    }

    /// Dispatch a single sync-builtin task to its `execute_in_arena` variant.
    /// All variants in `is_sync_builtin()` must be handled here.
    fn execute_sync_task_in_arena(
        &self,
        task: &Task,
        message: &mut Message,
        arena_ctx: &mut ArenaContext<'_>,
    ) -> Result<(usize, Vec<Change>)> {
        debug!(
            "Executing sync task in arena: {} ({})",
            task.id,
            task.function.function_name()
        );
        let executor = &self.internal_executor;
        let engine = executor.engine();
        let logic_cache = executor.logic_cache();

        match &task.function {
            FunctionConfig::Map { input, .. } => {
                input.execute_in_arena(message, arena_ctx, engine, logic_cache)
            }
            FunctionConfig::Validation { input, .. } => {
                input.execute_in_arena(message, arena_ctx, engine, logic_cache)
            }
            FunctionConfig::ParseJson { input, .. } => {
                crate::engine::functions::parse::execute_parse_json_in_arena(
                    message, input, arena_ctx,
                )
            }
            FunctionConfig::ParseXml { input, .. } => {
                let r = crate::engine::functions::parse::execute_parse_xml(message, input)?;
                arena_ctx.refresh_for_path(&message.context, "data");
                Ok(r)
            }
            FunctionConfig::PublishJson { input, .. } => {
                // Publish is read-only on the OwnedDataValue context (it writes
                // to `payload`). No arena refresh needed.
                crate::engine::functions::publish::execute_publish_json(message, input)
            }
            FunctionConfig::PublishXml { input, .. } => {
                crate::engine::functions::publish::execute_publish_xml(message, input)
            }
            FunctionConfig::Filter { input, .. } => {
                input.execute(message, engine, logic_cache)
            }
            FunctionConfig::Log { input, .. } => {
                input.execute(message, engine, logic_cache)
            }
            // Async or non-builtin variants should never reach this dispatch —
            // `next_async_boundary` guarantees stretch contents are sync builtins.
            _ => unreachable!(
                "execute_sync_task_in_arena called with non-sync-builtin task: {}",
                task.function.function_name()
            ),
        }
    }

    /// Execute all tasks in a workflow with trace collection
    async fn execute_tasks_with_trace(
        &self,
        workflow: &Workflow,
        message: &mut Message,
        trace: &mut ExecutionTrace,
        now: DateTime<Utc>,
    ) -> Result<()> {
        for task in &workflow.tasks {
            // Evaluate task condition directly against the OwnedDataValue context
            let should_execute = self
                .internal_executor
                .evaluate_condition(task.condition_index, &message.context)?;

            if !should_execute {
                debug!("Skipping task {} - condition not met", task.id);
                // Record skipped task step
                trace.add_step(ExecutionStep::task_skipped(&workflow.id, &task.id));
                continue;
            }

            // Execute the task with trace support
            let result = self.task_executor.execute_with_trace(task, message).await;

            // Extract mapping_contexts before passing result to handle_task_result
            let mapping_contexts = match &result {
                Ok((_, _, contexts)) => contexts.clone(),
                Err(_) => None,
            };

            // Convert to the standard result format for handle_task_result
            let standard_result = result.map(|(status, changes, _)| (status, changes));

            // Handle task result with control flow
            let control_flow = self.handle_task_result(
                standard_result,
                &workflow.id_arc,
                &task.id_arc,
                task.continue_on_error,
                message,
                now,
            )?;

            // Record executed step with message snapshot and optional mapping contexts
            let mut step = ExecutionStep::executed(&workflow.id, &task.id, message);
            if let Some(contexts) = mapping_contexts {
                step = step.with_mapping_contexts(contexts);
            }
            trace.add_step(step);

            if let TaskControlFlow::HaltWorkflow = control_flow {
                break;
            }
        }

        Ok(())
    }

    /// Handle the result of a task execution.
    ///
    /// `workflow_id_arc` and `task_id_arc` are the compile-time cached
    /// `Arc<str>` mirrors of `workflow.id` / `task.id`; we Arc-clone them into
    /// each `AuditTrail` rather than reallocating from the `&str` form.
    fn handle_task_result(
        &self,
        result: Result<(usize, Vec<Change>)>,
        workflow_id_arc: &Arc<str>,
        task_id_arc: &Arc<str>,
        continue_on_error: bool,
        message: &mut Message,
        now: DateTime<Utc>,
    ) -> Result<TaskControlFlow> {
        let workflow_id: &str = workflow_id_arc;
        let task_id: &str = task_id_arc;
        match result {
            Ok((status, changes)) => {
                // Handle filter skip — no audit trail, just continue
                if status == FILTER_STATUS_SKIP {
                    debug!("Task {} signaled skip (filter gate)", task_id);
                    return Ok(TaskControlFlow::Continue);
                }

                // Record audit trail. workflow_id_arc/task_id_arc are populated
                // by LogicCompiler at engine construction; cloning them is a
                // refcount bump, not a string copy. `now` is shared with all
                // other AuditTrails in this process_message call.
                message.audit_trail.push(AuditTrail {
                    timestamp: now,
                    workflow_id: Arc::clone(workflow_id_arc),
                    task_id: Arc::clone(task_id_arc),
                    status,
                    changes,
                });

                // Update progress metadata for workflow chaining. set_nested_value
                // auto-creates the intermediate "progress" object on first write
                // and overwrites individual keys on subsequent writes.
                set_nested_value(
                    &mut message.context,
                    "metadata.progress.workflow_id",
                    OwnedDataValue::String(workflow_id.to_string()),
                );
                set_nested_value(
                    &mut message.context,
                    "metadata.progress.task_id",
                    OwnedDataValue::String(task_id.to_string()),
                );
                set_nested_value(
                    &mut message.context,
                    "metadata.progress.status_code",
                    OwnedDataValue::from(status as u64),
                );

                // Handle filter halt — audit trail is recorded, halt the workflow
                if status == FILTER_STATUS_HALT {
                    info!(
                        "Task {} halted workflow {} (filter gate)",
                        task_id, workflow_id
                    );
                    return Ok(TaskControlFlow::HaltWorkflow);
                }

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
                Ok(TaskControlFlow::Continue)
            }
            Err(e) => {
                error!("Task {} failed: {:?}", task_id, e);

                // Record error in audit trail (Arc clones are refcount bumps).
                message.audit_trail.push(AuditTrail {
                    timestamp: now,
                    workflow_id: Arc::clone(workflow_id_arc),
                    task_id: Arc::clone(task_id_arc),
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

                if !continue_on_error {
                    Err(e)
                } else {
                    Ok(TaskControlFlow::Continue)
                }
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
        if let Some(compiled_workflow) = workflows.iter().find(|w| w.id == "test_workflow") {
            workflow = compiled_workflow.clone();
        }

        let (engine, logic_cache) = compiler.into_parts();
        let internal_executor = Arc::new(InternalExecutor::new(engine.clone(), logic_cache));
        let task_executor = Arc::new(TaskExecutor::new(
            Arc::new(HashMap::new()),
            internal_executor.clone(),
            engine,
        ));
        let workflow_executor = WorkflowExecutor::new(task_executor, internal_executor);

        let mut message = Message::from_value(&json!({}));

        // Execute workflow - should be skipped due to false condition
        let executed = workflow_executor
            .execute(&workflow, &mut message, Utc::now())
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
        if let Some(compiled_workflow) = workflows.iter().find(|w| w.id == "test_workflow") {
            workflow = compiled_workflow.clone();
        }

        let (engine, logic_cache) = compiler.into_parts();
        let internal_executor = Arc::new(InternalExecutor::new(engine.clone(), logic_cache));
        let task_executor = Arc::new(TaskExecutor::new(
            Arc::new(HashMap::new()),
            internal_executor.clone(),
            engine,
        ));
        let workflow_executor = WorkflowExecutor::new(task_executor, internal_executor);

        let mut message = Message::from_value(&json!({}));

        // Execute workflow - should succeed with empty task list
        let executed = workflow_executor
            .execute(&workflow, &mut message, Utc::now())
            .await
            .unwrap();
        assert!(executed);
    }
}
