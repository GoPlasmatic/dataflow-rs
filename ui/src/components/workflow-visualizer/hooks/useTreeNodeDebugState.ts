import { useMemo } from 'react';
import type { Workflow, Task, DebugNodeState, Context, Trace, TraceStep } from '../../../types';
import {
  getWorkflowState,
  getTaskState,
  isTaskCurrent,
  isWorkflowStep,
  findTaskStepIndex,
  stepErrors,
} from '../../../types';
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
 *
 * A trace records no condition evaluation of its own: the workflow's own
 * `skipped` step says the condition was false (or failed to evaluate, in
 * which case the step carries that failure's record), and any task step of
 * the workflow says it was true. A loop re-evaluates the condition every
 * sweep, so the latest of those at or before the current position decides;
 * but only a refusal before the first sweep records a step. A later sweep
 * the condition refused ends the loop with nothing beyond the counter
 * write, so this node cannot see it and keeps the last sweep's result.
 *
 * A workflow its rollout refused has the same `skipped` step, with no
 * condition behind it: only the run's record (`selection`) tells the two
 * apart, and the node then reports no result at all.
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

    const { trace, run, currentStepIndex } = dbgContext.state;

    let lastSkip = -1;
    let lastTask = -1;
    const end = Math.min(currentStepIndex, trace.steps.length - 1);
    for (let i = 0; i <= end; i++) {
      const s = trace.steps[i];
      if (s.workflow_id !== workflow.id) continue;
      if (isWorkflowStep(s)) {
        // The engine's steps (its loop's counter writes) and the workflow's
        // own failure are the workflow's too, but say nothing about its
        // condition.
        if (s.result === 'skipped') lastSkip = i;
      } else {
        lastTask = i;
      }
    }

    if (lastSkip === -1 && lastTask === -1) {
      return nullState;
    }

    if (lastSkip > lastTask) {
      // Skipped by the rollout, not by the condition: the condition was
      // never read, so there is no result to show, and the step is the
      // workflow's rather than this node's.
      const record = run?.workflows.find((w) => w.id === workflow.id);
      if (record?.selection === 'skipped_by_rollout') {
        return {
          state: 'skipped',
          isCurrent: false,
          isExecuted: false,
          isSkipped: true,
          hasError: false,
        };
      }

      const isCurrent = lastSkip === currentStepIndex;
      const hasError = stepErrors(trace.steps[lastSkip]).length > 0;
      return {
        state: isCurrent ? 'current' : hasError ? 'error' : 'skipped',
        isCurrent,
        isExecuted: false,
        isSkipped: true,
        hasError,
        conditionResult: false,
      };
    }

    return {
      state: 'executed',
      isCurrent: false,
      isExecuted: true,
      isSkipped: false,
      hasError: false,
      conditionResult: true,
    };
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
 *
 * A trace records no condition evaluation of its own: a task's `skipped`
 * step says its condition was false (or its function chose to skip, which
 * the wire does not tell apart), and a step that ran, whether it then
 * succeeded or failed, says it was true.
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

    // The current sweep's step, for a looping workflow — taking the first
    // match would pin this node to sweep 0 while the task node it sits under
    // advances, showing two iterations side by side.
    const stepIndex = findTaskStepIndex(trace, currentStepIndex, workflow.id, task.id);

    if (stepIndex === -1) {
      return nullState;
    }

    const taskStep = trace.steps[stepIndex];
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

    // Executed or failed: the condition admitted the task.
    return {
      state: 'executed',
      isCurrent,
      isExecuted: true,
      isSkipped: false,
      hasError: false,
      conditionResult: true,
    };
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
  }, [dbgContext, task.id, workflow.id]);
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

    // The current sweep's step, for the same reason as the task-condition
    // node above.
    const taskStepIndex = findTaskStepIndex(trace, currentStepIndex, workflow.id, task.id);

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

    // A validation puts one record per failed rule on its own step, with the
    // rule's message in it, so the step's records say which rules failed:
    // no diffing against the message's cumulative list, and no rule lights
    // up for an earlier workflow's error that happens to contain its text.
    const isCurrent = taskStepIndex === currentStepIndex;
    let hasError = false;
    const errors = stepErrors(taskStep);
    if (errors.length > 0) {
      const rules = (task.function?.input as Record<string, unknown> | undefined);
      const rulesList = (rules?.rules as Array<{ message: string }>) || [];
      const rule = rulesList[ruleIndex];
      if (rule) {
        // The engine records the rule's message verbatim, under this code.
        hasError = errors.some((e) => e.code === 'VALIDATION_ERROR' && e.message === rule.message);
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
 * The step of a task that the detail views read, with the trace and its
 * context before it and an on-demand mapping reader.
 */
export interface TaskDebugStep {
  trace: Trace;
  contextBefore: Context;
  mappingContextAt: (step: number, mapping: number) => Context | undefined;
  /** The step's index in `trace.steps` */
  stepIndex: number;
  step: TraceStep;
}

/**
 * The step of a task to read at the current position: the latest at or
 * before it (the current sweep's, for a looping workflow), so a detail view
 * shows what the task saw then and never a step the tree still marks
 * pending. `null` when the debugger is inactive, has no trace, or the task
 * has no step at or before the position. The step may be a skipped one: its
 * condition still read the context before it.
 */
export function useTaskDebugStep(task: Task, workflow: Workflow): TaskDebugStep | null {
  const dbgContext = useDebuggerOptional();

  return useMemo(() => {
    if (!dbgContext || !dbgContext.state.isActive || !dbgContext.state.trace) {
      return null;
    }
    const { trace, currentStepIndex } = dbgContext.state;
    const stepIndex = findTaskStepIndex(trace, currentStepIndex, workflow.id, task.id);
    if (stepIndex === -1 || stepIndex > currentStepIndex) {
      return null;
    }
    return {
      trace, stepIndex, step: trace.steps[stepIndex],
      contextBefore: dbgContext.contextAt(stepIndex - 1)!,
      mappingContextAt: dbgContext.mappingContextAt,
    };
  }, [dbgContext, task.id, workflow.id]);
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
