import * as wasm from '@goplasmatic/dataflow-wasm';
import type { EngineFacts } from '@goplasmatic/dataflow-wasm';
import { rethrowUninitialized } from './uninitialized';

// Read off the namespace, as versionCheck.ts does. An engine released before
// `engine_facts` existed would fail a *named* import at module-link time, with
// a bundler error that says nothing about versions. Off the namespace, a
// missing symbol is just a value this code can interpret.
const engineFactsFn = (wasm as { engine_facts?: () => EngineFacts }).engine_facts;

// Memoised on success only. The facts are compiled into the engine and cannot
// change under us, but a call before `init()` fails, and a later call, after
// it, must succeed rather than replay the failure.
let facts: EngineFacts | undefined;

/**
 * The loaded engine's facts: its version, the built-in function names, the
 * group depth limit, and what a `loop` and a `for_each` read for an absent
 * field. Read from the engine (`engine_facts()`) rather than kept as a copy
 * here, so the UI cannot drift from the engine it runs.
 *
 * The wasm package must be initialized first (`await init()`): the module's
 * exports call into an instance that does not exist until then, and a call
 * made earlier throws an `Error` saying so.
 */
export function engineFacts(): EngineFacts {
  if (facts === undefined) {
    if (engineFactsFn === undefined) {
      throw new Error(
        '@goplasmatic/dataflow-wasm does not export engine_facts: the engine is older ' +
          'than this build of @goplasmatic/dataflow-ui expects. Upgrade the wasm package, ' +
          'or run `npm run wasm:local` when working in this repo.',
      );
    }
    try {
      facts = engineFactsFn();
    } catch (err) {
      rethrowUninitialized(err, "the UI reads the engine's facts");
    }
  }
  return facts;
}
