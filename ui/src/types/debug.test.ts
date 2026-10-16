import { describe, expect, it } from 'vitest';
import { WasmEngine, contexts, type JsonValue, type RunResult } from '@goplasmatic/dataflow-wasm';
import {
  contextAtStep,
  findTaskStepIndex,
  getTaskState,
  getWorkflowState,
  isEngineStep,
  isWorkflowStep,
  stepErrors,
  stepState,
  toExecutionError,
  traceIsReplayable,
} from './debug';

/**
 * The first task writes a string, the second appends to it, which fails the
 * mapping and, under the workflow's default policy, stops the run; the third
 * never runs.
 */
const FAILING: JsonValue[] = [
  {
    id: 'w',
    tasks: [
      { id: 'ok', function: { name: 'map', input: { mappings: [{ path: 'data.s', logic: 'str' }] } } },
      { id: 'f', function: { name: 'map', input: { mappings: [{ path: 'data.s', logic: 'x', mode: 'append' }] } } },
      { id: 'never', function: { name: 'map', input: { mappings: [{ path: 'data.z', logic: 1 }] } } },
    ],
  },
];

/** Three sweeps; the validation's rule fails on the third (`i` is 2). */
const LOOPING: JsonValue[] = [
  {
    id: 'loop',
    loop: { counter: 'i', max: 3 },
    tasks: [
      {
        id: 'v',
        function: {
          name: 'validation',
          input: {
            rules: [
              { logic: { '<': [{ var: 'temp_data.i' }, 2] }, path: 'temp_data.i', message: 'i must be under 2' },
            ],
          },
        },
      },
    ],
  },
];

async function run(workflows: JsonValue[], payload: JsonValue): Promise<RunResult> {
  const engine = new WasmEngine(workflows);
  try {
    return await engine.run(payload, { trace: true });
  } finally {
    engine.free();
  }
}

describe('a failing run', () => {
  it('replays to the final context and marks the failed task an error', async () => {
    const result = await run(FAILING, {});
    expect(result.run.status).toBe('stopped');
    expect(result.run.workflows[0].exit).toBe('stopped');
    const trace = result.trace!;
    expect(traceIsReplayable(trace)).toBe(true);

    const replayed = contexts(trace);
    const last = trace.steps.length - 1;
    expect(contextAtStep(trace, replayed, last)).toEqual(result.message.context);
    expect(contextAtStep(trace, replayed, -1)).toEqual(trace.initial);

    const f = findTaskStepIndex(trace, last, 'w', 'f');
    expect(trace.steps[f].result).toBe('failed');
    expect(stepState(trace.steps[f])).toBe('error');
    expect(stepErrors(trace.steps[f]).length).toBeGreaterThan(0);
    expect(isWorkflowStep(trace.steps[f])).toBe(false);

    expect(getTaskState(trace, last, 'w', 'ok')).toBe('executed');
    expect(getTaskState(trace, last, 'w', 'f')).toBe('error');
    expect(getTaskState(trace, last, 'w', 'never')).toBe('pending');
    expect(getWorkflowState(trace, last, 'w')).toBe('error');
    // Before the failing step is reached, the task is still pending.
    expect(getTaskState(trace, f - 1, 'w', 'f')).toBe('pending');
    expect(getWorkflowState(trace, f - 1, 'w')).toBe('executed');
  });
});

describe('a looping run', () => {
  it("has engine steps that are no task's and skip no workflow, and puts a rule's failure on its step", async () => {
    const result = await run(LOOPING, {});
    expect(result.run.status).toBe('completed_with_errors');
    const trace = result.trace!;

    const engineSteps = trace.steps.filter(isEngineStep);
    expect(engineSteps.length).toBeGreaterThan(0);
    for (const step of engineSteps) {
      expect(isWorkflowStep(step)).toBe(true);
      expect(step.task).toBeNull();
      expect(stepState(step)).toBe('executed');
      expect(typeof step.loop_counter).toBe('number');
    }

    const sweeps = trace.steps
      .map((step, index) => ({ step, index }))
      .filter(({ step }) => step.task_id === 'v');
    expect(sweeps).toHaveLength(3);
    const [first, , third] = sweeps;
    expect(first.step.status).toBe(200);
    expect(third.step.status).toBe(400);
    expect(stepErrors(third.step).some((e) => e.message.includes('i must be under 2'))).toBe(true);

    expect(getTaskState(trace, first.index, 'loop', 'v')).toBe('executed');
    expect(getTaskState(trace, third.index, 'loop', 'v')).toBe('error');
    const last = trace.steps.length - 1;
    expect(getWorkflowState(trace, first.index, 'loop')).toBe('executed');
    expect(getWorkflowState(trace, last, 'loop')).toBe('error');
    // An engine step neither skips the workflow nor belongs to the task.
    expect(getTaskState(trace, engineSteps.length > 0 ? trace.steps.indexOf(engineSteps[0]) : 0, 'loop', 'v')).not.toBe('skipped');
  });
});

describe('toExecutionError', () => {
  it("keeps an EngineError's code and issues, and wraps any other error", () => {
    let thrown: unknown;
    try {
      new WasmEngine([{ id: 'w', tasks: [{ id: 't', function: { name: 'nope', input: {} } }] }]);
    } catch (e) {
      thrown = e;
    }
    const error = toExecutionError(thrown);
    expect(error.code).toBe('REJECTED');
    expect(error.issues?.[0].code).toBe('UNKNOWN_FUNCTION');
    expect(error.issues?.[0].path).toBe('tasks[0].function.name');

    expect(toExecutionError(new Error('x'))).toEqual({ code: 'ERROR', message: 'x' });
    expect(toExecutionError('plain')).toEqual({ code: 'ERROR', message: 'plain' });
  });
});
