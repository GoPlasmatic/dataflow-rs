import { engine_version } from '@goplasmatic/dataflow-wasm';

// Injected by both vite configs from package.json. Declared locally rather
// than in vite-env.d.ts: that file has previously been pulled into the
// declaration build and produced an empty dist/lib.d.ts (v2.1.3).
declare const __DATAFLOW_UI_VERSION__: string;

type Semver = [number, number, number];

/** Lenient parse — anything unparseable becomes 0 rather than throwing. */
function parseSemver(version: string): Semver {
  const parts = version
    .split('-')[0]
    .split('.')
    .map((p) => Number.parseInt(p, 10));
  return [parts[0] || 0, parts[1] || 0, parts[2] || 0];
}

/** True when `a` is strictly older than `b`. */
function isOlder(a: Semver, b: Semver): boolean {
  for (let i = 0; i < 3; i++) {
    if (a[i] !== b[i]) return a[i] < b[i];
  }
  return false;
}

// Memoised: the check is pure and the engine cannot change under us. A failure
// caches the *error* rather than a "checked" flag, so a second call re-throws
// instead of silently passing.
let result: { ok: true } | { ok: false; error: Error } | undefined;

/**
 * Throws when the loaded WASM engine is older than this build of
 * `@goplasmatic/dataflow-ui` expects.
 *
 * An older engine is not a cosmetic mismatch. `Workflow` does not set
 * `deny_unknown_fields`, so it silently *ignores* any workflow field added
 * after its release instead of rejecting it — a workflow using a newer feature
 * appears to run correctly while doing something else entirely. Failing here
 * converts that into an error at the point of use.
 *
 * A **newer** engine is fine and passes silently: this package declares a
 * caret range on the wasm dependency, so npm may legitimately resolve one, and
 * the UI simply does not exercise whatever it added.
 */
export function assertEngineVersion(): void {
  if (result === undefined) {
    const engineVersion = engine_version();
    const uiVersion = __DATAFLOW_UI_VERSION__;

    result = isOlder(parseSemver(engineVersion), parseSemver(uiVersion))
      ? {
          ok: false,
          error: new Error(
            `@goplasmatic/dataflow-wasm ${engineVersion} is older than ` +
              `@goplasmatic/dataflow-ui ${uiVersion} expects. Workflow fields added ` +
              `after ${engineVersion} are silently ignored by that engine, so results ` +
              `would not match the workflows shown. Upgrade the wasm package to ` +
              `>=${uiVersion}, or run \`npm run wasm:local\` when working in this repo.`,
          ),
        }
      : { ok: true };
  }

  if (!result.ok) {
    throw result.error;
  }
}

/** Test seam: forget the memoised result. */
export function resetEngineVersionCheck(): void {
  result = undefined;
}
