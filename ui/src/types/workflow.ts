import {
  ArrowRightLeft,
  Box,
  CheckCircle,
  FileCode,
  FileJson,
  Filter,
  ScrollText,
  Send,
  type LucideIcon,
} from 'lucide-react';
import { engineFacts } from '../engines/engineFacts';

/**
 * JSONLogic value type - can be any valid JSON value or JSONLogic expression
 */
export type JsonLogicValue =
  | string
  | number
  | boolean
  | null
  | JsonLogicValue[]
  | { [key: string]: JsonLogicValue };

/**
 * Function configuration for a task
 */
export interface FunctionConfig {
  /** Function name (e.g., "map", "validation", or custom) */
  name: string;
  /**
   * Function-specific input configuration.
   *
   * The engine reads an absent `input` as `{}`. The UI keeps it required so
   * every renderer can read it without a guard; a function that takes no
   * configuration is written with `input: {}`.
   */
  input: Record<string, unknown>;
}

/**
 * Task definition within a workflow
 */
export interface Task {
  /** Unique identifier for the task */
  id: string;
  /** Human-readable name */
  name: string;
  /** Optional description */
  description?: string;
  /** JSONLogic condition (evaluated against full context: data, metadata, temp_data) */
  condition?: JsonLogicValue;
  /** Function to execute */
  function: FunctionConfig;
  /**
   * What a failure (an error or a 5xx status) does. Defaults to `"fail"`.
   *
   * `"fail"` passes the failure to the workflow's `on_failure`; `"continue"`
   * records it and runs the next task; `"halt"` ends this workflow, and the
   * run goes on.
   */
  on_failure?: 'fail' | 'continue' | 'halt';
  /**
   * What a rejection (a 4xx status, which a failing `validation` returns)
   * does. Defaults to `"continue"`: the rejection is recorded and the next
   * task runs. `"halt"` ends this workflow, and the run goes on.
   */
  on_rejection?: 'continue' | 'halt';
  /**
   * The 3.x spelling of `on_failure: "continue"` (and, alone, of
   * `on_rejection: "continue"`). The engine still reads it, rewriting it on
   * load; new definitions write `on_failure`. Render through
   * {@link taskContinuesOnFailure}, which reads both.
   */
  continue_on_error?: boolean;
  /**
   * End the workflow once this task has run. Defaults to `false`.
   *
   * A statement about *position*, not outcome: a false `condition` or a
   * `Skip` outcome does not halt, but a task that failed under
   * `on_failure: "continue"` still does. Halting stops this workflow only.
   */
  terminal?: boolean;
  /**
   * The 3.x spelling of `on_rejection: "halt"` (and, beside
   * `continue_on_error: true`, of `on_failure: "halt"`). `"failure"` halted
   * on a recorded status of 400 or above; the engine still reads it,
   * rewriting it on load. Render through {@link taskHaltsOnFailure}, which
   * reads both spellings.
   */
  halt_on?: 'never' | 'failure';
  /**
   * Run this task's function once per element of an array. Absent runs it
   * once. Refused on a built-in, which runs inline; a host's function may
   * fan out.
   */
  for_each?: ForEach;
  /**
   * Host and UI data the engine keeps verbatim and never reads. The engine's
   * reader also moves a key it does not know here.
   */
  ext?: Record<string, unknown>;
}

/**
 * Whether a task's failure is recorded and the next task runs:
 * `on_failure: "continue"`, or 3.x's `continue_on_error: true` on its own.
 * Beside `halt_on: "failure"` that flag reads as `on_failure: "halt"`
 * instead, which {@link taskHaltsOnFailure} renders.
 *
 * The 3.x spellings are read only for a definition the compat reader refused,
 * which `migrateDefinitions` keeps as authored: one it accepts already arrives
 * as `on_failure`.
 */
export function taskContinuesOnFailure(task: Task): boolean {
  return (
    task.on_failure === 'continue' ||
    (task.continue_on_error === true && task.halt_on !== 'failure')
  );
}

/**
 * Whether the task's failing ends its workflow: `on_rejection: "halt"` (a
 * 4xx, which a failing `validation` returns), `on_failure: "halt"` (an error
 * or a 5xx), or, for a definition the reader refused, 3.x's `halt_on:
 * "failure"`, which the engine reads as the former, or as both beside
 * `continue_on_error: true`.
 */
export function taskHaltsOnFailure(task: Task): boolean {
  return (
    task.on_rejection === 'halt' || task.on_failure === 'halt' || task.halt_on === 'failure'
  );
}

/**
 * A task's fan-out. Mirrors `ForEachDef` in the engine's `src/def/types.rs`.
 *
 * Every call runs against its own copy of the message, and the calls are
 * folded back in element order — so `max_concurrency` changes timing, never
 * results.
 */
export interface ForEach {
  /** JSONLogic evaluated once. Must yield an array. */
  over: JsonLogicValue;
  /** `temp_data` field holding the element in each call; the index is at `<as>_index`. */
  as: string;
  /**
   * Calls in flight at once. Absent takes the engine's default
   * (`engineFacts().for_each_defaults.max_concurrency`, 1).
   */
  max_concurrency?: number;
  /** Context path each call writes its result to. Requires `into`. */
  collect?: string;
  /** Context path receiving the results as an array, `null` for a failed element. */
  into?: string;
}

/**
 * A contiguous run of tasks sharing one condition.
 *
 * Mirrors `GroupDef` in the engine's `src/def/types.rs`. In a workflow's
 * `tasks` array a group is an element carrying a `tasks` key; a plain task
 * carries `function`. The group condition is evaluated **once, on entry** — a
 * false result skips the whole span without evaluating the members' own
 * conditions.
 */
export interface TaskGroup {
  /** Unique identifier, sharing the task ID namespace */
  id: string;
  /** Human-readable name */
  name?: string;
  /** Optional description */
  description?: string;
  /** JSONLogic condition gating the whole span (evaluated once on entry) */
  condition?: JsonLogicValue;
  /** End the workflow once the group completes */
  terminal?: boolean;
  /**
   * **Not honoured on a group.** Error handling is per task and per workflow;
   * a group only gates a span, and has no policy key in the v4 spelling. The
   * engine's reader moves this 3.x key into the group's `ext` and reports a
   * `true` as `GROUP_CONTINUE_ON_ERROR`. Present here so a tool can surface
   * what the author wrote.
   */
  continue_on_error?: boolean;
  /** The nested steps. Must not be empty. */
  tasks: Step[];
  /**
   * Host and UI data the engine keeps verbatim and never reads. The engine's
   * reader also moves a key it does not know here.
   */
  ext?: Record<string, unknown>;
}

/** One element of a workflow's `tasks` array: a task or a group of them. */
export type Step = Task | TaskGroup;

/**
 * Whether a step is a group rather than a plain task.
 *
 * The test is **presence of a `tasks` key, nothing else** — the same test the
 * engine's parser makes (`src/def/parse.rs`: an element carrying `tasks` is
 * a group). A `tasks` key holding a non-array is still a group, and a
 * malformed one; reading it as a task instead would classify it differently
 * from the engine that has to run it.
 */
export function isTaskGroup(step: Step): step is TaskGroup {
  return (step as TaskGroup).tasks !== undefined;
}

/**
 * The members of a group, or none when `tasks` is malformed.
 *
 * `isTaskGroup` tests only for the presence of the key, so a group whose
 * `tasks` is not an array is still a group — with no members. The engine's
 * parser refuses such a workflow; a renderer must not iterate the value (a
 * string would iterate character by character). Mirrors the parser, which
 * descends only into a real array.
 */
export function groupMembers(group: TaskGroup): Step[] {
  return Array.isArray(group.tasks) ? group.tasks : [];
}

/**
 * The leaf tasks of a step tree, in document order — what the engine actually
 * runs, and what task-counting or task-lookup consumers want.
 */
export function flattenSteps(steps: Step[]): Task[] {
  const out: Task[] = [];
  for (const step of steps) {
    if (isTaskGroup(step)) {
      out.push(...flattenSteps(groupMembers(step)));
    } else {
      out.push(step);
    }
  }
  return out;
}

/**
 * How many leaf tasks a step tree holds.
 *
 * The same number as `flattenSteps(steps).length`, without building the list —
 * for counters that render on every parent update.
 */
export function countLeafSteps(steps: Step[]): number {
  let count = 0;
  for (const step of steps) {
    count += isTaskGroup(step) ? countLeafSteps(groupMembers(step)) : 1;
  }
  return count;
}

/**
 * Every leaf task in a workflow, in execution order: `loop.setup` steps first,
 * then `tasks`.
 */
export function workflowTasks(workflow: Workflow): Task[] {
  return [
    ...flattenSteps(workflow.loop?.setup ?? []),
    ...flattenSteps(workflow.tasks ?? []),
  ];
}

/**
 * Whether a condition is active (present and not trivially `true`).
 */
export function hasCondition(condition: unknown): condition is JsonLogicValue {
  return condition !== undefined && condition !== null && condition !== true;
}

/**
 * Engine-managed `for` loop over a workflow's task list.
 *
 * Mirrors `LoopDef` in the engine's `src/def/types.rs`. A workflow carrying a
 * `loop` runs its task list once per *sweep* rather than once. Per sweep the
 * engine writes the counter into `temp_data`, checks `counter < max`
 * (half-open), re-evaluates the workflow condition, runs the task list, then
 * advances the counter by `increment`.
 *
 * Both exits — reaching `max` and the condition going false — are normal
 * completion, never an error.
 */
export interface LoopConfig {
  /**
   * `temp_data` field the engine maintains as the induction variable. `"i"`
   * means `temp_data.i`; dot-paths nest (`"cursor.index"` →
   * `temp_data.cursor.index`). Absent still bounds the loop by `max`; the
   * count is simply not exposed to conditions or tasks.
   */
  counter?: string;
  /**
   * First counter value. Absent takes the engine's default
   * (`engineFacts().loop_defaults.init`, 0).
   */
  init?: number;
  /**
   * Added to the counter after each sweep; must be >= 1. Absent takes the
   * engine's default (`engineFacts().loop_defaults.increment`, 1).
   */
  increment?: number;
  /**
   * Required upper bound — sweeps run while `counter < max`. Half-open, so
   * `init: 0, max: n` yields `0..n-1`, exactly array indices.
   */
  max: number;
  /**
   * Steps run once, before the first sweep, in the normal step grammar
   * (groups allowed). Not a sweep: their trace steps and audit entries carry
   * no `loop_counter`. Shares the step id namespace with `tasks`.
   */
  setup?: Step[];
  /**
   * JSONLogic evaluated once, after `setup`. Must yield an array — anything
   * else, `null` included, is a workflow error. The counter indexes it, so the
   * loop also stops at the array's end.
   */
  over?: JsonLogicValue;
  /** `temp_data` field holding the current element of `over`. Requires `over`. */
  as?: string;
  /** `temp_data` field reset to `{}` at the start of every sweep. */
  scratch?: string;
}

/**
 * Workflow definition
 */
export interface Workflow {
  /** Unique identifier for the workflow */
  id: string;
  /** Human-readable name */
  name: string;
  /** Execution priority (lower = higher priority, 0 is highest) */
  priority?: number;
  /** Optional description */
  description?: string;
  /**
   * Optional folder path for grouping (e.g., "orders/processing"). Not an
   * engine key: the engine's reader keeps it in `ext.path`, and the folder
   * tree reads it from either place.
   */
  path?: string;
  /** JSONLogic condition (evaluated against full context: data, metadata, temp_data) */
  condition?: JsonLogicValue;
  /**
   * Steps in this workflow: each element is a {@link Task} or a
   * {@link TaskGroup}. Use {@link flattenSteps} for the leaf tasks.
   */
  tasks: Step[];
  /**
   * What a task's failure that reaches the workflow does. Defaults to
   * `"fail"`, which stops the run; `"continue"` runs the next sweep, or the
   * next workflow.
   */
  on_failure?: 'fail' | 'continue';
  /**
   * The 3.x spelling of `on_failure` (`true` is `"continue"`, `false` is
   * `"fail"`). The engine still reads it, rewriting it on load; new
   * definitions write `on_failure`. Render through
   * {@link workflowContinuesOnFailure}, which reads both.
   */
  continue_on_error?: boolean;
  /**
   * Engine-managed bounded loop over this workflow's task list. Absent runs
   * the task list exactly once.
   */
  loop?: LoopConfig;
  /** Channel this workflow is routed on. Defaults to `"default"`. */
  channel?: string;
  /** Lifecycle status. Defaults to `"active"`. */
  status?: WorkflowStatus;
  /**
   * Traffic split. The workflow serves only messages whose routing bucket
   * falls in the range; absent means it serves all of them.
   */
  rollout?: Rollout;
  /** Definition version number. Defaults to `1`. */
  version?: number;
  /** Arbitrary organisational tags. Defaults to `[]`. */
  tags?: string[];
  /** Creation timestamp (RFC 3339). */
  created_at?: string;
  /** Last-update timestamp (RFC 3339). */
  updated_at?: string;
  /**
   * Host and UI data the engine keeps verbatim and never reads. The engine's
   * reader also moves a key it does not know here, `path` included.
   */
  ext?: Record<string, unknown>;
}

/**
 * Whether a failure that reaches the workflow lets the run go on, to the
 * next sweep or the next workflow: `on_failure: "continue"`, or, for a
 * definition the compat reader refused, 3.x's `continue_on_error: true`.
 */
export function workflowContinuesOnFailure(workflow: Workflow): boolean {
  return workflow.on_failure === 'continue' || workflow.continue_on_error === true;
}

/**
 * Workflow lifecycle status.
 *
 * Mirrors `Status` in the engine's `src/def/types.rs`.
 */
export type WorkflowStatus = 'active' | 'paused' | 'archived';

/**
 * Traffic-split range over the `0..100` bucket space.
 *
 * Half-open: `{bucket_start: 0, bucket_end: 10}` serves buckets 0 through 9,
 * so adjacent ranges tile without overlapping.
 */
export interface Rollout {
  bucket_start: number;
  bucket_end: number;
}

/**
 * Map function mapping configuration
 */
export interface MapMapping {
  /**
   * Target path for the mapped value.
   *
   * A plain string is the ordinary case. Since dataflow-rs 3.9 it may also be
   * a JSONLogic expression that resolves to a path per message, which has no
   * single value to display — use `describeMappingPath` rather than rendering
   * it directly.
   */
  path: string | JsonLogicValue;
  /**
   * JSONLogic expression to compute the value. Absent on an `unset` mapping,
   * which has none. A `null` result is skipped, so logic that is always `null`
   * never writes; the engine reports it as `NULL_MAPPING`.
   */
  logic?: JsonLogicValue;
  /** Remove the key at `path` instead of writing it. */
  unset?: boolean;
  /**
   * What a `null` result does: `"skip"` (the default) leaves the path as it
   * was, `"unset"` removes it.
   */
  on_null?: 'skip' | 'unset';
  /**
   * How a result is written: `"set"` (the default) replaces the value,
   * `"append"` pushes it onto the array at `path`, and `"extend"` pushes each
   * element of an array result. Since dataflow-rs 3.15.
   */
  mode?: 'set' | 'append' | 'extend';
}

/**
 * Alias for MapMapping - used in tree and details views
 */
export type MappingItem = MapMapping;

/**
 * Map function input configuration
 */
export interface MapFunctionInput {
  mappings: MapMapping[];
}

/**
 * Validation rule configuration
 */
export interface ValidationRule {
  /** JSONLogic expression that must evaluate to exactly `true` */
  logic: JsonLogicValue;
  /**
   * Error message recorded when the rule fails.
   *
   * Required, like `logic`: the engine's `validation` config gives neither
   * field a default, so a rule missing either one is refused when the
   * workflow is compiled.
   */
  message: string;
}

/**
 * Validation function input configuration
 */
export interface ValidationFunctionInput {
  rules: ValidationRule[];
}

/**
 * The eight built-in functions, which the engine runs inline. Their names
 * are reserved: no registration shadows one, and a task running one cannot
 * carry `for_each`. The list itself is the engine's
 * (`engineFacts().builtin_functions`); this type names its members for
 * display code.
 *
 * `validation` is the one spelling: 3.x's alias `validate` is rewritten on
 * load, and a definition still carrying it renders as a custom function.
 * 3.x's integration configs (`http_call`, `enrich`, `publish_kafka`) left the
 * engine and are a host's functions, rendered as custom too.
 */
export type BuiltinFunctionType =
  | 'map'
  | 'validation'
  | 'parse_json'
  | 'parse_xml'
  | 'publish_json'
  | 'publish_xml'
  | 'filter'
  | 'log';

/**
 * Whether `name` is one of the engine's built-ins, by the loaded engine's own
 * list. Needs the wasm package initialized (see `engineFacts`).
 */
export function isBuiltinFunction(name: string): name is BuiltinFunctionType {
  return engineFacts().builtin_functions.includes(name);
}

/**
 * Get display info for a function type including the Lucide icon component.
 *
 * The eight built-ins have a label and an icon of their own; every other
 * name, a host's functions and 3.x's `validate` included, renders as a
 * custom function under its own name.
 */
export function getFunctionDisplayInfo(name: string): {
  label: string;
  colorClass: string;
  Icon: LucideIcon;
} {
  switch (name) {
    case 'map':
      return { label: 'Map', colorClass: 'df-function-badge-map', Icon: ArrowRightLeft };
    case 'validation':
      return { label: 'Validation', colorClass: 'df-function-badge-validation', Icon: CheckCircle };
    case 'parse_json':
      return { label: 'Parse JSON', colorClass: 'df-function-badge-builtin', Icon: FileJson };
    case 'parse_xml':
      return { label: 'Parse XML', colorClass: 'df-function-badge-builtin', Icon: FileCode };
    case 'publish_json':
      return { label: 'Publish JSON', colorClass: 'df-function-badge-builtin', Icon: Send };
    case 'publish_xml':
      return { label: 'Publish XML', colorClass: 'df-function-badge-builtin', Icon: Send };
    case 'filter':
      return { label: 'Filter', colorClass: 'df-function-badge-builtin', Icon: Filter };
    case 'log':
      return { label: 'Log', colorClass: 'df-function-badge-builtin', Icon: ScrollText };
    default:
      return { label: name, colorClass: 'df-function-badge-custom', Icon: Box };
  }
}

/**
 * Short chip text for a loop badge: `i: 0..10000`, or `0..10000` when the
 * loop leaves its counter unnamed. Appends ` step N` when the increment is
 * not the engine's default.
 *
 * Shows the counter *range*, never a sweep count: with `increment: 2` the
 * range `0..10000` runs 5,000 sweeps, so a `×10000` form would be wrong.
 */
export function loopBadgeLabel(loop: LoopConfig): string {
  if (loop.over !== undefined) {
    return `${loop.as ?? 'each'} in over, max ${loop.max}`;
  }
  const defaults = engineFacts().loop_defaults;
  const init = loop.init ?? defaults.init;
  const increment = loop.increment ?? defaults.increment;
  const step = increment === defaults.increment ? '' : ` step ${increment}`;
  const range = `${init}..${loop.max}`;
  return loop.counter ? `${loop.counter}: ${range}${step}` : `${range}${step}`;
}

/**
 * Text for the loop guard diamond: `i < 10000`, or `sweep < 10000` when the
 * counter is unnamed. The engine still tracks the count either way.
 */
export function loopGuardLabel(loop: LoopConfig): string {
  const subject = loop.counter ?? 'sweep';
  // An `over` loop has two bounds: the array's length and `max`.
  return loop.over !== undefined
    ? `${subject} < len, < ${loop.max}`
    : `${subject} < ${loop.max}`;
}

/**
 * Text for the loop tail node — the counter advance the engine performs after
 * each sweep: `i += 1`, or `next sweep` when the counter is unnamed.
 */
export function loopStepLabel(loop: LoopConfig): string {
  const increment = loop.increment ?? engineFacts().loop_defaults.increment;
  return loop.counter ? `${loop.counter} += ${increment}` : 'next sweep';
}

/**
 * Short chip text for a fan-out: `each p`, or `each p ×8` when calls run
 * concurrently.
 */
export function forEachBadgeLabel(forEach: ForEach): string {
  const concurrency =
    forEach.max_concurrency ?? engineFacts().for_each_defaults.max_concurrency;
  return concurrency > 1 ? `each ${forEach.as} ×${concurrency}` : `each ${forEach.as}`;
}

/** Full-sentence description of a fan-out, for a `title` tooltip. */
export function forEachDescription(forEach: ForEach): string {
  const concurrency =
    forEach.max_concurrency ?? engineFacts().for_each_defaults.max_concurrency;
  const pace =
    concurrency > 1 ? `up to ${concurrency} calls at a time` : 'one call at a time';
  const results = forEach.into
    ? ` Results land in ${forEach.into}, in element order.`
    : '';
  return (
    `Runs once per element of \`over\`, ${pace}, with the element at ` +
    `temp_data.${forEach.as} and its index at temp_data.${forEach.as}_index.${results}`
  );
}

/** Full-sentence description of the loop contract, for a `title` tooltip. */
export function loopDescription(loop: LoopConfig): string {
  const defaults = engineFacts().loop_defaults;
  const init = loop.init ?? defaults.init;
  const increment = loop.increment ?? defaults.increment;
  const subject = loop.counter ? `temp_data.${loop.counter}` : 'the sweep count';
  const setupCount = loop.setup ? countLeafSteps(loop.setup) : 0;
  const setup = setupCount > 0 ? `Runs ${setupCount} setup step(s) once first. ` : '';
  const scratch = loop.scratch
    ? ` temp_data.${loop.scratch} is reset to {} every sweep.`
    : '';
  if (loop.over !== undefined) {
    const slot = loop.as ? `temp_data.${loop.as}` : 'no slot';
    return (
      `${setup}Iterates the array \`over\` yields, one sweep per element (${slot}), ` +
      `while ${subject} < min(length, ${loop.max}), starting at ${init}, step ${increment}. ` +
      `Exits at the array's end, at the bound, or when the condition goes false.${scratch}`
    );
  }
  return (
    `${setup}Loops while ${subject} < ${loop.max}, starting at ${init}, step ${increment}. ` +
    `Exits when the bound is reached or the condition goes false.${scratch}`
  );
}
