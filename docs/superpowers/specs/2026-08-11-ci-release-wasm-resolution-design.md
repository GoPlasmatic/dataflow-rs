# CI and release WASM resolution

**Date:** 2026-08-11
**Status:** Approved, not yet implemented

## Problem

`ui/src/engines/versionCheck.ts` imports `engine_version` from
`@goplasmatic/dataflow-wasm`. That symbol is new in 3.3.0 and is exported by no
published version — verified by unpacking the 3.1.0 and 3.2.0 tarballs, neither
of which names it in `dataflow_wasm.d.ts`.

CI's `ui` job resolves the engine from npm, then runs `build:lib`
(`tsc && vite build`). The typecheck fails:

```
src/engines/versionCheck.ts(1,10): error TS2614: Module
'"@goplasmatic/dataflow-wasm"' has no exported member 'engine_version'.
exit code 2
```

This is circular: `dataflow-ui` 3.3.0 cannot typecheck without
`dataflow-wasm` 3.3.0's declarations, but that package is published only during
the release, which is gated on CI passing. The branch has been in this state
since commit `f97beeb`.

`release.yml`'s `publish-ui` job has the same defect independently. It runs
`npm ci` (lockfile pins the previous version), then copies only
`dataflow_wasm.js` and `dataflow_wasm_bg.wasm` over `node_modules` — not
`dataflow_wasm.d.ts` — so its `tsc` also reads stale declarations.

The failure is safe rather than destructive: `publish-crate` and `publish-wasm`
both declare `needs: [validate, ci]`, so nothing reaches a registry.

### The dependency is types-only

`@goplasmatic/dataflow-wasm` sits in `dependencies`, and `vite.lib.config.ts`
externalizes every dependency and peer dependency. The published
`dataflow-ui` bundle therefore never contains the engine; it imports it at
runtime from the consumer's `node_modules`. Only the `.d.ts` affects
`build:lib`. The `.js`/`.wasm` copy in `publish-ui` never entered the bundle.

## Approach

CI and release get different resolution strategies, because they answer
different questions.

| Context | Question | Engine source |
|---|---|---|
| CI | Does the UI work with the engine in *this commit*? | Built locally from the checkout |
| Release | Does the UI work with the engine users will install? | Downloaded from npm |

CI stops asking the registry anything, which is what breaks the cycle.

## Component 1 — CI (`.github/workflows/ci.yml`, `ui` job)

Add a Rust/wasm toolchain and one script call. The job keeps running in
parallel with `wasm`:

```yaml
- setup Rust (targets: wasm32-unknown-unknown)
- install wasm-pack     # curl installer, matching the `wasm` job
- npm ci                # published engine; about to be replaced
- npm run wasm:local    # build, verify, overlay
- npm run lint
- npm run build:lib
```

wasm-pack is installed the same way the `wasm` job does it — the prebuilt-binary
curl installer, not the pinned `cargo install`. The repo's stated rule is that
the pin exists for the job that *produces the artifact users install*, while
regression-checking jobs take the fast prebuilt binary. The `ui` job is the
latter.

`scripts/use-local-wasm.mjs` already performs the whole overlay: it runs
`wasm-pack build --target web --out-dir pkg --release`, gates on
`verify-wasm.mjs`, copies `dataflow_wasm.js`, `dataflow_wasm_bg.wasm` **and**
`dataflow_wasm.d.ts` into `node_modules`, and clears `node_modules/.vite`. No
new script is needed.

The existing comment on the job — that it resolves from npm and that
`release.yml` is what validates against the release's wasm — is now inverted
and must be rewritten.

**Accepted cost:** the wasm is built twice per CI run, once in `wasm` and once
in `ui`. This was chosen deliberately over passing an artifact between jobs, to
keep the two jobs independent and parallel.

## Component 2 — Release (`.github/workflows/release.yml`, `publish-ui` job)

```yaml
- npm ci
- wait for @goplasmatic/dataflow-wasm@$VERSION on the registry   # bounded poll
- npm install @goplasmatic/dataflow-wasm@$VERSION
- npm run build:lib
- npm pkg set dependencies.@goplasmatic/dataflow-wasm=^$VERSION  # existing step
```

`npm ci` alone cannot produce the right engine: it resolves the committed
lockfile, which pins the previous release. The explicit install is what makes
the job build against the artifact `publish-wasm` just pushed.

The poll absorbs npm registry propagation delay between `publish-wasm` and
`publish-ui`. It is bounded at 30 attempts, 10 seconds apart — five minutes —
and then exits non-zero with an explicit error naming the version it waited for.

**Deletions.** The `download-artifact` step and the two `cp` commands that
overlay `dataflow_wasm.js` / `dataflow_wasm_bg.wasm` are removed. Keeping them
would overwrite the published package with the local build, which is exactly
what "release validates the published dependency" rules out; and as established
above they never affected the bundle. `needs: [validate, publish-wasm]` stays,
for ordering.

The retry belongs here and only here. CI does not install the published engine,
so it has no registry race to absorb.

## Component 3 — Runtime guard (`ui/src/engines/versionCheck.ts`)

Replace the named import with a namespace import, and treat a missing
`engine_version` as "engine too old":

```ts
import * as wasm from '@goplasmatic/dataflow-wasm';

const engineVersionFn = (wasm as { engine_version?: () => string }).engine_version;
```

An engine that predates the first version exporting `engine_version` is, by
definition, older than any UI that requires it, so it routes into the existing
error path and message.

This fixes a real defect in the feature being stabilized. With a named import,
an engine lacking the symbol fails at module-link time with an opaque bundler
error instead of the intended message — precisely in the case the check was
written to handle. It also decouples the typecheck from the published `.d.ts`
shape.

## Error handling

- **CI:** a wasm build or verification failure fails the `ui` job, the same way
  it fails the `wasm` job.
- **Release:** on poll timeout the job fails after the crate and wasm have
  published. Every publish job is guarded by an "is this version already on the
  registry" check, so re-running the workflow is idempotent and safe.
- **Runtime:** an engine too old to export `engine_version` produces the
  friendly version-mismatch error rather than a link failure.

## Testing

CI's path is verifiable locally, and has already been exercised by hand:
`npm ci` (installs the published, older engine) followed by
`npm run wasm:local` then `npm run build:lib` passes, whereas `npm ci` followed
directly by `build:lib` fails with TS2614. That pair is the regression test for
this change.

The release path cannot be fully exercised without pushing a tag. The mitigation
is that each step is independently re-runnable and the publish jobs are
idempotent.

## Out of scope

- Pinning `ui/package.json`'s wasm dependency to an exact version. The caret
  range is required so `npm ci` resolves before the matching engine exists.
- `Workflow`'s missing `deny_unknown_fields`, the root cause that makes an old
  engine silently ignore unknown fields. That is a semver-visible change to the
  crate and needs its own decision.
