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

Run all of these before handing back any Rust change — CI enforces them and
treats warnings as errors:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy -p dataflow-rs --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
```

The doc check is not optional politeness. Clippy does not lint rustdoc, so
nothing else catches a broken intra-doc link, a link from public docs into a
private item, or an unbackticked generic like `Arc<Logic>` that rustdoc parses
as an HTML tag — all of which ship silently to docs.rs as dead or mangled text.

Lint *policy* lives in `[workspace.lints]` in the root `Cargo.toml`, inherited
by all three members, so a bare `cargo clippy` and rust-analyzer enforce what CI
enforces. Adding a lint there means fixing every existing violation in the same
change — CI's `-D warnings` gives no grace period.

`--all-targets` covers examples, tests and benches; `--all-features` covers the
`wasm-web` feature, which is otherwise silently skipped. Never leave clippy
warnings behind.

The second line is not redundant: it lints the **default** build, which is what
`cargo add dataflow-rs` delivers. `--workspace` cannot do it — `dataflow-wasm`
depends on `dataflow-rs` with `all-operators`, and cargo unifies features across
workspace members, so a `--workspace` invocation always has every operator
family on. Same reasoning applies to `cargo test -p dataflow-rs`.

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
cargo run --example micro_subtree_write_bench --release   # k map writes into one depth-2 subtree
cargo run --example micro_multiworkflow_bench --release   # N chained workflows, one message
cargo run --example async_handler_benchmark --release     # Marginal cost of one custom handler
cargo run --example map_performance_test --release        # 10 chained mappings across all 3 roots
```

The last four are regression guards for optimizations that have already landed
(the arena write-through splice and the shared-arena workflow run), not open
investigations — read them as "this must stay flat", not "this is the plan".

Benchmark numbers carry roughly ±2-3% run-to-run noise and occasional transient
P99 spikes. Compare the mean of 3+ runs before claiming a regression or a win.

**Interleave A/B runs.** Running all the "before" runs and then all the "after"
runs conflates the change with thermal drift: a measured -8.5% on
`realistic_benchmark` collapsed to +0.2% once the same two binaries were
alternated round-robin instead. Build both binaries first, copy them out of
`target/`, discard the first run of each as cold, and alternate.

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
- `retry.rs`: `RetryPolicy` / `retry_with_policy` — native-only (tokio time)
- `rollout.rs`: `Rollout` traffic-split range, `partition` / `validate_set`,
  `RolloutError`
- `secrets.rs`: `Secrets` store and the reserved `secret` operator — values
  expressions read but the engine never records
- `workflow.rs`: `Workflow` definition, lifecycle fields, `LoopConfig`, validation
- `task.rs`: `Task` and `TaskGroup` — both `#[non_exhaustive]`; construct via
  `Task::action`
- `authoring.rs`: `Workflow::validate_authored`, `WorkflowIssue`, `IssueCode` —
  the authoring-time surface a host checks definitions against
- `steps.rs`: The authored step grammar — `flatten` (the parser) and
  `walk_authored_steps` (the public walker), plus the `is_group` /
  `MAX_GROUP_DEPTH` facts both read
- `observer.rs`: `ExecutionObserver` and its event types — task, workflow and
  message lifecycle callbacks
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
- `template.rs`: `Template` — the JSONLogic type behind *every* config
  parameter, its compile-time constant cache, and `TemplateCompiler`
- `path_template.rs`: `PathTemplate<R>` — a parameter naming a *write
  destination*, with `ContextRoot`/`DataRoot` fixing the rooting in the type;
  plus `ParamCtx`, the evaluation handle the sync built-ins resolve against
- `integration.rs`: The three integration configs (`http_call`, `enrich`,
  `publish_kafka`) — config only, no handler ships
- `config.rs`: The `FunctionConfig` dispatch enum, `BUILTIN_FUNCTION_NAMES`,
  `BuiltinKind` and `DispatchableFunction`

### Key Implementation Details

- **Eval context is `{data, metadata, temp_data}` only.** `payload` is a
  separate field on `Message` and is *not* part of the JSONLogic evaluation
  context. A `{"var": "payload.foo"}` expression silently resolves to nothing —
  parse the payload into `data` first. This is an easy and invisible mistake to
  make when writing examples or benchmarks.
- **A workflow's `loop` makes its task list a bounded `for` loop.** Per sweep
  the engine writes the counter to `temp_data`, checks `counter < max`
  (half-open), re-evaluates the workflow condition, runs the task list
  unchanged, then advances by `increment`. `max` is required — it is what makes
  termination structural. Reaching it is *normal completion*, not an error.
  `TaskOutcome::Halt` breaks the whole loop, not one sweep. The engine owns the
  counter, so a body task writing that path is overwritten at the next
  increment. Looping workflows are excluded from the shared-arena fully-sync
  run (`joins_sync_run`) so bump-arena memory is freed between sweeps.
- **A group's condition is evaluated once, on entry — not per member.** An
  element of `tasks` carrying a `tasks` key parses as a `TaskGroup`; the tree is
  flattened into `Workflow::tasks` at parse time and each span is recorded on
  the task that opens it (`Task::group_starts`, outermost first). `GroupGate`
  closes spans by comparing `end` against the cursor, **not** by a per-task
  close count: with `A { B { t } }` and `B` false, nothing inside `A` ever runs,
  so the task that would carry "A closes here" is jumped straight over — yet `A`
  was entered and, if terminal, must still halt. Do not replace that with a
  counter.
- **The two control-flow keys a group cannot honour are resolved differently, on
  purpose.** `halt_on` on a group is **refused at parse time**
  (`steps.rs`, in `walk`) — the schema's one exception to "unknown keys are
  ignored". `continue_on_error` on a group is **linted, not refused**: captured
  by `GroupHeader` as `Option<Value>`, recorded on
  `TaskGroup::continue_on_error`, and reported by `check_workflow` as
  `GROUP_CONTINUE_ON_ERROR`. The difference is age, not principle — `halt_on`
  was new in 3.10.0 with no installed base, while `continue_on_error` is real on
  both `Task` and `Workflow` and may already sit on a host's group nodes, where
  refusing it would fail `Engine::build` for every workflow in that build.
  Honouring it was rejected too: propagating it to member tasks changes the
  error semantics of definitions that load today, silently. The capture is
  `Option<Value>` rather than `Option<bool>` precisely so a non-bool spelling
  stays an ignored unknown key instead of becoming a new parse error, and only a
  literal `true` is reported. Pinned by
  `a_group_continue_on_error_is_recorded_but_never_honoured` (runtime) and
  `a_group_without_continue_on_error_is_silent` (the trigger).
- **`Task::terminal` and `Task::halt_on` are applied *after* the status
  classification in `handle_task_result`, never before.** They only upgrade a
  `Continue` to a `HaltWorkflow`. Folding either into the `let halt = …` that
  starts the if-chain makes halting the first branch, so a terminal task
  returning 500 stops without recording `TASK_STATUS_ERROR` and without
  propagating when `continue_on_error` is false. Pinned by
  `terminal_task_returning_5xx_still_records_and_propagates` and
  `halt_on_failure_with_5xx_and_no_continue_on_error_still_propagates`.
  `TaskPass::halts_at(status)` is the single definition of both — `terminal`
  unconditionally, `halt_on` at `status >= 400`, the same threshold the
  classification itself splits on. Do not open-code a second notion of "failed".
- **A failing `validation` does not stop anything on its own.** It returns
  `Status(400)`, and `continue_on_error` covers only `5xx` and `Err`, so the next
  task runs. That is deliberate — plenty of workflows validate to record errors
  rather than to gate — and `halt_on: "failure"` is how an author opts into
  rejecting. Returning `TaskOutcome::Halt` instead would stamp `HALT_STATUS_CODE`
  (299) over the `400` on both the audit trail and `metadata.progress`, losing the
  status the host answers with; that is why the halt goes through the fold.
  `IssueCode::UnguardedValidation` reports the ungated shape at authoring time and
  is **informational** — never add it to `refuse_authoring_issues`.
- **`metadata.progress` is load-bearing.** The workflow executor writes
  `metadata.progress = {workflow_id, task_id, status_code}` after every task.
  Cross-workflow chaining depends on downstream conditions reading it, so do not
  gate, skip, or make this write conditional.
- **Context writes from `handle_task_result` need an arena refresh.** It writes
  `metadata.progress` always, and — when a host called
  `EngineBuilder::with_error_context_path` — appends failure records at the
  configured path. The sync stretch runs JSONLogic against a snapshot arena
  cache, so each write needs a matching `refresh_for_path` in
  `run_tasks_slice_in_arena`, placed **before** the `?`: an `Err` there does not
  end the arena scope, since `execute_sync_workflow_run` carries the same
  `ArenaContext` into the next workflow when the task has
  `continue_on_error: false` inside a workflow with `continue_on_error: true`.
  Appending to an *existing* array does not change the metadata child count, so
  the divergence rebuild in `executor.rs` will not save you.
- **Error codes come from one classifier.** `service_error_code`
  (`src/engine/error.rs`) is the single mapping — a `Service` error's `kind`
  verbatim, otherwise the variant's own code — and `ErrorInfo::new` routes
  through it so the two cannot drift. Do not reintroduce a flat `TASK_ERROR`
  fallback; that is what made every engine variant indistinguishable before
  3.5.0.
- **Operator families are opt-in, and enabling one is not a no-op.** The
  `datalogic-rs` extension operators ship behind cargo features (`ext-string`,
  `ext-array`, `ext-math`, `ext-control`, `ext-object`, `error-handling`,
  `datetime`, `all-operators`), all off by default. Because the engine always runs in
  templating mode, an operator whose family is off is *not* an error — the
  object echoes back as literal data. So turning a family on converts
  previously-inert values like `{"length": …}` into live operator calls, and
  `datetime` additionally changes `==` and the ordering operators on plain
  date-shaped strings. Both directions are pinned by `#[cfg]`-gated tests in
  `src/engine/compiler.rs`; keep them that way, and never test only
  `--all-features`.
- **Every function parameter is JSONLogic, and the static spelling is free.**
  A JSON literal *is* JSONLogic for itself, so `"path": "data.out"` and
  `"timeout_ms": 5000` mean what they always did. `Template::compile` asks
  `Logic::is_constant()` and, when the expression folded, evaluates it once at
  construction and caches the result; `PathTemplate` additionally precomputes
  the `(dotted, parts)` write pair. Only a parameter that reads the message
  does per-message work. Use `is_constant`, **not** `is_static`: the latter is
  true for rules that fail to fold *because they error* (`{"/": [1, 0]}`), so
  pre-evaluating those would move a runtime error to build time.
- **Resolution borrows; it must not clone.** `Template::resolve_str_in_arena`
  returns `Cow<'_, str>` and `PathTemplate::resolve_in_arena` returns
  `Cow<'_, ResolvedPath>`. Returning owned values instead costs a `String`
  allocation per parse/publish task per message and two atomic `Arc` bumps per
  `map` write — measured at ~9.6% of `realistic_benchmark` throughput. Borrowing
  is why 3.9 is *faster* than 3.8 rather than slower.
- **The template-key escape (`$`) is always on** —
  `compiler::TEMPLATE_KEY_ESCAPE`, applied at the one
  `datalogic_engine_builder()` every engine in this crate goes through, tests
  included. `{"$cat": …}` is the literal object; `{"cat": …}` is still the
  operator. Exactly one prefix is stripped from **every** template key, not only
  colliding ones, so a template emitting genuinely `$`-prefixed keys must double
  them. An escaped key does *not* constant-fold (pinned by
  `an_escaped_key_does_not_fold_to_a_constant`).
- **A single-key object with an unknown key is not inert.** It evaluates its
  argument and emits a structured object: `{"result": {"var": "x"}}` yields
  `{"result": 5}`. That is the ordinary single-key output template, which is why
  there is no "unknown operator key" lint — it could not be distinguished from a
  typo without firing on most correct workflows. Only *multi*-key objects and
  escaped keys go through the template-key checks.
- **Secrets are an operator, not a context root.** `{"secret": "name"}` reads
  an engine-held `Secrets` store (`src/engine/secrets.rs`) through a custom
  datalogic operator registered at every `LogicCompiler::with_operators` site.
  Nothing is composed into `message.context`, so `ArenaContext`,
  `evaluate_condition`, validation's raw `to_arena` and the trace code stay
  untouched — that is the whole guarantee. Do not "simplify" it into a
  `secrets` key on the context. `authoring::check_secrets` is the single
  implementation behind both `build()` refusal and `check_workflow`; `map`
  mappings and `log` expressions may not read a secret at all (blunt on
  purpose — there is no static line between a copy and a derived value).
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

Unit tests live in `mod tests` blocks alongside the code they cover (20 modules).
The integration suite is split by topic across `tests/`, one binary per file:

| File | Covers |
|---|---|
| `engine_execution.rs` | Async handler path, sync stretch, shared-arena runs |
| `mapping_semantics.rs` | `map` write semantics — replace vs. merge, `#` paths |
| `error_handling.rs` | Single error channel, `DataflowError::Service` |
| `tracing.rs` | Caller-owned `process_message_tracing` |
| `trace_options.rs` | `TraceOptions` — timing, diffs, budget, redaction |
| `observer.rs` | `ExecutionObserver` callbacks |
| `public_api.rs` | Built-in classification, typed configs, re-exports, connectors |
| `rollout.rs` | Traffic splits gated on `Message::routing_bucket` |
| `templates.rs` | `Template` config fields on custom handlers |
| `workflow_loop.rs` | `LoopConfig` — bounded per-sweep re-execution |
| `task_groups.rs` | `Task::terminal`, `Task::halt_on` and task groups — the guard-clause shape |
| `task_identity.rs` | `TaskContext` workflow/task ids and `loop_counter` |
| `operator_vocabulary.rs` | `operator_names` — every mirrored name checked live |
| `retry.rs` | `RetryPolicy` backoff, deadline and retryability, under a paused clock |
| `authoring_validation.rs` | `validate_authored` and `check_workflow` — codes, paths, the parse backstop |
| `secrets.rs` | `{"secret": …}` resolution, the engine surface, and the static rules |
| `secrets_isolation.rs` | The never-recorded guarantee, exit by exit — every `TraceOptions` shape, errors, observer, logs, and the access vectors that must not reach the store |
| `template_keys.rs` | The `$` escape end-to-end, and the two template-key issue codes |
| `jsonlogic_params.rs` | Computed parameters — map destinations, parse/publish targets, validation messages |

Each file under `tests/` compiles as its own crate, so fixtures used by more
than one live in `tests/common/mod.rs` and are pulled in with `mod common;`.
That module is `#![allow(dead_code)]` because no single binary uses all of it.

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

`cargo test --workspace --all-features` should report 723 passing.
`cargo test -p dataflow-rs` (default features) should report 627 — the operator
families are `#[cfg]`-gated on both sides, so the counts legitimately differ.

When extending the engine:

1. Implement `AsyncFunctionHandler` with a typed `Input` for custom tasks
2. Register via `Engine::builder().register(..)`
3. Mutate through `TaskContext::set` so the audit trail stays correct
4. Return the `TaskOutcome` variant that matches intent — don't encode control
   flow in a status code
