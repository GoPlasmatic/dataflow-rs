//! # Execution Trace Module
//!
//! This module provides step-by-step execution tracing for debugging workflows.
//! It captures message snapshots after each step, including which workflows/tasks
//! were executed or skipped.

use crate::engine::message::Message;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Result of executing a step (workflow or task)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StepResult {
    /// The step was executed
    Executed,
    /// The step was skipped due to condition being false
    Skipped,
}

/// A single step in the execution trace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionStep {
    /// ID of the workflow this step belongs to
    pub workflow_id: String,
    /// ID of the task (None for workflow-level skips)
    pub task_id: Option<String>,
    /// Result of the step execution
    pub result: StepResult,
    /// Message snapshot after this step (only for Executed steps)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    /// Context snapshots before each mapping (map tasks only, trace mode only).
    /// mapping_contexts[i] = message.context before mapping[i] executed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mapping_contexts: Option<Vec<Value>>,
}

impl ExecutionStep {
    /// Create a new executed step with a message snapshot
    pub fn executed(workflow_id: &str, task_id: &str, message: &Message) -> Self {
        Self {
            workflow_id: workflow_id.to_string(),
            task_id: Some(task_id.to_string()),
            result: StepResult::Executed,
            message: Some(message.clone()),
            mapping_contexts: None,
        }
    }

    /// Create a skipped task step
    pub fn task_skipped(workflow_id: &str, task_id: &str) -> Self {
        Self {
            workflow_id: workflow_id.to_string(),
            task_id: Some(task_id.to_string()),
            result: StepResult::Skipped,
            message: None,
            mapping_contexts: None,
        }
    }

    /// Create a skipped workflow step
    pub fn workflow_skipped(workflow_id: &str) -> Self {
        Self {
            workflow_id: workflow_id.to_string(),
            task_id: None,
            result: StepResult::Skipped,
            message: None,
            mapping_contexts: None,
        }
    }

    /// Set mapping context snapshots (for map tasks in trace mode)
    pub fn with_mapping_contexts(mut self, contexts: Vec<Value>) -> Self {
        self.mapping_contexts = Some(contexts);
        self
    }
}

/// Complete execution trace containing all steps
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTrace {
    /// All execution steps in order
    pub steps: Vec<ExecutionStep>,
}

impl ExecutionTrace {
    /// Create a new empty execution trace
    pub fn new() -> Self {
        Self { steps: Vec::new() }
    }

    /// Add a step to the trace
    pub fn add_step(&mut self, step: ExecutionStep) {
        self.steps.push(step);
    }

    /// Get the final message (from the last executed step)
    pub fn final_message(&self) -> Option<&Message> {
        self.steps
            .iter()
            .rev()
            .find(|s| s.result == StepResult::Executed)
            .and_then(|s| s.message.as_ref())
    }

    /// Check if execution was successful (no errors in final message)
    pub fn is_success(&self) -> bool {
        self.final_message()
            .map(|m| m.errors.is_empty())
            .unwrap_or(true)
    }

    /// Get number of executed steps
    pub fn executed_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.result == StepResult::Executed)
            .count()
    }

    /// Get number of skipped steps
    pub fn skipped_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.result == StepResult::Skipped)
            .count()
    }
}

impl Default for ExecutionTrace {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_step_result_serialization() {
        assert_eq!(
            serde_json::to_string(&StepResult::Executed).unwrap(),
            "\"executed\""
        );
        assert_eq!(
            serde_json::to_string(&StepResult::Skipped).unwrap(),
            "\"skipped\""
        );
    }

    #[test]
    fn test_execution_step_executed() {
        let message = Message::from_value(&json!({"test": "data"}));
        let step = ExecutionStep::executed("workflow1", "task1", &message);

        assert_eq!(step.workflow_id, "workflow1");
        assert_eq!(step.task_id, Some("task1".to_string()));
        assert_eq!(step.result, StepResult::Executed);
        assert!(step.message.is_some());
    }

    #[test]
    fn test_execution_step_task_skipped() {
        let step = ExecutionStep::task_skipped("workflow1", "task1");

        assert_eq!(step.workflow_id, "workflow1");
        assert_eq!(step.task_id, Some("task1".to_string()));
        assert_eq!(step.result, StepResult::Skipped);
        assert!(step.message.is_none());
    }

    #[test]
    fn test_execution_step_workflow_skipped() {
        let step = ExecutionStep::workflow_skipped("workflow1");

        assert_eq!(step.workflow_id, "workflow1");
        assert_eq!(step.task_id, None);
        assert_eq!(step.result, StepResult::Skipped);
        assert!(step.message.is_none());
    }

    #[test]
    fn test_execution_step_with_mapping_contexts() {
        let message = Message::from_value(&json!({"test": "data"}));
        let contexts = vec![json!({"data": {"a": 1}}), json!({"data": {"a": 1, "b": 2}})];

        let step = ExecutionStep::executed("workflow1", "task1", &message)
            .with_mapping_contexts(contexts.clone());

        assert_eq!(step.mapping_contexts, Some(contexts));

        // Verify serialization includes mapping_contexts
        let serialized = serde_json::to_value(&step).unwrap();
        assert!(serialized.get("mapping_contexts").is_some());
        assert_eq!(serialized["mapping_contexts"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_execution_step_without_mapping_contexts_serialization() {
        let message = Message::from_value(&json!({"test": "data"}));
        let step = ExecutionStep::executed("workflow1", "task1", &message);

        // mapping_contexts is None, should be omitted in serialization
        let serialized = serde_json::to_value(&step).unwrap();
        assert!(serialized.get("mapping_contexts").is_none());
    }

    #[test]
    fn test_execution_trace() {
        let mut trace = ExecutionTrace::new();
        let message = Message::from_value(&json!({"test": "data"}));

        trace.add_step(ExecutionStep::workflow_skipped("workflow0"));
        trace.add_step(ExecutionStep::executed("workflow1", "task1", &message));
        trace.add_step(ExecutionStep::task_skipped("workflow1", "task2"));

        assert_eq!(trace.steps.len(), 3);
        assert_eq!(trace.executed_count(), 1);
        assert_eq!(trace.skipped_count(), 2);
        assert!(trace.final_message().is_some());
        assert!(trace.is_success());
    }
}
