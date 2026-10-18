import { parse, type JsonValue } from '@goplasmatic/dataflow-wasm';
import type { Workflow } from '../types/workflow';

/**
 * Raw definitions read as the engine reads them: the compat reader rewrites
 * 3.x's spellings to their 4.0 ones, so the debugger renders and reasons about
 * exactly what the engine runs (see `parse` in the wasm package) rather than
 * re-deciding the v3 policy table itself.
 *
 * A definition the reader refuses is kept as authored, beside the engine's
 * issue when the adapter reports it, so a renderer still has something to
 * show. `parse` throws only for an element that is not a definition object at
 * all; that element is kept too.
 */
export function migrateDefinitions(raw: unknown[]): Workflow[] {
  return raw.map((definition) => {
    try {
      const parsed = parse(definition as JsonValue);
      return (parsed.definition ?? definition) as Workflow;
    } catch {
      return definition as Workflow;
    }
  });
}
