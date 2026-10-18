# Contributing to dataflow-rs

Thanks for your interest in contributing. This guide covers what you need to
get a change merged.

## Repository Layout

This is a Cargo workspace with four published artifacts that share one
version number, two members that are never published, and the book:

| Path | Artifact | Registry |
|---|---|---|
| `.` | `dataflow-rs`, the engine | crates.io |
| `derive/` | `dataflow-rs-derive`, the `#[derive(FunctionConfig)]` macro behind it | crates.io |
| `wasm/` | `@goplasmatic/dataflow-wasm`, the engine for the browser and Node | npm |
| `ui/` | `@goplasmatic/dataflow-ui`, the React debugger | npm |
| `docs/` | mdBook user guide, deployed to GitHub Pages | — |
| `docs-tests/` | Compiles the book's and the README's Rust examples as doctests | — |
| `corpus/` | The corpus: the cases the engine's tests load, and the tools that read them | — |

`wasm/` and `derive/` are workspace members; `ui/` is a separate npm project
driven by the release workflow. The root depends on the derive by path *and*
by version, because `cargo publish` refuses a path dependency without one, so
a release publishes the derive first.

## Prerequisites

- **Rust 1.98 or later** (Edition 2024). This is the declared MSRV and CI
  enforces it. The floor is inherited rather than chosen: `datalogic-rs` 5.8
  and `datavalue-rs` 0.3 both declare `rust-version = "1.98"`, so the crate
  cannot build on less. Let-chains (`if let ... && let ...`, stable since
  1.88) are therefore allowed, and clippy's `collapsible_if` asks for them.
- **Node.js 22**, only if you are touching `ui/`. CI and the release
  workflow run that version.
- **wasm-pack**, if you are touching `wasm/` or running the `ui/` debugger
  against unreleased engine changes (see *Running the debugger against your
  engine changes*).

## Getting Started

```bash
git clone https://github.com/GoPlasmatic/dataflow-rs
cd dataflow-rs
cargo test --workspace --all-features
```

Every test should pass: the root's tests and doctests, the derive's, the
corpus crate's and `docs-tests`' (the book's and the README's doctests).

The count is feature-dependent. The optional operator families are `#[cfg]`-gated
on both sides: some tests only exist when a family is on, others only when it is
off. The default build therefore reports a different number:

```bash
cargo test
```

A bare cargo command at the root selects the root package; every other member
takes `-p`, as in `cargo test -p dataflow-corpus` or
`cargo test -p dataflow-docs-tests`. Use the bare command for the default
build, not `--workspace`: `wasm/` depends on the root with `all-operators`, and
cargo unifies features across the members one invocation selects, so
`--workspace` would turn every family back on.

### Running the debugger against your engine changes

`ui/package.json` pins `@goplasmatic/dataflow-wasm` to the last **published**
version, because `npm ci` has to resolve it before a matching engine exists,
both here and in `release.yml`. `release.yml` installs that release's own
just-published engine instead of overlaying a local build, so the release
validates what consumers receive. A plain `npm run dev` therefore runs the
debugger against the last release, not your checkout.

The mismatch can hide bugs. An older engine reads a key it predates as an
unknown key: the definition still loads, with the key kept in the node's `ext`
and an issue reported, and does something other than what it says.

Two mechanisms keep that from biting you:

```bash
cd ui
npm run dev            # predev builds your engine and overlays it
```

`npm run dev` runs `wasm:local` first, which rebuilds `wasm/pkg`, verifies it,
and copies it over `node_modules`. It always rebuilds rather than reusing
whatever is already there. A stale build usually carries the *same* version as
your checkout, so no version check can see it. wasm-pack no-ops in about two
seconds when nothing changed.

If wasm-pack is not installed the step warns and continues, so a contributor
touching only `ui/` is not blocked. The second mechanism makes that safe: the
engine exports `engine_version()`, and `WasmEngineAdapter`
throws when the loaded engine is **older** than the `dataflow-ui` build using
it. A newer engine passes silently, since the dependency is a caret range and
npm may legitimately resolve one. Run `npm run wasm:local` by hand after any engine
change if you keep a dev server running.

Overlaying also clears `node_modules/.vite`, which is required: Vite keys its
dependency cache off the lockfile, which does not change when files are
overwritten in place, so without it the dev server keeps serving the old JS
glue against the new binary. That pairing fails confusingly: init succeeds, and
only a later mangled-name lookup throws.

This overlay is deliberately one-directional: `predev` and `wasm:local` are
the only scripts that call `use-local-wasm.mjs`. `build:lib` never does, and
`prepublishOnly` (and therefore `npm publish`) runs it. Keep it that way. `release.yml` installs the engine it has just published immediately
before running `build:lib`; a future `prebuild:lib` hook that overlaid a local
build would silently overwrite that published engine partway through
`publish-ui`, undoing the point of installing it.

`npm run wasm:local` refuses to overlay a build that fails
`wasm/scripts/verify-wasm.mjs`, the same guard the release pipeline runs
before publishing.

## Before You Open a Pull Request

CI runs these and treats warnings as errors, so run them locally before you
push:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy --all-targets -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
```

`--all-targets` covers examples, tests, and benches. `--all-features` covers the
`wasm-web` feature, which is otherwise silently skipped. The second clippy line
is not redundant: bare at the root, it lints the root's **default** build,
which is what `cargo add dataflow-rs` delivers, and `--workspace` cannot reach
it for the feature-unification reason above.

Do not skip the `cargo doc` line. Clippy does not lint rustdoc, so nothing above
catches a broken intra-doc link, a link from public docs into a private item, or
an unbackticked generic like `Arc<Logic>` that rustdoc parses as an HTML tag.
All of these ship silently to docs.rs as dead or mangled text.
CI runs it under `-D warnings` like everything else. `src/lib.rs` also warns on
`missing_docs` and `missing_debug_implementations`, which `-D warnings` makes
errors: every public item needs a doc comment and every public type a `Debug`
impl.

Lint *policy* lives in `[workspace.lints]` in the root `Cargo.toml` and is
inherited by every member, so a bare `cargo clippy` and rust-analyzer
enforce what CI enforces. Adding a lint there means fixing every existing
violation in the same change; CI's `-D warnings` gives no grace period.

The root has checks of its own beside the workspace-wide ones. CI runs all of
them; run the ones your change touches:

```bash
cargo clippy -p dataflow-rs-derive --all-targets -- -D warnings
cargo test
cargo test -p dataflow-docs-tests
cargo test --features ts --test ts_types
for f in datetime ext-string ext-array ext-math ext-control ext-object \
         error-handling all-operators tensor budget; do
  cargo test --no-default-features --features "$f"
done
```

The `ts` run holds
`bindings/dataflow.d.ts`, generated from the wire types, to the committed
file, doc comments included: a doc edit on a wire type means regenerating it
with `DATAFLOW_TS_WRITE=1 cargo test --features ts --test ts_types`. The
per-family loop exists because enabling an operator family is not a no-op,
so a family must pass alone, not only under `--all-features`.

`cargo test` at the root must stay under a minute, rebuild included. The
run-context model test runs once in the default build, at 150 sequences per
tuning; `DATAFLOW_V4_DEEP=1 cargo test --lib state::` runs its full 2,000.
Run it after any change under `src/state/`.

If you touched `wasm/`:

```bash
cargo clippy -p dataflow-wasm --target wasm32-unknown-unknown --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown --features wasm-web -- -D warnings
cargo clippy --target wasm32-unknown-unknown --features wasm-web,all-operators -- -D warnings
cd wasm
wasm-pack test --node
wasm-pack build --target web --out-dir pkg --release && node scripts/verify-wasm.mjs pkg
```

`wasm-pack test --node` runs the engine on wasm32 under Node; the release build
plus `verify-wasm.mjs` is what the release pipeline runs, and it checks that
the package's `.d.ts` carries the engine's wire types.

If you touched `ui/`:

```bash
cd ui && npm ci && npm run wasm:local && npm run lint && npm test && npm run build:lib
```

`npm test` is vitest over the engine `wasm:local` overlaid: the trace helpers
over real runs, and the debugger rendering a failing run.

`lint` runs with `--max-warnings 0`, so a single warning fails CI; `build:lib`
is `tsc && vite build`, so it type-checks the whole package as well as producing
the artifact, and its `postbuild:lib` hook verifies the emitted `lib.d.ts`.

If you changed dependencies:

```bash
cargo deny --all-features check
```

If you touched anything under `src/`, confirm you have not raised the MSRV:

```bash
rustup toolchain install 1.98
cargo +1.98 check --workspace --all-targets --all-features --locked
```

## Documentation

The mdBook source lives in `docs/src/`; `docs/book/` is generated output and is
not tracked. To preview:

```bash
mdbook serve docs
```

Every Rust snippet in **both** `README.md` and `docs/src/` compiles against
the crate at the root, through the `dataflow-docs-tests` workspace member:

- `README.md`: as `ReadmeDoctests` in `docs-tests/src/lib.rs`.
- `docs/src/**.md`: through the same crate, which includes each book page with
  `#[doc = include_str!(…)]`. Its `tests/coverage.rs` fails if a page is
  missing from the page list.

(`mdbook test` cannot do this. It only passes `-L` to rustdoc, and an
edition-2018+ `use dataflow_rs::…` needs `--extern`, which mdBook has no flag
for. Routing through Cargo makes the crate resolvable. The crate is separate
from the root because `docs/` is not shipped in the published package.)

So an edit to either can break the build:

```bash
cargo test -p dataflow-docs-tests
```

Three conventions when writing snippets:

- **Every Rust fence compiles.** Do not tag a fence `ignore`: an `ignore` is an
  unverified claim in user-facing docs. A snippet that would not compile on
  its own gets a hidden preamble instead.

- **Fragments need a hidden preamble.** Lines prefixed with `# ` are compiled by
  rustdoc but hidden from readers by mdBook, so a snippet that assumes an
  `engine` or `message` binding can declare one invisibly:

  ```text
  # use dataflow_rs::{Engine, Message, RunOptions};
  # async fn _demo(engine: Engine, mut message: Message) {
  let run = engine.run_with(&mut message, &RunOptions::default()).await;
  # let _ = run;
  # }
  ```

  A hidden wrapper that is never called type-checks the snippet without
  executing it, which suits examples containing placeholder JSON
  like `"tasks": [...]`. A snippet that shows a value asserts it, so the
  doctest verifies what the prose claims.

- **Tag every other fence with its language.** Unlabelled fences (```` ``` ````
  with no language) are treated as **Rust** by rustdoc. Tag definitions `json`,
  commands `bash`, manifests `toml`, and ASCII diagrams and sample output
  `text`. Only ```` ```rust ```` fences are compiled, so the JSON in a page is
  not checked; read it against the definition types in `src/def/types.rs`.

## The Corpus

`corpus/cases/<area>/<name>.json` holds one case per behaviour: definitions,
an engine configuration and the steps run against it. The engine's tests load
them through the `dataflow-corpus` crate, which also holds an order-sensitive
JSON value, normalisation, and independent models of the dot-path rules and
of trace replay that the engine is held to. Every case type refuses unknown
keys, so a typo fails loudly. See `corpus/README.md`.

## Benchmarks

The root's examples include its thirteen benchmarks. Always build them with
`--release`; a debug build measures nothing useful.

```bash
cargo run --example realistic_benchmark --release        # ISO 20022 -> SwiftMT-style workload
cargo run --example benchmark --release                  # Throughput + latency percentiles
cargo run --example micro_cond_bench --release           # Condition-eval / trivially-true folding
cargo run --example micro_aggregate_bench --release      # Aggregate-heavy (reduce/map)
cargo run --example micro_subtree_write_bench --release  # k map writes into one depth-2 subtree
cargo run --example micro_multiworkflow_bench --release  # N chained workflows, one message
cargo run --example async_handler_benchmark --release    # Marginal cost of one host function call
cargo run --example map_performance_test --release       # 10 chained mappings across all 3 roots
cargo run --example loop_scaling --release               # 1000-sweep append loop over a large context
cargo run --example large_context_handler --release      # Five expression fields over a 2 MiB context, per call
cargo run --example fan_out --release                    # for_each over 100 elements on a 2 MiB context
cargo run --example orion_connector --release            # Connector-shaped host function: document folds, output, a layer
cargo run --example observed_run --release               # The realistic workload with a counting observer
```

Every benchmark checks its result before it times anything, so a workload
that silently does less than it says fails rather than reporting a plausible
number, and prints one machine-readable `GATE {...}` line last.
`bench/gate.py` builds the examples, runs them round-robin and compares two
builds, a base commit against a candidate. The examples share
`examples/shared/`, which is not an example itself.

CI runs every example once, in smoke mode: `DATAFLOW_BENCH_SMOKE=1` makes a
benchmark run its startup check, then a few iterations, and mark its `GATE`
line `"smoke"`. Run it locally with `cargo build --examples` and `python3
bench/gate.py smoke --bins target/debug/examples`. A new benchmark reads its
iteration counts through `gate::count(full, smoke)` and is added to
`bench/gate.py`'s `BENCHES`.

Results carry roughly ±2–3% run-to-run noise plus occasional transient P99
spikes. **Compare the mean of at least 3 runs** before claiming a regression or
an improvement, and **interleave A/B runs**: running all the "before" runs and
then all the "after" runs conflates the change with thermal drift. Build both
binaries first, discard the first run of each as cold, and alternate. Include
your machine's core count with any numbers you quote.

Performance changes do not need to beat the baseline to be merged, but a PR that
knowingly regresses the hot path should say so and explain the tradeoff.

## Commit Messages

The repository follows [Conventional Commits](https://www.conventionalcommits.org/):

```text
feat(run): the run's deadline is checked at every host call
fix(map): preserve array structure on nested writes
docs: correct MSRV in installation guide
chore(deps): bump quick-xml to 0.42
```

Release notes are generated from commit history, so a clear subject line ends up
in front of users.

## Pull Requests

**Which branch.** `main` is dataflow-rs 4.0; every change goes there.

1. Fork the repository and branch from `main`
2. Make your change, with tests covering it
3. Run the checks above
4. Open a PR describing what changed and why

Add tests next to the code they cover in a `mod tests` block for unit-level
changes. Behaviour that spans the engine goes in `tests/it/`, one test binary
with a module per topic: the engine end to end (`run_basics.rs`,
`run_loops.rs`, `host_calls.rs`, `fan_out.rs`, `deadline.rs`,
`error_context.rs`, `observer.rs`, `trace.rs`, `acceptance.rs`), the
definitions and the compiler (`parse.rs`, `compat.rs`, `compile.rs`,
`checks.rs`, `fingerprint.rs`, `program.rs`, `registry.rs`,
`repository_definitions.rs`), functions and their configs (`derive.rs`,
`config_fields.rs`, `documents.rs`, `function_testing.rs`, `expr.rs`,
`path.rs`, `operators.rs`), the wire shapes (`wire.rs`, `run_json.rs`), and
the feature-gated `ops_budget.rs`. Pick the module whose topic already
matches, and add a new one, listed in `tests/it/main.rs`, only for a new area.
Run one with `cargo test --test it run_basics::`.

A fixture more than one module needs belongs in `tests/it/support/`; every
golden file goes through `support::golden`. Two tests are their own binaries
and stay so: `tests/secrets_isolation.rs` installs the process's logger, and
`tests/ts_types.rs` (built only under `ts`) is named in the bindings' header.

CI skips docs-only and Markdown-only changes by design, so a PR
touching only those paths will not show Rust checks.

## Adding a Function

A host function implements `Function` with a `#[derive(FunctionConfig)]`
config, and a task names it by the name it is registered under:

```rust
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::Output;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::{Ctx, Expr, Function, Outcome, Result};

#[derive(FunctionConfig)]
struct MyConfig {
    query: Expr<String>,
    #[output(default = "data.result")]
    output: Output,
}

struct MyFunction;

impl Function for MyFunction {
    type Config = MyConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &MyConfig) -> Result<Outcome> {
        let query = ctx.eval(&cfg.query)?;
        ctx.emit_default(OwnedDataValue::String(query.into_owned()))?;
        Ok(Outcome::Ok)
    }
}
```

Write through `ctx.emit_default`, `ctx.emit` or `ctx.set`, so the audit trail's
changes are recorded for you and the writes land only when the call returns
`Ok`, and return the `Outcome` variant that matches intent rather than encoding
control flow in a status code. `function::testing::call` runs one call through
the real compile, overlay and layers, for a unit test without a workflow.

The eight built-ins (`src/function/builtins/`) are different: they run inline
on the run's view, their names are reserved, and each is part of the
engine's contract, so adding one is a contract change, not a
registration. It needs a case in the corpus, a row in `BUILTIN_FUNCTIONS`
(which the wasm package's `engine_facts()` and the UI read), a page in
`docs/src/built-in-functions/` and a row in the README's table.

## Releases

Releases are tag-gated and maintainer-run. Pushing to `main` runs CI and never
publishes; the documentation site deploys by hand until 4.0 is released. A `v*` tag triggers `.github/workflows/release.yml`,
which validates that the tag matches the root `Cargo.toml` version, that
`wasm/` (its own version and its `dataflow-rs` requirement), `derive/` (its
version and the root's requirement on it), `ui/` and the install snippets in
`README.md` and `docs/src/` agree, and that `CHANGELOG.md` has the version's
section; then publishes the derive, the crate and both npm packages, and
cuts a GitHub release whose notes are that section. The `CRATES_IO_TOKEN`
secret needs the `publish-new` scope for a crate's first release, the
derive's included. The four published
versions move together, the derive's included: the root's manifest names
`dataflow-rs-derive` from crates.io at exactly the root's version, so the
derive publishes first. Contributors do not need to bump versions in a PR,
but a user-visible change gets a line under `CHANGELOG.md`'s `[Unreleased]`.

## Security

Do not report vulnerabilities through public issues. See
[SECURITY.md](SECURITY.md) for the private reporting process.

## License

By contributing, you agree that your contributions will be licensed under the
[Apache License 2.0](LICENSE).
