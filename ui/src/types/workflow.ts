import {
  ArrowRightLeft,
  Box,
  CheckCircle,
  FileCode,
  FileJson,
  Filter,
  Globe,
  ScrollText,
  Send,
  Sparkles,
  type LucideIcon,
} from 'lucide-react';

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
   * Required: the engine's `FunctionConfig` deserializer reads `{name, input}`
   * with no default for `input`, so a task omitting it fails to load.
   * Functions that take no configuration still need `input: {}`.
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
  /** Whether to continue workflow if this task fails */
  continue_on_error?: boolean;
  /**
   * End the workflow once this task has run. Defaults to `false`.
   *
   * A statement about *position*, not outcome: a false `condition` or a
   * `Skip` outcome does not halt, but a task that failed under
   * `continue_on_error` still does. Halting stops this workflow only.
   */
  terminal?: boolean;
}

/**
 * A contiguous run of tasks sharing one condition.
 *
 * Mirrors `TaskGroup` in `src/engine/task.rs`. In a workflow's `tasks` array a
 * group is an element carrying a `tasks` key; a plain task carries `function`.
 * The group condition is evaluated **once, on entry** — a false result skips
 * the whole span without evaluating the members' own conditions.
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
  /** The nested steps. Must not be empty. */
  tasks: Step[];
}

/** One element of a workflow's `tasks` array: a task or a group of them. */
export type Step = Task | TaskGroup;

/**
 * Whether a step is a group rather than a plain task.
 *
 * The test is **presence of a `tasks` key, nothing else** — the same test the
 * engine's parser makes (`is_group` in `src/engine/steps.rs`). A `tasks` key
 * holding a non-array is still a group, and a malformed one; reading it as a
 * task instead would classify it differently from the engine that has to run
 * it.
 */
export function isTaskGroup(step: Step): step is TaskGroup {
  return (step as TaskGroup).tasks !== undefined;
}

/**
 * The members of a group, or none when `tasks` is malformed.
 *
 * `isTaskGroup` tests only for the presence of the key, so a group whose
 * `tasks` is not an array is still a group — with no members. The engine
 * rejects such a workflow at parse time; a renderer must not iterate the value
 * (a string would iterate character by character). Mirrors the walker in
 * `src/engine/steps.rs`, which descends only into a real array.
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
 * Engine-managed `for` loop over a workflow's task list.
 *
 * Mirrors `LoopConfig` in `src/engine/workflow.rs`. A workflow carrying a
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
  /** First counter value. Defaults to 0. */
  init?: number;
  /** Added to the counter after each sweep. Defaults to 1; must be >= 1. */
  increment?: number;
  /**
   * Required upper bound — sweeps run while `counter < max`. Half-open, so
   * `init: 0, max: n` yields `0..n-1`, exactly array indices.
   */
  max: number;
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
  /** Optional folder path for grouping (e.g., "orders/processing") */
  path?: string;
  /** JSONLogic condition (evaluated against full context: data, metadata, temp_data) */
  condition?: JsonLogicValue;
  /**
   * Steps in this workflow: each element is a {@link Task} or a
   * {@link TaskGroup}. Use {@link flattenSteps} for the leaf tasks.
   */
  tasks: Step[];
  /** Whether to continue processing other workflows if this one fails */
  continue_on_error?: boolean;
  /**
   * Engine-managed bounded loop over this workflow's task list. Absent runs
   * the task list exactly once. The JSON key is `loop` (Rust
   * `#[serde(rename = "loop")]`).
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
}

/**
 * Workflow lifecycle status.
 *
 * Mirrors `WorkflowStatus` in `src/engine/workflow.rs`.
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
  /** Target path for the mapped value */
  path: string;
  /** JSONLogic expression to compute the value */
  logic: JsonLogicValue;
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
   * Required, like `logic`: the engine's `ValidationRule` gives neither field a
   * serde default, so a rule missing either one is rejected when the engine is
   * built.
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
 * Built-in function names the engine recognises.
 *
 * Mirrors `BUILTIN_FUNCTION_NAMES` in `src/engine/functions/config.rs`.
 * `validate` is an alias of `validation`.
 *
 * The last three are **config-only**: the crate supplies their schema but ships
 * no handler, so a workflow using one loads cleanly and then fails at dispatch
 * unless the host has registered an implementation.
 */
export type BuiltinFunctionType =
  | 'map'
  | 'validation'
  | 'validate'
  | 'parse_json'
  | 'parse_xml'
  | 'publish_json'
  | 'publish_xml'
  | 'filter'
  | 'log'
  | 'http_call'
  | 'enrich'
  | 'publish_kafka';

const BUILTIN_FUNCTION_NAMES: readonly BuiltinFunctionType[] = [
  'map',
  'validation',
  'validate',
  'parse_json',
  'parse_xml',
  'publish_json',
  'publish_xml',
  'filter',
  'log',
  'http_call',
  'enrich',
  'publish_kafka',
];

/**
 * Built-ins that need a handler registered by the host before they can run.
 */
export const INTEGRATION_FUNCTION_NAMES: readonly BuiltinFunctionType[] = [
  'http_call',
  'enrich',
  'publish_kafka',
];

/**
 * Check if a function is a built-in type
 */
export function isBuiltinFunction(name: string): name is BuiltinFunctionType {
  return (BUILTIN_FUNCTION_NAMES as readonly string[]).includes(name);
}

/**
 * Get display info for a function type including the Lucide icon component
 */
export function getFunctionDisplayInfo(name: string): {
  label: string;
  colorClass: string;
  Icon: LucideIcon;
} {
  switch (name) {
    case 'map':
      return { label: 'Map', colorClass: 'df-function-badge-map', Icon: ArrowRightLeft };
    case 'validate':
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
    case 'http_call':
      return { label: 'HTTP Call', colorClass: 'df-function-badge-builtin', Icon: Globe };
    case 'enrich':
      return { label: 'Enrich', colorClass: 'df-function-badge-builtin', Icon: Sparkles };
    case 'publish_kafka':
      return { label: 'Publish Kafka', colorClass: 'df-function-badge-builtin', Icon: Send };
    default:
      return { label: name, colorClass: 'df-function-badge-custom', Icon: Box };
  }
}

/** Engine default for `init` when the field is absent. */
const LOOP_INIT_DEFAULT = 0;
/** Engine default for `increment` when the field is absent. */
const LOOP_INCREMENT_DEFAULT = 1;

/**
 * Short chip text for a loop badge: `i: 0..10000`, or `0..10000` when the
 * loop leaves its counter unnamed. Appends ` step N` when the increment is
 * not 1.
 *
 * Shows the counter *range*, never a sweep count: with `increment: 2` the
 * range `0..10000` runs 5,000 sweeps, so a `×10000` form would be wrong.
 */
export function loopBadgeLabel(loop: LoopConfig): string {
  const init = loop.init ?? LOOP_INIT_DEFAULT;
  const increment = loop.increment ?? LOOP_INCREMENT_DEFAULT;
  const step = increment === LOOP_INCREMENT_DEFAULT ? '' : ` step ${increment}`;
  const range = `${init}..${loop.max}`;
  return loop.counter ? `${loop.counter}: ${range}${step}` : `${range}${step}`;
}

/**
 * Text for the loop guard diamond: `i < 10000`, or `sweep < 10000` when the
 * counter is unnamed. The engine still tracks the count either way.
 */
export function loopGuardLabel(loop: LoopConfig): string {
  return `${loop.counter ?? 'sweep'} < ${loop.max}`;
}

/**
 * Text for the loop tail node — the counter advance the engine performs after
 * each sweep: `i += 1`, or `next sweep` when the counter is unnamed.
 */
export function loopStepLabel(loop: LoopConfig): string {
  const increment = loop.increment ?? LOOP_INCREMENT_DEFAULT;
  return loop.counter ? `${loop.counter} += ${increment}` : 'next sweep';
}

/** Full-sentence description of the loop contract, for a `title` tooltip. */
export function loopDescription(loop: LoopConfig): string {
  const init = loop.init ?? LOOP_INIT_DEFAULT;
  const increment = loop.increment ?? LOOP_INCREMENT_DEFAULT;
  const subject = loop.counter ? `temp_data.${loop.counter}` : 'the sweep count';
  return (
    `Loops while ${subject} < ${loop.max}, starting at ${init}, step ${increment}. ` +
    `Exits when the bound is reached or the condition goes false.`
  );
}
