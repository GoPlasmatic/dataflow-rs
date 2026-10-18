import { describe, expect, it } from 'vitest';
import { migrateDefinitions } from './migrate';
import type { Task } from '../types/workflow';

describe('migrateDefinitions', () => {
  it('renders a 3.x policy as the 4.0 one the engine runs', () => {
    const [workflow] = migrateDefinitions([
      {
        id: 'w',
        tasks: [
          {
            id: 't',
            continue_on_error: true,
            function: { name: 'map', input: { mappings: [] } },
          },
        ],
      },
    ]);
    const task = workflow.tasks[0] as Task;
    expect(task.on_failure).toBe('continue');
    expect(task.continue_on_error).toBeUndefined();
  });

  it('keeps the authored definition when the reader refuses it', () => {
    // A v3 flag beside its v4 spelling: the reader refuses the mix rather than
    // guessing, and the authored object is kept so the tree still shows it.
    const [workflow] = migrateDefinitions([
      { id: 'w', continue_on_error: true, on_failure: 'fail', tasks: [] },
    ]);
    expect(workflow.continue_on_error).toBe(true);
    expect(workflow.on_failure).toBe('fail');
  });

  it('maps every definition in the array, in order', () => {
    const migrated = migrateDefinitions([
      { id: 'a', tasks: [] },
      { id: 'b', tasks: [] },
    ]);
    expect(migrated.map((w) => w.id)).toEqual(['a', 'b']);
  });
});
