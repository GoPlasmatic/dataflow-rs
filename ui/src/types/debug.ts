import type { JsonLogicValue } from './workflow';

/**
 * Message structure for workflow execution
 * Mirrors the Rust Message struct from WASM output
 */
export interface Message {
  /** Unique message ID */
  id: string;
  /** Original payload */
  payload: Record<string, unknown>;
  /** Context containing data, metadata, temp_data */
  context: {
    data: Record<string, unknown>;
    metadata: Record<string, unknown>;
    temp_data: Record<string, unknown>;
  };
  /** List of errors that occurred during processing */
  errors: ErrorInfo[];
  /** Audit trail of changes made during processing */
  audit_trail: AuditTrail[];
}

/**
 * Error information captured during execution
 */
export interface ErrorInfo {
  /** Error code or identifier */
  code: string;
  /** Human-readable error message */
  message: string;
  /** Task ID where error occurred */
  task_id?: string;
  /** Workflow ID where error occurred */
  workflow_id?: string;
}

/**
 * Audit trail entry for tracking changes (matches Rust AuditTrail)
 */
export interface AuditTrail {
  /** Workflow ID where change occurred */
  workflow_id: string;
  /** Task ID where change occurred */
  task_id: string;
  /** Timestamp of the change */
  timestamp: string;
  /** Changes made by the task */
  changes: Change[];
  /** Status code (e.g., 200 for success) */
  status: number;
}

/**
 * A single change in the audit trail
 */
export interface Change {
  /** Path to the changed field (e.g., "data.user.name") */
  path: string;
  /** Previous value */
  old_value: unknown;
  /** New value */
  new_value: unknown;
}

/**
 * Result of a step (executed or skipped)
 */
export type StepResult = 'executed' | 'skipped';

/**
 * A single step in the execution trace (matches Rust ExecutionStep)
 */
export interface ExecutionStep {
  /** ID of the workflow this step belongs to */
  workflow_id: string;
  /** ID of the task (undefined for workflow-level skips) */
  task_id?: string;
  /** Result of the step execution */
  result: StepResult;
  /** Message snapshot after this step (only for executed steps) */
  message?: Message;
  /** Context snapshots before each mapping (map tasks only, trace mode) */
  mapping_contexts?: Record<string, unknown>[];
}

/**
 * Complete execution trace (matches Rust ExecutionTrace)
 */
export interface ExecutionTrace {
  /** All execution steps in order */
  steps: ExecutionStep[];
}

/**
 * Debug state for a node in the execution tree
 */
export type DebugNodeState =
  | 'pending'    // Not yet processed
  | 'current'    // Currently being viewed
  | 'executed'   // Successfully executed
  | 'skipped'    // Skipped due to condition
  | 'error';     // Execution failed

/**
 * Result of evaluating a condition
 */
export interface ConditionResult {
  /** The condition that was evaluated */
  condition: JsonLogicValue;
  /** The result of evaluation */
  result: boolean;
  /** Data context used for evaluation */
  context: Record<string, unknown>;
}

/**
 * Playback state for the debugger
 */
export type PlaybackState = 'stopped' | 'playing' | 'paused';

/**
 * Complete debugger state
 */
export interface DebuggerState {
  /** Whether debug mode is active */
  isActive: boolean;
  /** Current execution trace */
  trace: ExecutionTrace | null;
  /** Current step index being viewed */
  currentStepIndex: number;
  /** Playback state */
  playbackState: PlaybackState;
  /** Playback speed (ms between steps) */
  playbackSpeed: number;
  /** Input payload for debugging */
  inputPayload: Record<string, unknown> | null;
  /** Whether the debugger is currently executing */
  isExecuting: boolean;
  /** Error during execution */
  executionError: string | null;
  /** Whether to skip steps with failed conditions (result: 'skipped') */
  skipFailedConditions: boolean;
}

/**
 * Actions for the debugger reducer
 */
export type DebuggerAction =
  | { type: 'ACTIVATE' }
  | { type: 'DEACTIVATE' }
  | { type: 'SET_INPUT_PAYLOAD'; payload: Record<string, unknown> }
  | { type: 'START_EXECUTION' }
  | { type: 'EXECUTE_TRACE'; trace: ExecutionTrace }
  | { type: 'EXECUTION_ERROR'; error: string }
  | { type: 'PLAY' }
  | { type: 'PAUSE' }
  | { type: 'STOP' }
  | { type: 'RESET' }
  | { type: 'STEP_FORWARD' }
  | { type: 'STEP_BACKWARD' }
  | { type: 'GO_TO_STEP'; index: number }
  | { type: 'SET_SPEED'; speed: number }
  | { type: 'SET_SKIP_FAILED_CONDITIONS'; skip: boolean };

/**
 * Create an empty message
 */
export function createEmptyMessage(): Message {
  return {
    id: '',
    payload: {},
    context: {
      data: {},
      metadata: {},
      temp_data: {},
    },
    errors: [],
    audit_trail: [],
  };
}

/**
 * Deep clone a message
 */
export function cloneMessage(message: Message): Message {
  return JSON.parse(JSON.stringify(message));
}

/**
 * Get the message at a specific step in the trace
 * Returns the message snapshot from the last executed step at or before the given index
 */
export function getMessageAtStep(trace: ExecutionTrace, stepIndex: number): Message | null {
  // Find the last executed step at or before the given index
  for (let i = stepIndex; i >= 0; i--) {
    const step = trace.steps[i];
    if (step.result === 'executed' && step.message) {
      return step.message;
    }
  }
  return null;
}

/**
 * Get the changes made at a specific step
 * Returns the changes from the last audit_trail entry if step was executed
 */
export function getChangesAtStep(trace: ExecutionTrace, stepIndex: number): Change[] {
  const step = trace.steps[stepIndex];
  if (!step || step.result !== 'executed' || !step.message) {
    return [];
  }

  // The last audit_trail entry corresponds to this step's task execution
  const auditTrail = step.message.audit_trail;
  if (auditTrail.length === 0) {
    return [];
  }

  return auditTrail[auditTrail.length - 1].changes;
}

/**
 * Check if a step is for a specific workflow
 */
export function isStepForWorkflow(step: ExecutionStep, workflowId: string): boolean {
  return step.workflow_id === workflowId;
}

/**
 * Check if a step is for a specific task
 */
export function isStepForTask(step: ExecutionStep, workflowId: string, taskId: string): boolean {
  return step.workflow_id === workflowId && step.task_id === taskId;
}

/**
 * Get the context snapshot for a specific mapping within a step.
 * For map tasks, returns the context before that mapping executed.
 * Falls back to the step's message context if mapping_contexts is not available.
 */
export function getMappingContext(
  step: ExecutionStep,
  mappingIndex: number
): Record<string, unknown> | undefined {
  return (step.mapping_contexts?.[mappingIndex] as Record<string, unknown>) ?? step.message?.context;
}

/**
 * Get the state of a workflow based on the trace and current step
 * Returns:
 * - 'pending' if all workflow steps are after the current step
 * - 'executed'/'skipped'/'error' for workflows with steps at or before current
 * Note: Workflows don't show as 'current' - only individual tasks do
 */
export function getWorkflowState(
  trace: ExecutionTrace,
  currentStepIndex: number,
  workflowId: string
): DebugNodeState {
  // Find all step indices for this workflow
  const workflowStepIndices: number[] = [];
  trace.steps.forEach((s, idx) => {
    if (s.workflow_id === workflowId) {
      workflowStepIndices.push(idx);
    }
  });

  if (workflowStepIndices.length === 0) {
    return 'pending';
  }

  const firstStepIndex = Math.min(...workflowStepIndices);

  // If all workflow steps are after the current step, workflow is pending
  if (firstStepIndex > currentStepIndex) {
    return 'pending';
  }

  // Check actual results for steps at or before current position
  const workflowSteps = trace.steps.filter(s => s.workflow_id === workflowId);

  // Check if the workflow was skipped (no task_id means workflow-level skip)
  const workflowSkipStep = workflowSteps.find(s => !s.task_id && s.result === 'skipped');
  if (workflowSkipStep) {
    const skipIndex = trace.steps.indexOf(workflowSkipStep);
    if (skipIndex <= currentStepIndex) {
      return 'skipped';
    }
  }

  // Check if any step has an error (only steps at or before current)
  const hasError = workflowSteps.some(s => {
    const stepIndex = trace.steps.indexOf(s);
    return stepIndex <= currentStepIndex && s.message && s.message.errors.length > 0;
  });
  if (hasError) {
    return 'error';
  }

  // Check if any task was executed (only steps at or before current)
  const hasExecuted = workflowSteps.some(s => {
    const stepIndex = trace.steps.indexOf(s);
    return stepIndex <= currentStepIndex && s.result === 'executed';
  });
  if (hasExecuted) {
    return 'executed';
  }

  return 'pending';
}

/**
 * Get the state of a task based on the trace and current step
 * Returns:
 * - 'pending' for future steps (after current)
 * - 'executed'/'skipped'/'error' for steps at or before current position
 * Note: Tasks show as 'executed' when on their step (completed state)
 */
export function getTaskState(
  trace: ExecutionTrace,
  currentStepIndex: number,
  workflowId: string,
  taskId: string
): DebugNodeState {
  // Find the step for this task
  const taskStepIndex = trace.steps.findIndex(
    s => s.workflow_id === workflowId && s.task_id === taskId
  );

  if (taskStepIndex === -1) {
    return 'pending';
  }

  const taskStep = trace.steps[taskStepIndex];

  // If task step is after current viewing position, it's pending
  if (taskStepIndex > currentStepIndex) {
    return 'pending';
  }

  // For steps at or before current position, show actual result
  if (taskStep.result === 'skipped') {
    return 'skipped';
  }

  if (taskStep.result === 'executed') {
    // Check for errors in the message
    if (taskStep.message && taskStep.message.errors.length > 0) {
      // Check if error is from this task
      const taskError = taskStep.message.errors.find(
        e => e.task_id === taskId && e.workflow_id === workflowId
      );
      if (taskError) {
        return 'error';
      }
    }
    return 'executed';
  }

  return 'pending';
}

/**
 * Check if a task is the currently viewed step
 */
export function isTaskCurrent(
  trace: ExecutionTrace,
  currentStepIndex: number,
  workflowId: string,
  taskId: string
): boolean {
  const taskStepIndex = trace.steps.findIndex(
    s => s.workflow_id === workflowId && s.task_id === taskId
  );
  return taskStepIndex === currentStepIndex;
}
