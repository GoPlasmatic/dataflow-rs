# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Dataflow-rs is a lightweight, rule-driven workflow engine for building data
processing pipelines and nanoservices in Rust. It provides an async-first
execution model with pre-compiled JSONLogic for high performance.

This is a **Cargo workspace with three published artifacts**, all version-locked
to the root `Cargo.toml` version by the release workflow:

| Path | Artifact | Registry |
|---|---|---|
| `.` | `dataflow-rs` | crates.io |
| `wasm/` | `@goplasmatic/dataflow-wasm` | npm |
| `ui/` | `@goplasmatic/dataflow-ui` (React debugger) | npm |

`wasm/` is a workspace member; `ui/` is a separate npm project driven by the
release workflow. Bumping a version means bumping all three.

### Core Architecture

- **Engine**: Central async component that processes messages through workflows sequentially
- **Workflow (Rule)**: Collection of tasks with a JSONLogic condition
- **Task (Action)**: Individual processing unit backed by an `AsyncFunctionHandler`
- **Message**: Carries `payload`, a `context` (`data` / `metadata` / `temp_data`), audit trail, and errors
- **TaskContext**: Per-call handle given to handlers — typed accessors plus audit-recording setters
- **TaskOutcome**: What a handler returns — `Success`, `Status(u16)`, `Skip`, or `Halt`

### Key Design Patterns

- **Sequential Workflow Processing**: Workflows execute sequentially so later workflows can depend on earlier ones
- **Pre-compiled JSONLogic**: All expressions compiled at `Engine::build()`, zero runtime parsing
- **Typed Handler Config**: Each handler declares `type Input`; task config is deserialized once at startup, so malformed config fails at build time rather than on first message
- **Audit Trails**: Change tracking via `TaskContext::set`, gated by `Message::capture_changes`

## Development Commands

### Build and Test

```bash
cargo build
cargo test --workspace --all-features    # what CI runs
cargo test -- --nocapture                # with output
```

### Code Quality

Run both of these before handing back any Rust change — CI enforces them and
treats warnings as errors:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

`--all-targets` covers examples, tests and benches; `--all-features` covers the
`wasm-web` feature, which is otherwise silently skipped. Never leave clippy
warnings behind.

**MSRV is 1.85 and CI enforces it.** Do not use language features stabilized
after 1.85 — most easily tripped is let-chains (`if let ... && let ...`), which
need 1.88. Write nested `if let`s instead. Verify with:

```bash
cargo +1.85 check --workspace --all-targets --all-features --locked
```

### Examples

```bash
cargo run --example hello_world           # Minimal getting-started example
cargo run --example rules_engine          # IFTTT-style rules engine demo
cargo run --example complete_workflow     # Parse -> Transform -> Validate
cargo run --example custom_function       # Custom handler
cargo run --example error_handling        # Error handling patterns
cargo run --example async_migration       # Typed Input + TaskContext + TaskOutcome shape
```

Benchmarks (always `--release`; a debug build measures nothing useful):

```bash
cargo run --example benchmark --release             # Throughput + latency percentiles
cargo run --example realistic_benchmark --release   # ISO 20022 -> SwiftMT-style workload
cargo run --example micro_cond_bench --release      # Condition-eval / trivially-true folding
cargo run --example micro_aggregate_bench --release # Aggregate-heavy (reduce/map)
```

Benchmark numbers carry roughly ±2-3% run-to-run noise and occasional transient
P99 spikes. Compare the mean of 3+ runs before claiming a regression or a win.

### Release Process

Releases are **tag-gated**, not branch-gated. Pushing to `main` runs CI and
deploys the docs; it never publishes. `.github/workflows/release.yml` fires only
on a `v*` tag, validates that the tag matches the root `Cargo.toml` version, then
publishes the crate, the wasm package, and the UI package, and cuts a GitHub
release. Both `wasm/Cargo.toml` and `ui/package.json` must already carry the
matching version.

## Code Structure

### Core Engine (`src/engine/`)

- `mod.rs`: `Engine` and `EngineBuilder` (construction, hot reload, message entry points)
- `compiler.rs`: JSONLogic compilation, priority sorting of workflows
- `executor.rs`: Arena-backed context view; sync built-in dispatch and narrow refresh
- `workflow_executor.rs`: Workflow orchestration, audit trail, progress metadata
- `task_executor.rs`: Per-task execution and outcome handling
- `task_context.rs`: `TaskContext` — accessors and audit-recording setters for handlers
- `task_outcome.rs`: `TaskOutcome` and `HALT_STATUS_CODE`
- `message.rs`: `Message`, `MessageBuilder`, `AuditTrail`, `Change`
- `workflow.rs`: `Workflow` definition, lifecycle fields, validation
- `task.rs`: `Task` structure
- `trace.rs`: `ExecutionTrace` / `ExecutionStep` for step-through debugging
- `error.rs`: `DataflowError`, `ErrorInfo`, retryability classification
- `utils.rs`: Path splitting and nested get/set helpers

### Built-in Functions (`src/engine/functions/`)

- `mod.rs`: `AsyncFunctionHandler` trait, object-safe `Dyn` sibling, registration
- `parse.rs`: `parse_json`, `parse_xml`
- `map.rs`: `map` — JSONLogic-driven assignment to dot-paths
- `validation.rs`: `validation` — rules with custom error messages
- `filter.rs`: `filter` — pipeline control flow (`halt` / `skip`)
- `log.rs`: `log` — structured logging at a configurable level
- `publish.rs`: `publish_json`, `publish_xml`
- `config.rs`: Typed config schemas, including the three integrations
  (`http_call`, `enrich`, `publish_kafka`) that ship as *config only* — you must
  register a matching handler yourself

### Key Implementation Details

- **Eval context is `{data, metadata, temp_data}` only.** `payload` is a
  separate field on `Message` and is *not* part of the JSONLogic evaluation
  context. A `{"var": "payload.foo"}` expression silently resolves to nothing —
  parse the payload into `data` first. This is an easy and invisible mistake to
  make when writing examples or benchmarks.
- **`metadata.progress` is load-bearing.** The workflow executor writes
  `metadata.progress = {workflow_id, task_id, status_code}` after every task.
  Cross-workflow chaining depends on downstream conditions reading it, so do not
  gate, skip, or make this write conditional.
- **Handler contract**: implement `AsyncFunctionHandler` with a
  `type Input: DeserializeOwned`, and

  ```rust
  async fn execute(
      &self,
      ctx: &mut TaskContext<'_>,
      input: &Self::Input,
  ) -> Result<TaskOutcome>;
  ```

  Handlers do **not** return changes. Write through `ctx.set(path, value)`, which
  records the audit-trail `Change` for you.
- **Registration**: `Engine::builder().register("name", handler)` — accepts any
  `AsyncFunctionHandler` and boxes it internally. `Engine::new(workflows, map)`
  is the lower-level escape hatch.
- **Error Handling — two channels**: `process_message` returns `Err` only when
  the engine stopped early. Errors from tasks with `continue_on_error = true` are
  recorded in `message.errors()` without producing an `Err`. Always check both.
- **Hot Reload**: `engine.with_new_workflows(..)` swaps workflows while keeping
  custom function registrations; the old engine stays valid for in-flight messages.
- **Async-First**: all execution paths are async, on the Tokio runtime.

### Testing Patterns

Unit tests live in `mod tests` blocks alongside the code they cover (12 modules);
`tests/workflow_engine_test.rs` holds the integration suite.

Documentation examples are compiled, so they cannot drift from the API:

- `README.md` — via the `ReadmeDoctests` hook at the bottom of `src/lib.rs`.
- `docs/src/**.md` — via the `dataflow-docs-tests` workspace member. It is a
  separate crate because `docs/` is not shipped in the published crate (see
  the root `include` list), so an `include_str!` from `src/lib.rs` would break
  the published package. Its `tests/coverage.rs` fails if a docs page is
  missing from the page list.

When editing docs: fragments get a hidden `# ` preamble (compiled by rustdoc,
hidden from readers by mdBook) rather than an `ignore` tag; unlabelled fences
are treated as Rust, so tag diagrams `text`. See CONTRIBUTING.md for the
conventions.

`cargo test --workspace --all-features` should report 197 passing.

When extending the engine:

1. Implement `AsyncFunctionHandler` with a typed `Input` for custom tasks
2. Register via `Engine::builder().register(..)`
3. Mutate through `TaskContext::set` so the audit trail stays correct
4. Return the `TaskOutcome` variant that matches intent — don't encode control
   flow in a status code
