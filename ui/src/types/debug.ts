/**
 * The debugger's types and the helpers that read a trace.
 *
 * The wire shapes are the engine's: `Message`, `Trace`, `Run` and the rest
 * come from `@goplasmatic/dataflow-wasm`, whose `.d.ts` is generated from
 * the dataflow-rs crate, and are re-exported here for consumers. This file
 * adds only what the debugger keeps beside them.
 *
 * A dataflow-rs 4 trace holds `initial`, the context before any workflow, and
 * one step per call, skip, failure and engine write, each with its changes.
 * There is no per-step message snapshot: the context at a step is rebuilt by
 * replaying the changes, which the engine does (`contexts(trace)` from the
 * wasm package) so that the UI and the engine never disagree on the rules.
 */
import type {
  Context,
  ErrorRecord,
  Issue,
  Message,
  Run,
  RunResult,
  Trace,
  TraceStep,
} from '@goplasmatic/dataflow-wasm';

export type {
  AuditEntry,
  AuditMode,
  Change,
  Context,
  DataflowError,
  EngineError,
  EngineFacts,
  ErrorRecord,
  Issue,
  JsonValue,
  Message,
  ParseResult,
  Run,
  RunOptions,
  RunResult,
  RunStatus,
  Selection,
  StepResult,
  Trace,
  TraceOptions,
  TraceStep,
  WorkflowExit,
  WorkflowRecord,
} from '@goplasmatic/dataflow-wasm';

/**
 * Why a run could not start: what the engine threw.
 *
 * `code` is the engine's (`"REJECTED"` for a definition that does not load,
 * with its `issues`; `"INVALID_ARGUMENT"` for a malformed argument), or
 * `"ERROR"` for anything else thrown on the way.
 */
export interface ExecutionError {
  code: string;
  message: string;
  issues?: Issue[];
}

/**
 * An `ExecutionError` from whatever was thrown: an `EngineError` keeps its
 * `code` and `issues`; any other `Error` its message.
 */
export function toExecutionError(err: unknown): ExecutionError {
  if (err instanceof Error) {
    const engineError = err as Error & { code?: unknown; issues?: unknown };
    return {
      code: typeof engineError.code === 'string' ? engineError.code : 'ERROR',
      message: err.message,
      ...(Array.isArray(engineError.issues) ? { issues: engineError.issues as Issue[] } : {}),
    };
  }
  return { code: 'ERROR', message: String(err) };
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
 * Playback state for the debugger
 */
export type PlaybackState = 'stopped' | 'playing' | 'paused';

/**
 * Complete debugger state
 */
export interface DebuggerState {
  /** Whether debug mode is active */
  isActive: boolean;
  /** The trace of the last run, when it was traced */
  trace: Trace | null;
  /** How the last run ended and what each workflow did */
  run: Run | null;
  /** The message after the last run */
  message: Message | null;
  /**
   * The context after each step of `trace`, replayed once per run through
   * the engine's `contexts`. Empty when the trace cannot be replayed.
   */
  contexts: Context[];
  /** Current step index being viewed; -1 is the ready state, before step 0 */
  currentStepIndex: number;
  /** Playback state */
  playbackState: PlaybackState;
  /** Playback speed (ms between steps) */
  playbackSpeed: number;
  /** Input payload for debugging */
  inputPayload: Record<string, unknown> | null;
  /** Whether the debugger is currently executing */
  isExecuting: boolean;
  /** Why the last run could not start, if it could not */
  executionError: ExecutionError | null;
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
  | { type: 'EXECUTE_RESULT'; result: RunResult; contexts: Context[] }
  | { type: 'EXECUTION_ERROR'; error: ExecutionError }
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
 * Whether a trace can be replayed: its byte budget was not crossed, so every
 * step carries its changes. A trace recorded with `changes: false` is not
 * truncated and replays to `initial` at every step, which is what it holds.
 */
export function traceIsReplayable(trace: Trace): boolean {
  return !trace.truncated;
}

/**
 * Whether a step is the workflow's own, not a task's: its skip, its
 * failure, or the engine's writes for its loop (see `isEngineStep`).
 */
export function isWorkflowStep(step: TraceStep): boolean {
  return step.task === null;
}

/**
 * Whether a step is the engine's: the writes of a loop's counter, `as` and
 * `scratch` before a sweep, or the counter write on which the loop exits
 * (its `max` reached, or a limit stopping the run). The step does not say
 * which: no sweep follows the last one, and a sweep the condition refused
 * records no step. It belongs to no task and skips no workflow.
 */
export function isEngineStep(step: TraceStep): boolean {
  return step.task === null && step.result === 'executed';
}

/**
 * The records a step added to the message: a `validation`'s per-rule
 * records, a function's own, then the engine's. Absent on the wire when
 * empty.
 */
export function stepErrors(step: TraceStep): ErrorRecord[] {
  return step.errors ?? [];
}

/**
 * The state a step shows: `failed` is an error (the failure went to the
 * workflow's policy or stopped the run), an executed step that recorded
 * errors is one too, and a skip is `skipped`.
 */
export function stepState(step: TraceStep): DebugNodeState {
  if (step.result === 'failed') return 'error';
  if (step.result === 'skipped') return 'skipped';
  return stepErrors(step).length > 0 ? 'error' : 'executed';
}

/**
 * The context at a step: `contexts[index]`, the context after that step, or
 * `trace.initial` before the first step (`index` -1) and when the replay has
 * nothing for the step.
 */
export function contextAtStep(trace: Trace, contexts: Context[], index: number): Context {
  if (index < 0) return trace.initial;
  return contexts[index] ?? trace.initial;
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
  trace: Trace,
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
  trace: Trace,
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

    // The workflow's own skipped step: its condition or its rollout refused
    // it. The engine's steps (its loop's counter writes) are the workflow's
    // too, but they are executed, not skipped.
    if (isWorkflowStep(step) && step.result === 'skipped') {
      skipped = true;
    }
    if (step.result === 'executed') {
      hasExecuted = true;
    }
    // A step's own records, not the message's cumulative `errors`: the latter
    // would report every workflow after the first failure as failed.
    if (!hasError && stepState(step) === 'error') {
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
  trace: Trace,
  currentStepIndex: number,
  workflowId: string,
  taskId: string
): DebugNodeState {
  // Find the step for this task — the current sweep's, for a looping workflow.
  const taskStepIndex = findTaskStepIndex(trace, currentStepIndex, workflowId, taskId);

  if (taskStepIndex === -1) {
    return 'pending';
  }

  // If task step is after current viewing position, it's pending
  if (taskStepIndex > currentStepIndex) {
    return 'pending';
  }

  return stepState(trace.steps[taskStepIndex]);
}

/**
 * Check if a task is the currently viewed step
 */
export function isTaskCurrent(
  trace: Trace,
  currentStepIndex: number,
  workflowId: string,
  taskId: string
): boolean {
  const step = trace.steps[currentStepIndex];
  return step?.workflow_id === workflowId && step?.task_id === taskId;
}
