# CI and Release WASM Resolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Break the CI/release circular dependency by giving each context its own engine source — CI builds the wasm from the commit under test, release installs the wasm it just published.

**Architecture:** Three independent changes. `ui/src/engines/versionCheck.ts` stops hard-coupling its typecheck to the published `.d.ts` via a namespace import. `ci.yml`'s `ui` job gains a Rust toolchain and calls the existing `wasm:local` overlay script. `release.yml`'s `publish-ui` job drops the local-artifact overlay and instead waits for, then installs, the version `publish-wasm` published moments earlier.

**Tech Stack:** GitHub Actions, TypeScript 6, Vite 8, wasm-pack 0.14, npm 20.

## Global Constraints

- Conventional-commit subjects; prose paragraph bodies wrapped at ~72 chars; **no** `Co-authored-by` or `Generated with` footers (matches existing history).
- `ui/` has **no test runner** — no vitest, no jest, no `test` script, no `*.test.*` files. Verification for UI changes is `npx tsc --noEmit`, `npm run lint` (`--max-warnings 0`), and `npm run build:lib`. Do not add a test framework; it is out of scope.
- Node version in CI is `'20'`; do not change it.
- The `wasm` job's wasm-pack install is the unpinned curl installer; `release.yml`'s `publish-wasm` pins `cargo install wasm-pack --version 0.14.0 --locked`. Regression-checking jobs use the curl installer; artifact-producing jobs use the pin. Preserve that split.
- Never run `npm audit fix --force` in this repo — it proposes downgrading `monaco-editor` to dodge a `dompurify` range the existing `overrides` entry already escapes.
- Running `npm ci` or `npm install` in `ui/` wipes the local engine overlay. After any such command locally, re-run `npm run wasm:local` before trusting a typecheck.

---

### Task 1: Decouple the version check from the published type declarations

`versionCheck.ts` uses a named import for `engine_version`. No published
`dataflow-wasm` exports it (verified against the 3.1.0 and 3.2.0 tarballs), so
`tsc` fails TS2614 against any registry-resolved engine. A named import also
fails at module-link time with an opaque bundler error in precisely the case the
check exists to report: an engine too old to know about it.

**Files:**
- Modify: `ui/src/engines/versionCheck.ts:1` (the import) and `:46-68` (`assertEngineVersion`)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: `assertEngineVersion(): void` — unchanged signature, unchanged export name, still the only export of the module. `ui/src/engines/index.ts` and `ui/src/lib.ts` re-export it as-is and must not change.

- [ ] **Step 1: Reproduce the failure against the published engine**

This is the regression test. It must fail before the change and pass after.

```bash
cd ui
npm ci                      # installs the published engine, which lacks engine_version
npx tsc --noEmit -p tsconfig.json
```

Expected: FAIL with

```
src/engines/versionCheck.ts(1,10): error TS2614: Module
'"@goplasmatic/dataflow-wasm"' has no exported member 'engine_version'.
```

- [ ] **Step 2: Replace the named import with a namespace import**

Replace line 1 of `ui/src/engines/versionCheck.ts`:

```ts
import { engine_version } from '@goplasmatic/dataflow-wasm';
```

with:

```ts
import * as wasm from '@goplasmatic/dataflow-wasm';

// Read off the namespace rather than `import { engine_version }`. An engine
// released before the export existed would fail a *named* import at
// module-link time, with a bundler error that says nothing about versions —
// in exactly the case this module exists to explain. Off the namespace, a
// missing symbol is just a value this code can interpret.
const engineVersionFn = (wasm as { engine_version?: () => string }).engine_version;
```

Placement: this replaces line 1 only. The `declare const __DATAFLOW_UI_VERSION__:
string;` block and its comment (currently lines 3-6) follow it unchanged — do not
move, merge, or delete them. That comment records why the declaration is here and
not in `vite-env.d.ts`, which once produced an empty `dist/lib.d.ts`.

- [ ] **Step 3: Treat a missing export as "too old"**

Replace the body of `assertEngineVersion` (the `if (result === undefined)` block)
with:

```ts
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
```

Note the message now says "fields added after **it**" rather than interpolating
the version a second time, so it reads correctly in both branches.

Leave `parseSemver`, `isOlder`, the `result` memo, the doc comment on
`assertEngineVersion`, and the trailing `if (!result.ok) throw result.error;`
exactly as they are.

- [ ] **Step 4: Verify the typecheck now passes against the published engine**

```bash
cd ui
npx tsc --noEmit -p tsconfig.json && echo "TSC PASS"
npm run lint && echo "LINT PASS"
npm run build:lib && echo "BUILD PASS"
```

Expected: all three print their PASS line. `node_modules` still holds the
*published* engine from Step 1 — that is the point, the typecheck no longer
depends on it.

- [ ] **Step 5: Verify it still passes against the local engine**

```bash
cd ui
npm run wasm:local          # requires wasm-pack; builds ../wasm and overlays it
npx tsc --noEmit -p tsconfig.json && echo "TSC PASS (local engine)"
```

Expected: PASS. Both engines must typecheck, since CI will use the local one and
consumers will use a published one.

- [ ] **Step 6: Commit**

```bash
git add ui/src/engines/versionCheck.ts
git commit -F - <<'EOF'
fix(ui): read engine_version off the namespace, not a named import

A named import binds at module-link time, so an engine released before
engine_version existed fails with a bundler error naming a missing export
— in exactly the situation this module was written to explain in prose.
Reading the symbol off a namespace import turns its absence into a value
this code can interpret, and an engine with no engine_version is reported
as what it is: older than the build checking it.

It also unblocks the type-check. No published dataflow-wasm exports the
symbol, so tsc failed TS2614 against any registry-resolved engine, which
is what CI installs.
EOF
```

---

### Task 2: Build the engine from the checkout in CI's `ui` job

The `ui` job resolves the engine from npm, which cannot work for a UI change
that uses an export added in the same commit — the only engine with that export
is published by a release gated on this very job. Build it instead.

**Files:**
- Modify: `.github/workflows/ci.yml:200-206` (replace the comment and the
  `Install dependencies` step; steps at `:208-219` stay as they are)

**Interfaces:**
- Consumes: `ui/scripts/use-local-wasm.mjs` via the existing `wasm:local` npm
  script. It runs `wasm-pack build ../wasm --target web --out-dir pkg
  --release`, gates on `wasm/scripts/verify-wasm.mjs`, then copies
  `dataflow_wasm.js`, `dataflow_wasm_bg.wasm` **and** `dataflow_wasm.d.ts` into
  `ui/node_modules/@goplasmatic/dataflow-wasm/`. It exits non-zero if wasm-pack
  is missing, if the build fails, or if verification fails.
- Produces: nothing other tasks consume.

- [ ] **Step 1: Confirm the sequence works locally before encoding it in YAML**

```bash
cd ui
npm ci                                   # published engine
npm run build:lib                        # EXPECT FAIL only if Task 1 is not yet applied
npm run wasm:local                       # build + verify + overlay this checkout's engine
npm run lint && npm run build:lib && echo "CI SEQUENCE PASS"
```

Expected: the final line prints. This is the exact sequence the job will run.

- [ ] **Step 2: Replace the npm-resolution comment and step**

In `.github/workflows/ci.yml`, replace lines 200-206 — that is, this block:

```yaml
      # Resolves @goplasmatic/dataflow-wasm from npm at the version pinned in
      # package.json rather than building it locally. That is enough to catch
      # UI-side build and typing regressions; release.yml is what validates the
      # UI against this release's freshly-built wasm.
      - name: Install dependencies
        run: npm ci
        working-directory: ui
```

with:

```yaml
      # Builds the engine from this checkout instead of resolving it from npm.
      # Resolving from npm cannot work in general: a UI change that uses a wasm
      # export added in the same commit has no published engine to compile
      # against, and the release that would publish one is gated on this job.
      # release.yml is the counterpart — it validates the UI against the engine
      # it has just published, which is what consumers actually install.
      - name: Setup Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown

      - name: Cache cargo
        uses: actions/cache@v6
        with:
          path: |
            ~/.cargo/registry/index/
            ~/.cargo/registry/cache/
            ~/.cargo/git/db/
            target/
          key: ${{ runner.os }}-cargo-ui-${{ hashFiles('**/Cargo.lock') }}
          restore-keys: ${{ runner.os }}-cargo-ui-

      # Prebuilt binary, as in the `wasm` job. release.yml pins a version
      # instead, because that job produces the artifact users install; this one
      # only has to catch regressions.
      - name: Install wasm-pack
        run: curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh

      - name: Install dependencies
        run: npm ci
        working-directory: ui

      # Builds ../wasm, refuses to proceed if verify-wasm.mjs rejects the
      # binary, then overlays the glue, the binary and the type declarations
      # onto the package npm ci just installed.
      - name: Overlay this checkout's engine
        run: npm run wasm:local
        working-directory: ui
```

The cache key is `cargo-ui-`, deliberately distinct from the `wasm` job's
`cargo-wasm-`, so the two jobs do not write the same cache entry concurrently.

- [ ] **Step 3: Verify the workflow file is still valid YAML**

```bash
cd /Users/codetiger/Development/Plasmatic/dataflow-rs
python3 -c "import yaml,sys; d=yaml.safe_load(open('.github/workflows/ci.yml')); \
  print('steps in ui job:'); [print(' -', s.get('name')) for s in d['jobs']['ui']['steps']]"
```

Expected: parses without error, and prints, in order: Checkout, Setup Node.js,
Setup Rust, Cache cargo, Install wasm-pack, Install dependencies, Overlay this
checkout's engine, Lint, Type-check and build library.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -F - <<'EOF'
ci: build the engine from the checkout in the ui job

The job resolved dataflow-wasm from npm, which cannot work for a UI change
that uses an export added in the same commit: the only engine carrying it
is published by a release that is itself gated on this job. That is a
cycle, and it is why the branch adding engine_version could not go green.

The ui job now builds ../wasm and overlays it through the wasm:local
script the repo already uses for local development, so the UI is checked
against the engine it ships beside. The engine is consequently built twice
per run, here and in the wasm job; that is accepted to keep the two jobs
independent and running in parallel.
EOF
```

---

### Task 3: Build against the published engine in `publish-ui`

`publish-ui` runs `npm ci` — which resolves the lockfile's pin on the *previous*
release — and then overlays `dataflow_wasm.js` and `dataflow_wasm_bg.wasm` from
the build artifact, but not `dataflow_wasm.d.ts`. So its `tsc` reads stale
declarations. Since `vite.lib.config.ts` externalizes every dependency, those two
copied files never entered the bundle anyway; the `.d.ts` it omits is the only
file that affects `build:lib`.

Replace the overlay with an explicit install of the version `publish-wasm` just
pushed, so the library is validated against the artifact consumers install.

**Files:**
- Modify: `.github/workflows/release.yml` — delete `:251-255`
  (`Download wasm-pkg artifact`) and `:271-274` (`Copy local WASM build`); add
  two steps after `Install dependencies` (`:268-269`); rewrite the comment at
  `:279-283`; extend the comment at `:237`.

**Interfaces:**
- Consumes: `needs.validate.outputs.version` (already wired), and the npm
  registry state produced by the `publish-wasm` job.
- Produces: nothing other tasks consume.

- [ ] **Step 1: Test the wait loop against a version that exists**

Before wiring it in, prove the loop's two outcomes. Run from anywhere:

```bash
VERSION=3.2.0
PKG="@goplasmatic/dataflow-wasm@$VERSION"
for i in $(seq 1 30); do
  if npm view "$PKG" version >/dev/null 2>&1; then echo "resolvable on attempt $i"; break; fi
  echo "waiting ($i/30)"; sleep 10
done
```

Expected: prints `resolvable on attempt 1` immediately (3.2.0 is published).

- [ ] **Step 2: Test the wait loop against a version that does not exist**

Use a shortened bound so this finishes quickly:

```bash
PKG="@goplasmatic/dataflow-wasm@99.99.99"
for i in $(seq 1 2); do
  if npm view "$PKG" version >/dev/null 2>&1; then echo "resolvable"; break; fi
  echo "waiting ($i/2)"; sleep 2
done
echo "loop finished without resolving (this is the timeout path)"
```

Expected: two `waiting` lines, then the timeout line. Confirms `npm view`
returns non-zero for a missing version rather than hanging or erroring oddly.

- [ ] **Step 3: Delete the artifact download**

Remove lines 251-255 of `.github/workflows/release.yml` entirely:

```yaml
      - name: Download wasm-pkg artifact
        uses: actions/download-artifact@v8
        with:
          name: wasm-pkg
          path: wasm/pkg/
```

Leave `needs: [validate, publish-wasm]` at line 245 untouched — it still
sequences the two jobs, which is what makes the published version available.

- [ ] **Step 4: Delete the local overlay**

Remove lines 271-274 (post-deletion they will have shifted up by five):

```yaml
      - name: Copy local WASM build
        run: |
          cp wasm/pkg/dataflow_wasm.js ui/node_modules/@goplasmatic/dataflow-wasm/
          cp wasm/pkg/dataflow_wasm_bg.wasm ui/node_modules/@goplasmatic/dataflow-wasm/
```

- [ ] **Step 5: Add the wait and the install**

Immediately after the `Install dependencies` step (`run: cd ui && npm ci`),
insert:

```yaml
      # `npm ci` above resolved the lockfile, which pins the *previous*
      # release's engine. publish-wasm has since published this release's, so
      # install that explicitly: the library is then type-checked and built
      # against the exact artifact consumers will install, rather than against
      # a local build that would never reach them.
      #
      # The wait absorbs registry propagation. publish-wasm finished moments
      # ago and the new version is not always immediately resolvable; without
      # this the release would fail spuriously after the crate and the engine
      # had already published.
      - name: Wait for the published engine to be resolvable
        run: |
          VERSION="${{ needs.validate.outputs.version }}"
          PKG="@goplasmatic/dataflow-wasm@$VERSION"
          for i in $(seq 1 30); do
            if npm view "$PKG" version >/dev/null 2>&1; then
              echo "$PKG resolvable on attempt $i"
              exit 0
            fi
            echo "waiting for $PKG (attempt $i/30)"
            sleep 10
          done
          echo "::error::$PKG did not become resolvable within 5 minutes"
          exit 1

      - name: Install this release's published engine
        run: |
          VERSION="${{ needs.validate.outputs.version }}"
          cd ui && npm install "@goplasmatic/dataflow-wasm@$VERSION"
```

- [ ] **Step 6: Rewrite the now-false comment on the dep-sync step**

The comment above `Sync dataflow-wasm dep to release version` still describes
the deleted overlay. Replace it:

```yaml
      # The repo pins dataflow-wasm at the last *published* version so `npm ci`
      # can resolve the lockfile before this release's engine exists. The step
      # above has already installed the release version, which npm records with
      # its default caret prefix; restate it explicitly so the published
      # package.json declares dataflow-wasm@^X regardless of any local
      # save-prefix configuration.
      - name: Sync dataflow-wasm dep to release version
```

Keep the step itself exactly as it is.

- [ ] **Step 7: Note that the uploaded artifact is now diagnostic-only**

Nothing downloads `wasm-pkg` any more. Keep the upload — it is the only record
of exactly what was published — but say so. Extend the step at line 237:

```yaml
      # Kept after publish-ui stopped consuming it: nothing downloads this now,
      # but it is the only record of the exact bytes that went to npm, which is
      # what a post-mortem on a bad release needs.
      - name: Upload wasm-pkg artifact
```

- [ ] **Step 8: Verify the workflow file is still valid YAML**

```bash
cd /Users/codetiger/Development/Plasmatic/dataflow-rs
python3 -c "import yaml; d=yaml.safe_load(open('.github/workflows/release.yml')); \
  print('publish-ui steps:'); [print(' -', s.get('name')) for s in d['jobs']['publish-ui']['steps']]"
```

Expected: parses, and prints in order: Checkout, Setup Node.js, Sync UI version
from Cargo.toml, Install dependencies, Wait for the published engine to be
resolvable, Install this release's published engine, Build UI library, Sync
dataflow-wasm dep to release version, Check if version exists on npm, Publish to
npm. No `Download wasm-pkg artifact`, no `Copy local WASM build`.

- [ ] **Step 9: Confirm no dangling references to the removed artifact path**

```bash
grep -n "wasm/pkg\|wasm-pkg" .github/workflows/release.yml
```

Expected: hits only inside the `publish-wasm` job (the build, the generated
`package.json`, `verify-wasm.mjs`, and the upload). No hit inside `publish-ui`.

- [ ] **Step 10: Commit**

```bash
git add .github/workflows/release.yml
git commit -F - <<'EOF'
ci(release): build the UI against the published engine

publish-ui ran npm ci, which resolves the lockfile's pin on the previous
release, and then copied dataflow_wasm.js and dataflow_wasm_bg.wasm from
the build artifact over node_modules — but not dataflow_wasm.d.ts. So the
type-check read the previous release's declarations, and a UI depending on
a new engine export could not build.

Those two files were never load-bearing: vite.lib.config.ts externalizes
every dependency, so the engine has never been part of the published
bundle, and only the .d.ts affects build:lib. Copying a local build over
the dependency also meant the job never exercised what consumers install.

It now waits for the version publish-wasm just pushed and installs it. The
wait is bounded at five minutes and exists because registry propagation is
not instant; failing there would strand a release whose crate and engine
had already published, and every publish job is guarded by an
already-published check so re-running is safe.
EOF
```

---

### Task 4: Correct the stale cross-reference in the overlay script

`ui/scripts/use-local-wasm.mjs` opens by saying it mirrors what `publish-ui`
does. After Task 3 that is no longer true, and the script is now CI and local
tooling rather than a copy of the release pipeline.

**Files:**
- Modify: `ui/scripts/use-local-wasm.mjs:1-25` (header comment only — no code
  changes)

**Interfaces:**
- Consumes: nothing.
- Produces: nothing. Behaviour is unchanged; this task only edits comments.

- [ ] **Step 1: Rewrite the stale paragraph**

Replace this paragraph in the header comment (lines 5-8):

```js
// This mirrors what release.yml's publish-ui job already does: the dependency
// stays pinned to a *published* version so `npm ci` can resolve the lockfile,
// and the fresh build is copied over node_modules. Using a `file:` dependency
// instead would break that resolution.
```

with:

```js
// The dependency stays pinned to a *published* version so `npm ci` can resolve
// the lockfile before a matching engine exists; a `file:` dependency would
// break that resolution. This script is what bridges the gap, and ci.yml's ui
// job runs it for the same reason a developer does. release.yml deliberately
// does the opposite — it installs the engine it has just published, so the
// release validates what consumers actually receive.
```

- [ ] **Step 2: Confirm the script still runs**

```bash
cd ui && npm run wasm:local
```

Expected: builds, verifies, prints the three `copied` lines, exits 0. Comment-only
edits cannot change this, but run it to be sure nothing was truncated.

- [ ] **Step 3: Commit**

```bash
git add ui/scripts/use-local-wasm.mjs
git commit -F - <<'EOF'
docs(ui): correct the overlay script's cross-reference

The header said the script mirrors publish-ui. That job now installs the
engine it has just published rather than overlaying a local build, so the
two deliberately differ. Describe the split instead: this script exists so
a checkout can run against its own engine, in CI and on a developer's
machine, while the release validates the published artifact.
EOF
```

---

### Task 5: Verify the whole gate end to end

**Files:**
- No changes. Verification only.

**Interfaces:**
- Consumes: everything from Tasks 1-4.
- Produces: nothing.

- [ ] **Step 1: Run the full Rust gate**

Nothing in this plan touches Rust, so these must be unchanged from baseline.

```bash
cd /Users/codetiger/Development/Plasmatic/dataflow-rs
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy -p dataflow-rs --all-targets -- -D warnings
cargo test --workspace --all-features 2>&1 | grep -E "^test result:" | awk '{s+=$4; f+=$6} END {print "PASSED:", s, "FAILED:", f}'
cargo test -p dataflow-rs 2>&1 | grep -E "^test result:" | awk '{s+=$4; f+=$6} END {print "PASSED:", s, "FAILED:", f}'
```

Expected: fmt silent, both clippy runs clean, `PASSED: 444 FAILED: 0` and
`PASSED: 371 FAILED: 0`.

- [ ] **Step 2: Reproduce CI's `ui` job exactly**

```bash
cd ui
npm ci
npm run wasm:local
npm run lint
npm run build:lib
echo "UI JOB PASS"
```

Expected: `UI JOB PASS`.

- [ ] **Step 3: Prove the published-engine path also builds**

This is what `publish-ui` will do, with the currently published engine standing
in for the one the release would publish.

```bash
cd ui
npm ci                      # published engine only — no overlay this time
npm run build:lib
echo "PUBLISHED-ENGINE PATH PASS"
```

Expected: `PUBLISHED-ENGINE PATH PASS`. This only holds because of Task 1; it is
the check that the release will not fail on a type error.

- [ ] **Step 4: Restore the local overlay and confirm the tree is clean**

```bash
cd ui && npm run wasm:local
cd .. && git status --porcelain
```

Expected: `git status` prints nothing. `npm ci` in the previous steps may rewrite
`ui/package-lock.json`; if it shows as modified, inspect the diff and revert it
with `git checkout -- ui/package-lock.json` unless the change is intended.

---

## Notes for the implementer

- **Task order matters.** Task 1 must land before Task 2's Step 1 will pass, and
  before Task 5 Step 3 can succeed at all.
- **Task 1 alone turns CI green.** Once the typecheck no longer depends on the
  published `.d.ts`, the `ui` job passes without any workflow change. Task 2 is
  not what fixes the build — it is what makes CI actually exercise the engine
  from the commit under test, which is a separate requirement.
- **The workflow changes cannot be executed locally.** GitHub Actions is not run
  here. The YAML parse checks plus the locally-reproduced step sequences are the
  available verification; do not claim the workflows are "tested".
- Reference: `docs/superpowers/specs/2026-08-11-ci-release-wasm-resolution-design.md`.

### Explicitly out of scope

Do not do these, even though they look adjacent:

- **Pinning `ui/package.json`'s wasm dependency to an exact version.** The caret
  range is load-bearing: `npm ci` has to resolve before the matching engine is
  published. An exact pin breaks local installs until release day.
- **Adding `deny_unknown_fields` to `Workflow`.** It is the root cause of the
  silent-field-drop this version check works around, but it is a semver-visible
  change to the crate and needs its own decision.
- **Adding a test runner to `ui/`.** There is none today; `tsc`, `eslint` and
  `build:lib` are the verification surface.
- **Changing `Cargo.lock`.** Nothing here touches Rust. If `wasm-pack` refreshes
  it as a side effect of `wasm:local`, revert it with
  `git checkout -- Cargo.lock` before committing.
