// The definition types are a hand mirror of the engine's reader, so this
// holds them to it: every key the engine knows at a level must appear in the
// matching UI type, and every key the UI type carries beyond the engine's must
// be one of the v3 spellings it renders on purpose. A new engine key — the
// bug this guards — fails the first test; a stray UI key fails the second.
//
// The engine's keys come from `engine_facts()`, which reads the very tables
// the walk uses, not a copy.

import { engine_facts } from '@goplasmatic/dataflow-wasm';
import { describe, expect, it } from 'vitest';
import type {
  ForEach,
  FunctionConfig,
  LoopConfig,
  Rollout,
  Task,
  TaskGroup,
  Workflow,
} from './workflow';

// Typed as `Record<keyof T, true>`, so a key added to an interface fails to
// compile here until it is listed: the runtime set the test reads cannot go
// stale silently.
const WORKFLOW_KEYS: Record<keyof Workflow, true> = {
  id: true,
  name: true,
  priority: true,
  description: true,
  path: true,
  condition: true,
  tasks: true,
  on_failure: true,
  continue_on_error: true,
  loop: true,
  channel: true,
  status: true,
  rollout: true,
  version: true,
  tags: true,
  created_at: true,
  updated_at: true,
  ext: true,
};

const TASK_KEYS: Record<keyof Task, true> = {
  id: true,
  name: true,
  description: true,
  condition: true,
  function: true,
  on_failure: true,
  on_rejection: true,
  continue_on_error: true,
  terminal: true,
  halt_on: true,
  for_each: true,
  ext: true,
};

const GROUP_KEYS: Record<keyof TaskGroup, true> = {
  id: true,
  name: true,
  description: true,
  condition: true,
  terminal: true,
  continue_on_error: true,
  tasks: true,
  ext: true,
};

const LOOP_KEYS: Record<keyof LoopConfig, true> = {
  counter: true,
  init: true,
  increment: true,
  max: true,
  setup: true,
  over: true,
  as: true,
  scratch: true,
};

const FOR_EACH_KEYS: Record<keyof ForEach, true> = {
  over: true,
  as: true,
  max_concurrency: true,
  collect: true,
  into: true,
};

const ROLLOUT_KEYS: Record<keyof Rollout, true> = {
  bucket_start: true,
  bucket_end: true,
};

const FUNCTION_KEYS: Record<keyof FunctionConfig, true> = {
  name: true,
  input: true,
};

/** The UI type keys, by the engine's level name. */
const UI_KEYS: Record<string, Record<string, true>> = {
  workflow: WORKFLOW_KEYS,
  task: TASK_KEYS,
  group: GROUP_KEYS,
  loop: LOOP_KEYS,
  for_each: FOR_EACH_KEYS,
  rollout: ROLLOUT_KEYS,
  function: FUNCTION_KEYS,
};

/**
 * The keys a UI type carries that the engine does not: the v3 spellings it
 * still renders when the compat reader refuses a definition (see
 * `migrateDefinitions`, which rewrites the ones it accepts), and the UI's own
 * `path`. Any other extra is a stray key and fails.
 */
const UI_ONLY: Record<string, readonly string[]> = {
  workflow: ['path', 'continue_on_error'],
  task: ['continue_on_error', 'halt_on'],
  group: ['continue_on_error'],
};

describe('the definition types are held to the engine', () => {
  it('carry every key the engine knows at their level', () => {
    const facts = engine_facts();
    for (const [level, keys] of Object.entries(facts.definition_keys)) {
      const ui = UI_KEYS[level];
      expect(ui, `no UI type for the engine's ${level}`).toBeDefined();
      for (const key of keys) {
        expect(Object.keys(ui), `${level}.${key}`).toContain(key);
      }
    }
  });

  it('carry nothing else, but the v3 spellings listed', () => {
    const facts = engine_facts();
    for (const [level, ui] of Object.entries(UI_KEYS)) {
      const known = new Set(facts.definition_keys[level as keyof typeof facts.definition_keys]);
      const allowed = new Set(UI_ONLY[level] ?? []);
      for (const key of Object.keys(ui)) {
        expect(known.has(key) || allowed.has(key), `the UI adds ${level}.${key}`).toBe(true);
      }
    }
  });
});
