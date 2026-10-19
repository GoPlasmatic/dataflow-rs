import { createElement, type ReactNode } from 'react';
import { act, renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import {
  debuggerReducer,
  getFilteredStepIndices,
  initialState,
  DebuggerProvider,
  useDebugger,
} from './DebuggerContext';
import type { EngineFactory, RunResult, Trace, Workflow } from '../../../types';

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
      replay: null,
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

  it('steps in the requested direction from a hidden step selected manually', () => {
    const state = {
      ...initialState,
      trace: sampleTrace,
      currentStepIndex: 1,
      skipFailedConditions: true,
    };
    expect(debuggerReducer(state, { type: 'STEP_FORWARD' }).currentStepIndex).toBe(2);
    expect(debuggerReducer(state, { type: 'STEP_BACKWARD' }).currentStepIndex).toBe(0);
  });

  it('does not start playback when no visible steps remain', () => {
    for (const trace of [
      { ...sampleTrace, steps: [] },
      { ...sampleTrace, steps: [sampleTrace.steps[1]] },
    ]) {
      const state = { ...initialState, trace, skipFailedConditions: true };
      expect(debuggerReducer(state, { type: 'PLAY' }).playbackState).toBe('stopped');
      expect(debuggerReducer({ ...state, playbackState: 'playing' }, { type: 'STEP_FORWARD' }).playbackState).toBe('paused');
    }
    const state = { ...initialState, trace: sampleTrace, currentStepIndex: 2 };
    expect(debuggerReducer(state, { type: 'PLAY' }).playbackState).toBe('stopped');
  });

  it('falls back to a previous visible step, or ready, when enabling the filter', () => {
    for (const [steps, index] of [
      [[sampleTrace.steps[0], sampleTrace.steps[1]], 0],
      [[sampleTrace.steps[1], sampleTrace.steps[1]], -1],
    ] as const) {
      const state = { ...initialState, trace: { ...sampleTrace, steps: [...steps] }, currentStepIndex: 1 };
      expect(debuggerReducer(state, { type: 'SET_SKIP_FAILED_CONDITIONS', skip: true }).currentStepIndex).toBe(index);
    }
  });
});

describe('debugger engine lifecycle', () => {
  it('replays selected contexts on demand and releases the cursor on reset', () => {
    const wrapper = ({ children }: { children: ReactNode }) =>
      createElement(DebuggerProvider, { children });
    const { result, unmount } = renderHook(() => useDebugger(), { wrapper });
    act(() => result.current.executeResult(sampleResult));
    expect(result.current.currentContext?.data).toEqual({ x: 0 });
    act(() => result.current.goToStep(2));
    expect(result.current.currentContext?.data).toEqual({ x: 2 });
    expect(result.current.contextAt(0)?.data).toEqual({ x: 1 });
    expect(result.current.contextAt(2)?.data).toEqual({ x: 2 });
    const free = vi.spyOn(result.current.state.replay!, 'free');
    act(() => result.current.reset());
    expect(free).toHaveBeenCalledTimes(1);
    expect(result.current.currentContext).toBeNull();
    unmount();
    expect(free).toHaveBeenCalledTimes(1);
  });

  it('reuses engines and rebuilds after a replacement fails without reusing a disposed handle', async () => {
    const run = vi.fn().mockResolvedValue(sampleResult);
    const dispose = vi.fn();
    const factory = vi.fn<EngineFactory>()
      .mockImplementationOnce(() => ({ run, dispose }))
      .mockImplementationOnce(() => { throw new Error('rejected definition'); })
      .mockImplementationOnce(() => ({ run, dispose }));
    const wrapper = ({ children }: { children: ReactNode }) =>
      createElement(DebuggerProvider, { engineFactory: factory, children });
    const { result, unmount } = renderHook(() => useDebugger(), { wrapper });
    const workflows: Workflow[] = [{ id: 'w', name: 'Valid', tasks: [] }];

    await result.current.runExecution(workflows, {});
    await result.current.runExecution(workflows, {});
    expect(factory).toHaveBeenCalledTimes(1);
    expect(run).toHaveBeenCalledWith({}, { trace: true });

    await expect(result.current.runExecution([{ id: 'bad', name: 'Invalid', tasks: [] }], {})).rejects.toThrow('rejected definition');
    expect(dispose).toHaveBeenCalledTimes(1);
    await result.current.runExecution(workflows, {});
    expect(factory).toHaveBeenCalledTimes(3);
    expect(run).toHaveBeenCalledTimes(3);
    unmount();
    expect(dispose).toHaveBeenCalledTimes(2);
  });
});
