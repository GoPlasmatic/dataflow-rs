# Contributing to dataflow-rs

Thanks for your interest in contributing. This guide covers what you need to
get a change merged.

## Repository Layout

This is a Cargo workspace with three published artifacts that share a version
number:

| Path | Artifact | Registry |
|---|---|---|
| `.` | `dataflow-rs` | crates.io |
| `wasm/` | `@goplasmatic/dataflow-wasm` | npm |
| `ui/` | `@goplasmatic/dataflow-ui` — React debugger | npm |
| `docs/` | mdBook user guide, deployed to GitHub Pages | — |

## Prerequisites

- **Rust 1.85 or later** (Edition 2024). This is the crate's declared MSRV and
  CI enforces it, so avoid language features stabilized after 1.85 — notably
  let-chains (`if let ... && let ...`), which need 1.88.
- **Node.js 20+** — only if you are touching `ui/`.
- **wasm-pack** — if you are touching `wasm/`, or running the `ui/` debugger
  against unreleased engine changes (see *Running the debugger against your
  engine changes*).

## Getting Started

```bash
git clone https://github.com/GoPlasmatic/dataflow-rs
cd dataflow-rs
cargo test --workspace --all-features
```

You should see 444 passing tests (260 unit, 96 integration, 21 doc, 66 docs-page
doctests, 1 docs coverage).

The count is feature-dependent. The optional operator families are `#[cfg]`-gated
on both sides — some tests only exist when a family is on, others only when it is
off — so the default build reports a different number:

```bash
cargo test -p dataflow-rs
```

should report 371 passing (254 unit, 96 integration, 21 doc). Use `-p dataflow-rs`,
not `--workspace`: `dataflow-wasm` depends on `dataflow-rs` with `all-operators`,
and cargo unifies features across workspace members, so `--workspace` would turn
every family back on.

### Running the debugger against your engine changes

`ui/package.json` pins `@goplasmatic/dataflow-wasm` to the last **published**
version, because `npm ci` has to be able to resolve it — both here and in
`release.yml`, which then copies the release's own build over `node_modules`.
So a plain `npm run dev` runs the debugger against the last release, not your
checkout.

That matters more than it sounds. `Workflow` does not set
`deny_unknown_fields`, so a field the published wasm has never heard of is
silently ignored rather than rejected: the debugger runs, and quietly does the
wrong thing. A workflow using an unreleased feature appears to work while
producing results that disagree with the engine you are editing.

Build your engine into the installed package instead:

```bash
cd ui
npm run wasm:local     # wasm-pack build -> verify -> copy over node_modules
npm run dev
```

Re-run it after any engine change, and after every `npm ci` / `npm install` —
both restore the published package. It also clears `node_modules/.vite`, which
is required: Vite keys its dependency cache off the lockfile, which does not
change when files are overwritten in place, so without that the dev server
keeps serving the old JS glue against the new binary. That pairing fails
confusingly, because init succeeds and only a later mangled-name lookup throws.

`npm run wasm:local` refuses to overlay a build that fails
`wasm/scripts/verify-wasm.mjs` — the same guard the release pipeline runs
before publishing.

## Before You Open a Pull Request

CI runs these and treats warnings as errors. Running them locally first is the
difference between a green PR and a red one:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

`--all-targets` covers examples, tests, and benches. `--all-features` covers the
`wasm-web` feature, which is otherwise silently skipped.

If you touched `wasm/`:

```bash
cargo clippy -p dataflow-rs --target wasm32-unknown-unknown --features wasm-web -- -D warnings
cd wasm && wasm-pack test --node
```

If you touched `ui/`:

```bash
cd ui && npm ci && npm run build:lib
```

If you changed dependencies:

```bash
cargo deny --all-features check
```

If you touched anything under `src/`, confirm you have not raised the MSRV:

```bash
rustup toolchain install 1.85
cargo +1.85 check --workspace --all-targets --all-features --locked
```

## Documentation

The mdBook source lives in `docs/src/`; `docs/book/` is generated output and is
not tracked. To preview:

```bash
mdbook serve docs
```

Rust snippets in **both** `README.md` and `docs/src/` are compiled as doctests
by `cargo test`:

- `README.md` — via the `ReadmeDoctests` hook at the bottom of `src/lib.rs`.
- `docs/src/**.md` — via the `dataflow-docs-tests` workspace member, which
  includes each book page with `#[doc = include_str!(…)]`.

(`mdbook test` cannot do this. It only passes `-L` to rustdoc, and an
edition-2018+ `use dataflow_rs::…` needs `--extern`, which mdBook has no flag
for. Routing through Cargo is what makes the crate resolvable.)

So an edit to either can break the build. Two conventions when writing snippets:

- **Fragments need a hidden preamble.** Lines prefixed with `# ` are compiled by
  rustdoc but hidden from readers by mdBook, so a snippet that assumes an
  `engine` or `message` binding can declare one invisibly:

  ```text
  # use dataflow_rs::{Engine, Message};
  # async fn _demo(engine: Engine, mut message: Message) -> dataflow_rs::Result<()> {
  engine.process_message(&mut message).await?;
  # Ok(()) }
  ```

  A hidden wrapper that is never called type-checks the snippet without
  executing it, which is what you want for examples containing placeholder JSON
  like `"tasks": [...]`.

- **Tag genuinely non-compilable blocks `ignore`.** API signature listings
  (`pub fn f() -> T` with no body) are the main legitimate case. Prefer a hidden
  preamble over `ignore` wherever possible — an `ignore` is an unverified claim
  in user-facing docs.

Unlabelled fences (```` ``` ```` with no language) are treated as **Rust** by
rustdoc. Tag ASCII diagrams and sample output as `text`.

## Benchmarks

Always build benchmarks with `--release`; a debug build measures nothing useful.

```bash
cargo run --example benchmark --release
cargo run --example realistic_benchmark --release
```

Results carry roughly ±2–3% run-to-run noise plus occasional transient P99
spikes. **Compare the mean of at least 3 runs** before claiming a regression or
an improvement, and include your machine's core count with any numbers you
quote — the headline figures in the README are from a 10-core machine.

Performance changes do not need to beat the baseline to be merged, but a PR that
knowingly regresses the hot path should say so and explain the tradeoff.

## Commit Messages

The repository follows [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(engine): add channel routing for workflow dispatch
fix(map): preserve array structure on nested writes
docs: correct MSRV in installation guide
chore(deps): bump quick-xml to 0.41
```

Release notes are generated from commit history, so a clear subject line ends up
in front of users.

## Pull Requests

1. Fork the repository and branch from `main`
2. Make your change, with tests covering it
3. Run the checks above
4. Open a PR describing what changed and why

Add tests next to the code they cover in a `mod tests` block for unit-level
changes. Behaviour that spans the engine goes in `tests/`, which is split by
topic — one binary per file (`engine_execution.rs`, `mapping_semantics.rs`,
`error_handling.rs`, `tracing.rs`, `trace_options.rs`, `observer.rs`,
`public_api.rs`, `rollout.rs`, `templates.rs`, `workflow_loop.rs`). Pick the
file whose topic already matches; add a new one only for a genuinely new area.

Because each file is its own crate, a fixture needed by more than one of them
belongs in `tests/common/mod.rs` (declared `pub`, pulled in with `mod common;`).

Note that CI skips docs-only and Markdown-only changes by design, so a PR
touching only those paths will not show Rust checks.

## Adding a Built-in Function

Built-ins live in `src/engine/functions/`. Implement `AsyncFunctionHandler` with
a typed `Input`:

```rust
#[async_trait]
impl AsyncFunctionHandler for MyFunction {
    type Input = MyInput;

    async fn execute(
        &self,
        ctx: &mut TaskContext<'_>,
        input: &MyInput,
    ) -> Result<TaskOutcome> {
        ctx.set("data.result", OwnedDataValue::from(&json!(42)));
        Ok(TaskOutcome::Success)
    }
}
```

Write through `ctx.set` so audit-trail changes are recorded for you, and return
the `TaskOutcome` variant that matches intent rather than encoding control flow
in a status code. Register the function in `src/engine/functions/mod.rs` and
document it in `docs/src/built-in-functions/` and the README table.

## Releases

Releases are tag-gated and maintainer-run. Pushing to `main` runs CI and deploys
docs; it never publishes. A `v*` tag triggers `.github/workflows/release.yml`,
which validates that the tag matches the root `Cargo.toml` version, then
publishes the crate and both npm packages. Contributors do not need to bump
versions in a PR.

## Security

Do not report vulnerabilities through public issues. See
[SECURITY.md](SECURITY.md) for the private reporting process.

## License

By contributing, you agree that your contributions will be licensed under the
[Apache License 2.0](LICENSE).
