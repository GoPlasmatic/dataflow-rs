import type { JsonLogicValue, MappingItem } from '../types';

/**
 * Convert mappings array to object notation for DataLogic visualization
 */
export function convertMappingsToObject(mappings: MappingItem[]): Record<string, JsonLogicValue> {
  const result: Record<string, JsonLogicValue> = {};
  for (const mapping of mappings) {
    result[mapping.path] = mapping.logic;
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
