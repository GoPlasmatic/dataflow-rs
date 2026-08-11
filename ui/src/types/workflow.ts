import { ArrowRightLeft, CheckCircle, Box, type LucideIcon } from 'lucide-react';

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
  /** Function-specific input configuration */
  input?: Record<string, unknown>;
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
  /** Tasks in this workflow */
  tasks: Task[];
  /** Whether to continue processing other workflows if this one fails */
  continue_on_error?: boolean;
  /**
   * Engine-managed bounded loop over this workflow's task list. Absent runs
   * the task list exactly once. The JSON key is `loop` (Rust
   * `#[serde(rename = "loop")]`).
   */
  loop?: LoopConfig;
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
  /** JSONLogic expression that should return true for valid data */
  logic: JsonLogicValue;
  /** Error message if validation fails */
  message: string;
  /** Optional path for the validation target */
  path?: string;
}

/**
 * Validation function input configuration
 */
export interface ValidationFunctionInput {
  rules: ValidationRule[];
}

/**
 * Built-in function types
 */
export type BuiltinFunctionType = 'map' | 'validation' | 'validate';

/**
 * Check if a function is a built-in type
 */
export function isBuiltinFunction(name: string): name is BuiltinFunctionType {
  return name === 'map' || name === 'validation' || name === 'validate';
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
