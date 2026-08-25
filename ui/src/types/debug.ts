import type { JsonLogicValue } from './workflow';

/**
 * Message structure for workflow execution
 * Mirrors the Rust Message struct from WASM output
 */
export interface Message {
  /** Unique message ID */
  id: string;
  /**
   * Original payload, exactly as handed to the engine.
   *
   * Not necessarily an object: the WASM engine stores the payload as the raw
   * **string** it was given, leaving parsing to a `parse_json` / `parse_xml`
   * task. It is also not part of the JSONLogic evaluation context, which sees
   * `context` only.
   */
  payload: unknown;
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
  /**
   * Context path the error refers to, when the error is about one.
   * Always serialized, so it may be `null`.
   */
  path?: string | null;
  /**
   * Workflow ID where the error occurred.
   *
   * Absent for errors raised without executor identity — notably `validation`
   * failures and errors a custom handler adds through `TaskContext::add_error`.
   */
  workflow_id?: string;
  /** Task ID where the error occurred. Absent for the same reasons. */
  task_id?: string;
  /** RFC 3339 timestamp, when the producer set one. */
  timestamp?: string;
  /** Whether a retry was attempted, for errors produced by a retry loop. */
  retry_attempted?: boolean;
  /** Number of retries performed, for errors produced by a retry loop. */
  retry_count?: number;
  /**
   * Operator-only detail. Deliberately excluded from the error's `Display`
   * form, so treat it as diagnostic output rather than a user-facing message.
   */
  detail?: string;
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
  /**
   * Loop counter value for the sweep that produced this entry, when the
   * workflow carries a `loop`. Absent for non-looping workflows. Strictly
   * increasing across sweeps, so it both identifies the iteration and carries
   * its meaning (the array index, for per-item loops).
   */
  loop_counter?: number;
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
  /**
   * ID of the task.
   *
   * `null` for workflow-level skips — the Rust field is always serialized, so
   * the key is present either way. Test it for truthiness rather than with
   * `'task_id' in step` or `=== undefined`.
   */
  task_id?: string | null;
  /** Result of the step execution */
  result: StepResult;
  /**
   * Message snapshot after this step. Absent for skipped steps, when the host
   * ran with `TraceOptions { snapshots: false }`, and for executed steps
   * recorded after `max_snapshot_bytes` was exceeded.
   */
  message?: Message;
  /** Context snapshots before each mapping (map tasks only, trace mode) */
  mapping_contexts?: Record<string, unknown>[];
  /** RFC 3339 wall-clock start of the task body. Executed steps only. */
  started_at?: string;
  /** Task body duration in microseconds. Executed steps only. */
  duration_us?: number;
  /**
   * This task's own writes, when the host ran with `TraceOptions { changes: true }`.
   * Preferred over reading the last `audit_trail` entry, which mis-attributes
   * on a skip-outcome step.
   */
  changes?: Change[];
  /**
   * Loop counter of the sweep this step belongs to, when the workflow carries
   * a `loop`. Absent otherwise. Group steps by it to reconstruct per-iteration
   * execution.
   */
  loop_counter?: number;
}

/**
 * Complete execution trace (matches Rust ExecutionTrace)
 */
export interface ExecutionTrace {
  /** All execution steps in order */
  steps: ExecutionStep[];
  /**
   * Set when the host's snapshot budget was exceeded, meaning one or more
   * executed steps carry no `message`. Absent on a complete trace.
   */
  truncated?: boolean;
}

/**
 * Whether this trace carries message snapshots at all.
 *
 * A host running with `TraceOptions { snapshots: false }` (e.g. `timings_only()`)
 * produces steps with ids, result and timing but no state to inspect, so the
 * step-detail views have nothing to render.
 */
export function traceHasSnapshots(trace: ExecutionTrace): boolean {
  return trace.steps.some((s) => s.result === 'executed' && s.message !== undefined);
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
 * Get the changes made at a specific step.
 *
 * Prefers the step's own `changes` field, which the engine attributes to this
 * task exactly. Falls back to the last `audit_trail` entry for traces recorded
 * without `TraceOptions { changes: true }` — note that fallback mis-attributes
 * on a skip-outcome step, because a skip records no audit entry and the last
 * entry then belongs to an earlier task.
 */
export function getChangesAtStep(trace: ExecutionTrace, stepIndex: number): Change[] {
  const step = trace.steps[stepIndex];
  if (!step || step.result !== 'executed') {
    return [];
  }

  // Exact attribution when the host enabled it.
  if (step.changes !== undefined) {
    return step.changes;
  }

  // Fallback: the last audit_trail entry usually corresponds to this step.
  if (!step.message) {
    return [];
  }
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
 * Error count to diff `stepIndex`'s snapshot against, or `null` when there is
 * no usable baseline.
 *
 * A *skipped* step ran nothing and so introduced nothing: walk back past it.
 * An *executed* step with no snapshot is a hole punched by
 * `max_snapshot_bytes` — that budget is checked per step and is not monotone,
 * so a large snapshot is dropped while a later smaller one is still kept.
 * Errors raised in the hole are folded into this step's cumulative list and
 * cannot be separated out by counting, which is what `null` reports.
 */
function errorBaselineAt(trace: ExecutionTrace, stepIndex: number): number | null {
  for (let i = stepIndex - 1; i >= 0; i--) {
    const earlier = trace.steps[i];
    if (earlier.message) return earlier.message.errors.length;
    if (earlier.result === 'executed') return null;
  }
  return 0;
}

/** Whether an error belongs to this step by the ids its producer recorded. */
function matchesStepIds(error: ErrorInfo, step: ExecutionStep): boolean {
  return error.workflow_id === step.workflow_id && error.task_id === step.task_id;
}

/**
 * Errors that first appear at `stepIndex`.
 *
 * `Message.errors` is cumulative for the whole run, so a snapshot taken during
 * workflow B still carries workflow A's failures. Comparing against the most
 * recent earlier snapshot isolates the ones this step actually produced.
 *
 * This is also the only way to attribute a `validation` failure or an error a
 * custom handler added through `TaskContext::add_error`: those carry no
 * `workflow_id` / `task_id`, so they cannot be matched by id. Where a dropped
 * snapshot leaves no baseline, id matching is the fallback — exact for errors
 * that do carry ids, and silent rather than wrong for the ones that do not.
 *
 * Returns `[]` when snapshots are disabled, since there is then nothing to diff.
 */
export function errorsIntroducedAt(trace: ExecutionTrace, stepIndex: number): ErrorInfo[] {
  const step = trace.steps[stepIndex];
  if (!step?.message) return [];

  const baseline = errorBaselineAt(trace, stepIndex);
  return baseline === null
    ? step.message.errors.filter(e => matchesStepIds(e, step))
    : step.message.errors.slice(baseline);
}

/**
 * How many errors first appear at `stepIndex`.
 *
 * The allocation-free form of [`errorsIntroducedAt`], for callers that only
 * ask whether the step introduced anything. `getWorkflowState` runs this once
 * per step of the workflow on every playback tick.
 */
export function errorCountIntroducedAt(trace: ExecutionTrace, stepIndex: number): number {
  const step = trace.steps[stepIndex];
  if (!step?.message) return 0;

  const errors = step.message.errors;
  const baseline = errorBaselineAt(trace, stepIndex);
  if (baseline !== null) return errors.length - baseline;

  let count = 0;
  for (const error of errors) {
    if (matchesStepIds(error, step)) count++;
  }
  return count;
}

/**
 * Index of the step to render for a task, given where playback is.
 *
 * A looping workflow records one step per task *per sweep*, all sharing the
 * same `(workflow_id, task_id)`. Taking the first match would pin every node to
 * sweep 0, so prefer the most recent sweep at or before the current position
 * and fall back to the next upcoming one.
 *
 * Returns `-1` when the task has no step in the trace.
 */
export function findTaskStepIndex(
  trace: ExecutionTrace,
  currentStepIndex: number,
  workflowId: string,
  taskId: string
): number {
  let lastAtOrBefore = -1;
  let firstAfter = -1;

  for (let i = 0; i < trace.steps.length; i++) {
    const s = trace.steps[i];
    if (s.workflow_id !== workflowId || s.task_id !== taskId) continue;
    if (i <= currentStepIndex) {
      lastAtOrBefore = i;
    } else if (firstAfter === -1) {
      firstAfter = i;
    }
  }

  return lastAtOrBefore !== -1 ? lastAtOrBefore : firstAfter;
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
  // One pass over the trace, collecting the indices this workflow owns. Every
  // later question is answered from those indices directly — re-deriving them
  // with `trace.steps.indexOf(step)` inside a predicate is what made this
  // quadratic in trace length, on a function that re-runs on every tick.
  const workflowStepIndices: number[] = [];
  for (let i = 0; i < trace.steps.length; i++) {
    if (trace.steps[i].workflow_id === workflowId) {
      workflowStepIndices.push(i);
    }
  }

  if (workflowStepIndices.length === 0) {
    return 'pending';
  }

  // Indices are collected in order, so the first is the smallest.
  if (workflowStepIndices[0] > currentStepIndex) {
    return 'pending';
  }

  let skipped = false;
  let hasError = false;
  let hasExecuted = false;

  for (const stepIndex of workflowStepIndices) {
    // Ascending, so everything from here on is after the current position.
    if (stepIndex > currentStepIndex) {
      break;
    }
    const step = trace.steps[stepIndex];

    // No task_id means a workflow-level skip.
    if (!step.task_id && step.result === 'skipped') {
      skipped = true;
    }
    if (step.result === 'executed') {
      hasExecuted = true;
    }
    // Whether this step introduced an error *of its own*. Testing
    // `errors.length > 0` would report every workflow after the first failure
    // as failed, since `Message.errors` is cumulative.
    if (!hasError && errorCountIntroducedAt(trace, stepIndex) > 0) {
      hasError = true;
    }
  }

  if (skipped) return 'skipped';
  if (hasError) return 'error';
  if (hasExecuted) return 'executed';
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
  // Find the step for this task — the current sweep's, for a looping workflow.
  const taskStepIndex = findTaskStepIndex(trace, currentStepIndex, workflowId, taskId);

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
    // Errors this step introduced, rather than every error accumulated so far.
    // Matching on `error.task_id` alone would miss `validation` failures, which
    // carry no ids.
    if (errorCountIntroducedAt(trace, taskStepIndex) > 0) {
      return 'error';
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
  const step = trace.steps[currentStepIndex];
  return step?.workflow_id === workflowId && step?.task_id === taskId;
}
