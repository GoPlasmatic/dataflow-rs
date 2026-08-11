import * as wasm from '@goplasmatic/dataflow-wasm';

// Read off the namespace rather than `import { engine_version }`. An engine
// released before the export existed would fail a *named* import at
// module-link time, with a bundler error that says nothing about versions —
// in exactly the case this module exists to explain. Off the namespace, a
// missing symbol is just a value this code can interpret.
const engineVersionFn = (wasm as { engine_version?: () => string }).engine_version;

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
    const engineVersion = engineVersionFn?.();
    const uiVersion = __DATAFLOW_UI_VERSION__;

    // No `engine_version` at all means the engine predates the release that
    // added it, so it is necessarily older than any build running this check.
    const tooOld =
      engineVersion === undefined ||
      isOlder(parseSemver(engineVersion), parseSemver(uiVersion));

    result = tooOld
      ? {
          ok: false,
          error: new Error(
            `@goplasmatic/dataflow-wasm ${engineVersion ?? '(too old to report its version)'} ` +
              `is older than @goplasmatic/dataflow-ui ${uiVersion} expects. Workflow ` +
              `fields added after it are silently ignored by that engine, so results ` +
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
