import { useMemo } from 'react';
import type { Workflow, Task, DebugNodeState } from '../../../types';
import { getWorkflowState, getTaskState, isTaskCurrent } from '../../../types';
import { useDebuggerOptional } from '../context';

/**
 * Result of the debug state hook for a tree node
 */
export interface TreeNodeDebugState {
  /** Current debug state of this node */
  state: DebugNodeState | null;
  /** Whether this node is the current step being viewed */
  isCurrent: boolean;
  /** Whether this node has been executed (or is being executed) */
  isExecuted: boolean;
  /** Whether this node was skipped due to condition */
  isSkipped: boolean;
  /** Whether this node had an error */
  hasError: boolean;
  /** Condition result if this is a condition node */
  conditionResult?: boolean;
}

/**
 * Null state when debugger is not active
 */
const nullState: TreeNodeDebugState = {
  state: null,
  isCurrent: false,
  isExecuted: false,
  isSkipped: false,
  hasError: false,
};

/**
 * Hook to get debug state for a workflow node
 * Note: Workflows never show as 'current' - only tasks do
 */
export function useWorkflowDebugState(workflow: Workflow): TreeNodeDebugState {
  const dbgContext = useDebuggerOptional();

  return useMemo(() => {
    if (!dbgContext || !dbgContext.state.isActive || !dbgContext.hasTrace || !dbgContext.state.trace) {
      return nullState;
    }

    // Don't show any state when at step -1 (ready state)
    if (dbgContext.state.currentStepIndex < 0) {
      return nullState;
    }

    const state = getWorkflowState(
      dbgContext.state.trace,
      dbgContext.state.currentStepIndex,
      workflow.id
    );

    return {
      state,
      isCurrent: false, // Workflows never show as current
      isExecuted: state === 'executed',
      isSkipped: state === 'skipped',
      hasError: state === 'error',
    };
  }, [dbgContext, workflow.id]);
}

/**
 * Hook to get debug state for a workflow condition node
 * Note: In the simplified trace format, workflow conditions are implicit
 * A workflow skip step indicates the condition was false
 */
export function useWorkflowConditionDebugState(workflow: Workflow): TreeNodeDebugState {
  const dbgContext = useDebuggerOptional();

  return useMemo(() => {
    if (!dbgContext || !dbgContext.state.isActive || !dbgContext.hasTrace || !dbgContext.state.trace) {
      return nullState;
    }

    // Don't show any state when at step -1 (ready state)
    if (dbgContext.state.currentStepIndex < 0) {
      return nullState;
    }

    const { trace, currentStepIndex } = dbgContext.state;

    // Find workflow-level skip step (task_id is undefined/null for workflow skips)
    const workflowSkipStep = trace.steps.find(
      s => s.workflow_id === workflow.id && !s.task_id && s.result === 'skipped'
    );

    if (workflowSkipStep) {
      // Workflow was skipped due to condition
      const stepIndex = trace.steps.indexOf(workflowSkipStep);
      return {
        state: stepIndex === currentStepIndex ? 'current' : 'skipped',
        isCurrent: stepIndex === currentStepIndex,
        isExecuted: false,
        isSkipped: true,
        hasError: false,
        conditionResult: false,
      };
    }

    // Check if workflow has any executed tasks (condition passed)
    const hasExecutedTasks = trace.steps.some(
      s => s.workflow_id === workflow.id && s.task_id && s.result === 'executed'
    );

    if (hasExecutedTasks) {
      return {
        state: 'executed',
        isCurrent: false,
        isExecuted: true,
        isSkipped: false,
        hasError: false,
        conditionResult: true,
      };
    }

    return nullState;
  }, [dbgContext, workflow.id]);
}

/**
 * Hook to get debug state for a task node
 */
export function useTaskDebugState(task: Task, workflow: Workflow): TreeNodeDebugState {
  const dbgContext = useDebuggerOptional();

  return useMemo(() => {
    if (!dbgContext || !dbgContext.state.isActive || !dbgContext.hasTrace || !dbgContext.state.trace) {
      return nullState;
    }

    // Don't show any state when at step -1 (ready state)
    if (dbgContext.state.currentStepIndex < 0) {
      return nullState;
    }

    const state = getTaskState(
      dbgContext.state.trace,
      dbgContext.state.currentStepIndex,
      workflow.id,
      task.id
    );

    const isCurrent = isTaskCurrent(
      dbgContext.state.trace,
      dbgContext.state.currentStepIndex,
      workflow.id,
      task.id
    );

    return {
      state,
      isCurrent,
      isExecuted: state === 'executed',
      isSkipped: state === 'skipped',
      hasError: state === 'error',
    };
  }, [dbgContext, task.id, workflow.id]);
}

/**
 * Hook to get debug state for a task condition node
 * Note: In the simplified trace format, task conditions are implicit
 * A task skip step indicates the condition was false
 */
export function useTaskConditionDebugState(task: Task, workflow: Workflow): TreeNodeDebugState {
  const dbgContext = useDebuggerOptional();

  return useMemo(() => {
    if (!dbgContext || !dbgContext.state.isActive || !dbgContext.hasTrace || !dbgContext.state.trace) {
      return nullState;
    }

    // Don't show any state when at step -1 (ready state)
    if (dbgContext.state.currentStepIndex < 0) {
      return nullState;
    }

    const { trace, currentStepIndex } = dbgContext.state;

    // Find the step for this task
    const taskStep = trace.steps.find(
      s => s.workflow_id === workflow.id && s.task_id === task.id
    );

    if (!taskStep) {
      return nullState;
    }

    const stepIndex = trace.steps.indexOf(taskStep);
    const isCurrent = stepIndex === currentStepIndex;

    // If step is after current position, it's pending
    if (stepIndex > currentStepIndex) {
      return nullState;
    }

    if (taskStep.result === 'skipped') {
      // Task was skipped due to condition
      return {
        state: 'skipped',
        isCurrent,
        isExecuted: false,
        isSkipped: true,
        hasError: false,
        conditionResult: false,
      };
    }

    if (taskStep.result === 'executed') {
      return {
        state: 'executed',
        isCurrent,
        isExecuted: true,
        isSkipped: false,
        hasError: false,
        conditionResult: true,
      };
    }

    return nullState;
  }, [dbgContext, task.id, workflow.id]);
}

/**
 * Hook to get debug state for a mapping node within a task
 */
export function useMappingDebugState(task: Task, workflow: Workflow, _mappingIndex: number): TreeNodeDebugState {
  const dbgContext = useDebuggerOptional();

  return useMemo(() => {
    if (!dbgContext || !dbgContext.state.isActive || !dbgContext.hasTrace || !dbgContext.state.trace) {
      return nullState;
    }

    if (dbgContext.state.currentStepIndex < 0) {
      return nullState;
    }

    const state = getTaskState(
      dbgContext.state.trace,
      dbgContext.state.currentStepIndex,
      workflow.id,
      task.id
    );

    const isCurrent = isTaskCurrent(
      dbgContext.state.trace,
      dbgContext.state.currentStepIndex,
      workflow.id,
      task.id
    );

    return {
      state,
      isCurrent,
      isExecuted: state === 'executed',
      isSkipped: state === 'skipped',
      hasError: state === 'error',
    };
  }, [dbgContext, task.id, workflow.id, _mappingIndex]);
}

/**
 * Hook to get debug state for a validation rule node within a task
 */
export function useValidationRuleDebugState(task: Task, workflow: Workflow, ruleIndex: number): TreeNodeDebugState {
  const dbgContext = useDebuggerOptional();

  return useMemo(() => {
    if (!dbgContext || !dbgContext.state.isActive || !dbgContext.hasTrace || !dbgContext.state.trace) {
      return nullState;
    }

    if (dbgContext.state.currentStepIndex < 0) {
      return nullState;
    }

    const { trace, currentStepIndex } = dbgContext.state;

    const taskStepIndex = trace.steps.findIndex(
      s => s.workflow_id === workflow.id && s.task_id === task.id
    );

    if (taskStepIndex === -1 || taskStepIndex > currentStepIndex) {
      return { ...nullState, state: 'pending' as DebugNodeState };
    }

    const taskStep = trace.steps[taskStepIndex];

    if (taskStep.result === 'skipped') {
      return {
        state: 'skipped' as DebugNodeState,
        isCurrent: false,
        isExecuted: false,
        isSkipped: true,
        hasError: false,
      };
    }

    // Check if this specific rule produced an error
    const isCurrent = taskStepIndex === currentStepIndex;
    let hasError = false;
    if (taskStep.message && taskStep.message.errors.length > 0) {
      // Validation errors include the rule message; check if any error matches
      const rules = (task.function?.input as Record<string, unknown> | undefined);
      const rulesList = (rules?.rules as Array<{ message: string }>) || [];
      const rule = rulesList[ruleIndex];
      if (rule) {
        hasError = taskStep.message.errors.some(e => e.message.includes(rule.message));
      }
    }

    return {
      state: hasError ? 'error' as DebugNodeState : 'executed' as DebugNodeState,
      isCurrent,
      isExecuted: !hasError,
      isSkipped: false,
      hasError,
    };
  }, [dbgContext, task.id, task.function, workflow.id, ruleIndex]);
}

/**
 * Generic hook to get debug state for any node type
 */
export function useTreeNodeDebugState(options: {
  type: 'workflow' | 'workflow-condition' | 'task' | 'task-condition' | 'mapping' | 'validation-rule';
  workflow: Workflow;
  task?: Task;
}): TreeNodeDebugState {
  const { type, workflow, task } = options;

  // We need to call all hooks unconditionally to satisfy React's rules of hooks
  const workflowState = useWorkflowDebugState(workflow);
  const workflowConditionState = useWorkflowConditionDebugState(workflow);
  const taskState = useTaskDebugState(task || ({} as Task), workflow);
  const taskConditionState = useTaskConditionDebugState(task || ({} as Task), workflow);

  switch (type) {
    case 'workflow':
      return workflowState;
    case 'workflow-condition':
      return workflowConditionState;
    case 'task':
      return task ? taskState : nullState;
    case 'task-condition':
      return task ? taskConditionState : nullState;
    // Mappings and validation rules inherit from parent task
    case 'mapping':
    case 'validation-rule':
      return task ? taskState : nullState;
    default:
      return nullState;
  }
}
