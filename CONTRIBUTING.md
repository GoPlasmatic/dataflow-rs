# Contributing to dataflow-rs

Thanks for your interest in contributing! This guide covers what you need to
get a change reviewed and merged.

## Repository Layout

This repository is a Cargo workspace with four published packages that share a
single version number, two internal test crates, and the documentation book:

| Path | Package | Registry |
|---|---|---|
| `.` | `dataflow-rs` — the core rules engine | crates.io |
| `derive/` | `dataflow-rs-derive` — the `#[derive(FunctionConfig)]` procedural macro | crates.io |
| `wasm/` | `@goplasmatic/dataflow-wasm` — WebAssembly bindings for browsers and Node.js | npm |
| `ui/` | `@goplasmatic/dataflow-ui` — React visualizer and step-by-step debugger | npm |
| `docs/` | mdBook user guide, deployed to GitHub Pages | none |
| `docs-tests/` | Compiles Rust code blocks in the book and README as doctests | none |
| `corpus/` | Shared test cases and reference models used by the test suite | none |

`wasm/` and `derive/` are Cargo workspace members, while `ui/` is a standalone
npm package managed by the release workflow. The version number is defined once
in `[workspace.package] version` in the root `Cargo.toml`. All published Rust
crates inherit this version and reference internal crates via
`[workspace.dependencies]`. `scripts/check-versions.sh` verifies that the npm
packages and documentation version pins stay in sync on every pull request.

## Prerequisites

- **Rust 1.98 or later** (Edition 2024). This is the minimum supported Rust
  version (MSRV) enforced by CI, required by `datalogic-rs` 5.8 and
  `datavalue-rs` 0.3. Let-chains (`if let ... && let ...`, stabilized in Rust
  1.88) are supported and expected by Clippy's `collapsible_if` lint.
- **Node.js 22**, required only if you are working on `ui/`.
- **wasm-pack**, required if you are working on `wasm/` or testing `ui/`
  against local engine changes (see below).

## Getting Started

```bash
git clone https://github.com/GoPlasmatic/dataflow-rs
cd dataflow-rs
cargo test --workspace --all-features
```

All tests should pass, including unit tests, integration tests, derive tests,
corpus tests, and documentation doctests (`docs-tests`).

To test the root crate with its default feature set:

```bash
cargo test
```

> **Note:** Running `cargo` at the root without `--workspace` selects only the
> root package. Use `-p` to target other workspace members (for example,
> `cargo test -p dataflow-corpus` or `cargo test -p dataflow-docs-tests`).
> Avoid passing `--workspace` when testing the default feature set, because
> `wasm/` enables `wasm-web` and `datetime`, which Cargo feature unification
> would enable across the workspace.

### Running the UI Debugger Against Local Engine Changes

In `ui/package.json`, `@goplasmatic/dataflow-wasm` points to the latest
**published** npm version so that `npm ci` works before a new release is
published. To test your local Rust engine changes in the UI, run:

```bash
cd ui
npm run dev            # predev rebuilds wasm/pkg and copies it into node_modules
```

`npm run dev` automatically runs `wasm:local` first, which rebuilds `wasm/pkg`,
runs `wasm/scripts/verify-wasm.mjs`, copies the output into `node_modules`, and
clears `node_modules/.vite` so Vite picks up the updated bundle. If you keep
the dev server running while editing Rust code, run `npm run wasm:local`
manually to refresh the WASM build.

If `wasm-pack` is not installed, `wasm:local` logs a warning and continues with
the published WASM package so UI-only contributors are not blocked. At runtime,
`WasmEngineAdapter` checks `engine_version()` and throws an error if the loaded
WASM module is older than the `dataflow-ui` version.

## Before You Open a Pull Request

CI treats all compiler, Clippy, and rustdoc warnings as errors. Run these
checks locally before pushing:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy --all-targets -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
```

- `--all-targets` checks examples, tests, and benchmarks.
- `--all-features` includes the `wasm-web` feature.
- The second `cargo clippy` command checks the root crate with its **default**
  features.
- `cargo doc` catches broken intra-doc links and unescaped generics in doc
  comments. Every public item must have a doc comment (`missing_docs`) and
  every public type must implement `Debug` (`missing_debug_implementations`).

Workspace lint rules are configured in `[workspace.lints]` in the root
`Cargo.toml` and inherited by all crates.

Additional targeted checks run in CI depending on what you changed:

```bash
cargo clippy -p dataflow-rs-derive --all-targets -- -D warnings
cargo test
cargo test -p dataflow-docs-tests
cargo test --features ts --test ts_types
for f in datetime tensor budget; do
  cargo test --no-default-features --features "$f"
done
```

- If you change a wire type or its doc comments, regenerate
  `bindings/dataflow.d.ts` with:
  `DATAFLOW_TS_WRITE=1 cargo test --features ts --test ts_types`
- If you modify `src/state/`, run the full state-model test suite:
  `DATAFLOW_DEEP=1 cargo test --lib state::`

If you changed `wasm/`:

```bash
cargo clippy -p dataflow-wasm --target wasm32-unknown-unknown --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown --features wasm-web -- -D warnings
cargo clippy --target wasm32-unknown-unknown --features wasm-web,datetime -- -D warnings
cd wasm
wasm-pack test --node
wasm-pack build --target web --out-dir pkg --release && node scripts/verify-wasm.mjs pkg
```

If you changed `ui/`:

```bash
cd ui && npm ci && npm run wasm:local && npm run lint && npm test && npm run build:lib
```

If you changed dependencies:

```bash
cargo deny --all-features check
```

If you changed code under `src/`, verify that it still builds on the MSRV:

```bash
rustup toolchain install 1.98
cargo +1.98 check --workspace --all-targets --all-features --locked
```

## Documentation

The mdBook source lives in `docs/src/` (`docs/book/` is generated output and is
not tracked in git). To preview the book locally:

```bash
mdbook serve docs
```

Every Rust code block in `README.md`, `derive/README.md`, and `docs/src/` is
compiled and run as a doctest via the `dataflow-docs-tests` crate, and every
JSON workflow snippet is validated by `tests/it/repository_definitions.rs`:

```bash
cargo test -p dataflow-docs-tests
cargo test --test it repository_definitions
```

Follow these conventions when writing documentation snippets:

- **Every Rust code block must compile.** Do not use `ignore`. Use hidden `# `
  setup lines if a snippet needs imports or surrounding context:

  ```text
  # use dataflow_rs::{Engine, Message, RunOptions};
  # async fn _demo(engine: Engine, mut message: Message) {
  let run = engine.run_with(&mut message, &RunOptions::default()).await;
  # let _ = run;
  # }
  ```

- **Always specify a language on code fences.** Rustdoc treats unlabeled
  ```` ``` ```` fences as Rust code. Tag JSON snippets with `json` (or `jsonc`
  if they contain `//` comments or `...` placeholders), shell commands with
  `bash`, Cargo manifests with `toml`, and diagrams or plain output with `text`.

## The Corpus

`corpus/cases/<area>/<name>.json` contains declarative test cases that verify
engine behavior against golden files (`tests/golden/corpus/`). The
`dataflow-corpus` crate also provides independent reference implementations of
dot-path resolution and trace replay used by the test suite. See
[corpus/README.md](corpus/README.md) for details.

## Benchmarks

The repository includes 13 benchmark examples. Always run them with
`--release`:

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

Each benchmark verifies its output before timing and prints a machine-readable
`GATE {...}` summary line at the end. You can compare two commits using
`bench/gate.py`, or run a quick smoke check locally with:

```bash
cargo build --examples
python3 bench/gate.py smoke --bins target/debug/examples
```

Benchmark results typically vary by ±2–3% between runs. When measuring
performance changes, **interleave at least 3 A/B runs** after discarding a
warmup run, and include your CPU/core count when sharing numbers in a PR.

## Commit Messages

We follow [Conventional Commits](https://www.conventionalcommits.org/):

```text
feat(run): check run deadline before each host call
fix(map): preserve array structure on nested writes
docs: clarify error handling channels in quick start
chore(deps): bump quick-xml to 0.42
```

## Pull Requests

All changes target the `main` branch.

1. Fork the repository and create a branch from `main`.
2. Make your changes and add tests.
3. Run the verification checks listed above.
4. Add a brief entry under `[Unreleased]` in `CHANGELOG.md` for user-visible changes.
5. Open a pull request explaining what changed and why.

Where to add tests:
- **Unit tests:** Add a `mod tests` block alongside the code you modified.
- **Integration tests:** Add tests to the matching topic module under
  `tests/it/` (for example, `run_basics.rs`, `host_calls.rs`, `compile.rs`,
  `trace.rs`). Run a single module with `cargo test --test it run_basics::`.
  Shared test helpers and golden file utilities live in `tests/it/support/`.

## Adding a Function

Custom host functions implement `Function` with a `#[derive(FunctionConfig)]`
config struct and are registered on `EngineBuilder` by name:

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

Use `ctx.emit_default`, `ctx.emit`, or `ctx.set` to write back to the message
so changes are tracked in the audit trail and rolled back automatically on
error. You can unit-test a custom function directly without building a workflow
using `function::testing::call`.

Built-in functions (`src/function/builtins/`) are part of the core engine
specification and have reserved names. Adding a new built-in function requires
adding corpus test cases, updating `BUILTIN_FUNCTIONS` (used by `engine_facts()`
and the UI), and documenting the function in `docs/src/built-in-functions/` and
`README.md`.

## Releases

Releases are triggered by pushing a `v*` git tag and are run by maintainers via
`.github/workflows/release.yml`. The workflow runs `scripts/check-versions.sh`
to confirm that Cargo manifests, `ui/package.json`, install snippets in the
docs, and `CHANGELOG.md` all match the tag version before publishing to
crates.io and npm.

## Security

Please do not report security vulnerabilities in public issues. See
[SECURITY.md](SECURITY.md) for instructions on private disclosure.

## License

By contributing, you agree that your contributions will be licensed under the
[Apache License 2.0](LICENSE).
