import { describe, expect, it } from 'vitest';
import {
  WasmEngine,
  contexts,
  mapping_contexts,
  type Context,
  type JsonValue,
  type RunOptions,
  type RunResult,
} from '@goplasmatic/dataflow-wasm';
import {
  contextAtStep,
  findTaskStepIndex,
  getTaskState,
  getWorkflowState,
  isEngineStep,
  isTaskCurrent,
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

/** A `map` task writing `value` at `path`, with any other task fields. */
function write(id: string, path: string, value: JsonValue, fields: Record<string, JsonValue> = {}): JsonValue {
  return { id, ...fields, function: { name: 'map', input: { mappings: [{ path, logic: value }] } } };
}

/** One `map` of three mappings; the second reads what the first wrote. */
const THREE_MAPPINGS: JsonValue[] = [
  {
    id: 'w',
    tasks: [
      {
        id: 'm',
        function: {
          name: 'map',
          input: {
            mappings: [
              { path: 'data.a', logic: 1 },
              { path: 'data.b', logic: { var: 'data.a' } },
              { path: 'data.c', logic: 3 },
            ],
          },
        },
      },
    ],
  },
];

async function run(
  workflows: JsonValue[],
  payload: JsonValue,
  options: RunOptions = { trace: true },
): Promise<RunResult> {
  const engine = new WasmEngine(workflows);
  try {
    return await engine.run(payload, options);
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

describe('a workflow its condition refused', () => {
  it("records one skipped step that is the workflow's own, and is skipped at it", async () => {
    const result = await run([{ id: 'w', condition: false, tasks: [write('t', 'data.a', 1)] }], {});
    expect(result.run.status).toBe('completed');
    expect(result.run.workflows[0].selection).toBe('skipped_by_condition');
    const trace = result.trace!;
    expect(trace.steps).toHaveLength(1);

    const [step] = trace.steps;
    expect(step.result).toBe('skipped');
    expect(step.task).toBeNull();
    expect(step.task_id).toBeNull();
    expect(isWorkflowStep(step)).toBe(true);
    expect(isEngineStep(step)).toBe(false);
    expect(stepState(step)).toBe('skipped');

    expect(getWorkflowState(trace, trace.steps.length - 1, 'w')).toBe('skipped');
    expect(getWorkflowState(trace, -1, 'w')).toBe('pending');
    expect(getTaskState(trace, trace.steps.length - 1, 'w', 't')).toBe('pending');
  });
});

describe('a task its condition refused', () => {
  it('records a skipped step with its id, and is skipped at it while the workflow ran', async () => {
    const result = await run(
      [{ id: 'w', tasks: [write('t', 'data.a', 1, { condition: false }), write('u', 'data.b', 2)] }],
      {},
    );
    expect(result.run.status).toBe('completed');
    expect(result.run.workflows[0].selection).toBe('ran');
    const trace = result.trace!;
    expect(trace.steps).toHaveLength(2);

    const t = findTaskStepIndex(trace, trace.steps.length - 1, 'w', 't');
    expect(t).toBe(0);
    expect(trace.steps[t].result).toBe('skipped');
    expect(trace.steps[t].task_id).toBe('t');
    expect(trace.steps[t].status).toBeNull();
    expect(isWorkflowStep(trace.steps[t])).toBe(false);
    expect(stepState(trace.steps[t])).toBe('skipped');

    expect(getTaskState(trace, t, 'w', 't')).toBe('skipped');
    expect(getTaskState(trace, t, 'w', 'u')).toBe('pending');
    expect(getTaskState(trace, t + 1, 'w', 'u')).toBe('executed');
    // A task's skip is not the workflow's.
    expect(getWorkflowState(trace, t, 'w')).toBe('pending');
    expect(getWorkflowState(trace, t + 1, 'w')).toBe('executed');
  });
});

describe('isTaskCurrent', () => {
  it("is true at the task's step and false one before", async () => {
    const result = await run(FAILING, {});
    const trace = result.trace!;
    const f = findTaskStepIndex(trace, trace.steps.length - 1, 'w', 'f');
    expect(f).toBeGreaterThan(0);
    expect(isTaskCurrent(trace, f, 'w', 'f')).toBe(true);
    expect(isTaskCurrent(trace, f - 1, 'w', 'f')).toBe(false);
    // The step before is the first task's.
    expect(isTaskCurrent(trace, f - 1, 'w', 'ok')).toBe(true);
    expect(isTaskCurrent(trace, -1, 'w', 'ok')).toBe(false);
  });
});

describe('traceIsReplayable', () => {
  it('is false for a trace that crossed its byte budget, and true for one that did not', async () => {
    const whole = await run(THREE_MAPPINGS, {});
    expect(whole.trace!.truncated).toBe(false);
    expect(traceIsReplayable(whole.trace!)).toBe(true);

    const cut = await run(THREE_MAPPINGS, {}, { trace: { max_bytes: 10 } });
    expect(cut.trace!.truncated).toBe(true);
    expect(traceIsReplayable(cut.trace!)).toBe(false);
    // The run and its steps are the same; only the changes are gone.
    expect(cut.run.status).toBe(whole.run.status);
    expect(cut.trace!.steps).toHaveLength(whole.trace!.steps.length);
    expect(cut.trace!.steps[0].changes).toHaveLength(0);
  });
});

describe('mapping_contexts', () => {
  it('gives the context after each mapping, so mapping j was evaluated against [j - 1]', async () => {
    const result = await run(THREE_MAPPINGS, {});
    const trace = result.trace!;
    const m = findTaskStepIndex(trace, trace.steps.length - 1, 'w', 'm');
    expect(trace.steps[m].mapping_ends).toHaveLength(3);

    const after = mapping_contexts(trace, m);
    expect(after).toHaveLength(3);
    const data = (context: Context) => context.data as Record<string, unknown>;
    // What mapping 1 read: mapping 0's write, and not yet its own.
    expect(data(after[0])).toHaveProperty('a', 1);
    expect(data(after[0])).not.toHaveProperty('b');
    // So it wrote `a`'s value, 1, to `b`.
    expect(data(after[1])).toHaveProperty('b', 1);
    expect(data(after[1])).not.toHaveProperty('c');
    // The last holds every mapping's write.
    expect(data(after[2])).toEqual({ a: 1, b: 1, c: 3 });
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
