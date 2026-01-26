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
  /** JSONLogic condition (evaluated against metadata only) */
  condition?: JsonLogicValue;
  /** Function to execute */
  function: FunctionConfig;
  /** Whether to continue workflow if this task fails */
  continue_on_error?: boolean;
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
  /** JSONLogic condition (evaluated against metadata only) */
  condition?: JsonLogicValue;
  /** Tasks in this workflow */
  tasks: Task[];
  /** Whether to continue processing other workflows if this one fails */
  continue_on_error?: boolean;
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
export type BuiltinFunctionType = 'map' | 'validation';

/**
 * Check if a function is a built-in type
 */
export function isBuiltinFunction(name: string): name is BuiltinFunctionType {
  return name === 'map' || name === 'validation';
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
    case 'validation':
      return { label: 'Validation', colorClass: 'df-function-badge-validation', Icon: CheckCircle };
    default:
      return { label: name, colorClass: 'df-function-badge-custom', Icon: Box };
  }
}
