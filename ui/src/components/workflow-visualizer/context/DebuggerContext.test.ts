import { describe, expect, it } from 'vitest';
import {
  debuggerReducer,
  getFilteredStepIndices,
  initialState,
} from './DebuggerContext';
import type { RunResult, Trace } from '../../../types';

const sampleTrace: Trace = {
  initial: { data: { x: 0 }, metadata: {}, temp_data: {} },
  steps: [
    {
      workflow: 0,
      task: 0,
      workflow_id: 'w1',
      task_id: 't1',
      result: 'executed',
      status: 200,
      changes: [{ path: 'data.x', old_value: 0, new_value: 1 }],
    },
    {
      workflow: 0,
      task: 1,
      workflow_id: 'w1',
      task_id: 't2',
      result: 'skipped',
      status: null,
      changes: [],
    },
    {
      workflow: 0,
      task: 2,
      workflow_id: 'w1',
      task_id: 't3',
      result: 'executed',
      status: 200,
      changes: [{ path: 'data.x', old_value: 1, new_value: 2 }],
    },
  ],
  truncated: false,
};

const sampleResult: RunResult = {
  message: {
    id: 'm1',
    payload: {},
    context: { data: { x: 2 }, metadata: {}, temp_data: {} },
    audit_trail: [],
    errors: [],
  },
  run: {
    status: 'completed',
    retryable: false,
    partial: false,
    matched: 1,
    workflows: [
      {
        id: 'w1',
        index: 0,
        selection: 'ran',
        sweeps: 1,
        exit: 'completed',
      },
    ],
  },
  trace: sampleTrace,
};

describe('debuggerReducer and getFilteredStepIndices', () => {
  it('filters skipped steps when skipFailedConditions is enabled', () => {
    expect(getFilteredStepIndices(sampleTrace, false)).toEqual([0, 1, 2]);
    expect(getFilteredStepIndices(sampleTrace, true)).toEqual([0, 2]);
  });

  it('transitions through execution, stepping, filtering, and reset', () => {
    let state = debuggerReducer(initialState, { type: 'ACTIVATE' });
    expect(state.isActive).toBe(true);

    state = debuggerReducer(state, { type: 'START_EXECUTION' });
    expect(state.isExecuting).toBe(true);

    state = debuggerReducer(state, {
      type: 'EXECUTE_RESULT',
      result: sampleResult,
      contexts: [
        { data: { x: 1 }, metadata: {}, temp_data: {} },
        { data: { x: 1 }, metadata: {}, temp_data: {} },
        { data: { x: 2 }, metadata: {}, temp_data: {} },
      ],
    });
    expect(state.isExecuting).toBe(false);
    expect(state.currentStepIndex).toBe(-1);
    expect(state.playbackState).toBe('paused');

    // Step forward through all steps
    state = debuggerReducer(state, { type: 'STEP_FORWARD' });
    expect(state.currentStepIndex).toBe(0);
    state = debuggerReducer(state, { type: 'STEP_FORWARD' });
    expect(state.currentStepIndex).toBe(1);

    // Enabling skipFailedConditions while on a skipped step jumps to next valid step
    state = debuggerReducer(state, { type: 'SET_SKIP_FAILED_CONDITIONS', skip: true });
    expect(state.currentStepIndex).toBe(2);

    // Stepping backward under skipFailedConditions skips index 1
    state = debuggerReducer(state, { type: 'STEP_BACKWARD' });
    expect(state.currentStepIndex).toBe(0);
    state = debuggerReducer(state, { type: 'STEP_BACKWARD' });
    expect(state.currentStepIndex).toBe(-1);

    // GO_TO_STEP and SET_SPEED clamp/validate
    state = debuggerReducer(state, { type: 'GO_TO_STEP', index: 2 });
    expect(state.currentStepIndex).toBe(2);
    state = debuggerReducer(state, { type: 'SET_SPEED', speed: 1 });
    expect(state.playbackSpeed).toBeGreaterThanOrEqual(100);

    // RESET clears trace and run
    state = debuggerReducer(state, { type: 'RESET' });
    expect(state.trace).toBeNull();
    expect(state.run).toBeNull();
    expect(state.currentStepIndex).toBe(-1);
  });
});
