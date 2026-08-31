import type { JsonLogicValue, MappingItem } from '../types';

/**
 * How to label a mapping's destination.
 *
 * A destination is a plain string in the ordinary case, but since dataflow-rs
 * 3.9 it may be a JSONLogic expression resolved per message — which has no
 * single value to show. Rendering that directly would yield "[object Object]",
 * so name it by the expression instead, which is what an author would search
 * their workflow for.
 */
export function describeMappingPath(path: MappingItem['path']): string {
  return typeof path === 'string' ? path : JSON.stringify(path);
}

/**
 * Convert mappings array to object notation for DataLogic visualization
 */
export function convertMappingsToObject(mappings: MappingItem[]): Record<string, JsonLogicValue> {
  const result: Record<string, JsonLogicValue> = {};
  for (const mapping of mappings) {
    result[describeMappingPath(mapping.path)] = mapping.logic;
  }
  return result;
}

/**
 * Safely stringify an object, handling circular references
 */
export function safeStringify(obj: unknown, indent = 2): string {
  const seen = new WeakSet();
  return JSON.stringify(
    obj,
    (_key, value) => {
      if (typeof value === 'object' && value !== null) {
        if (seen.has(value)) {
          return '[Circular]';
        }
        seen.add(value);
      }
      return value;
    },
    indent
  );
}
