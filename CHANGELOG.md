# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] — v3.0.0 dev

The `feature/datalogic-v5` branch is the unreleased v3.0.0 development line.
Entries below describe the in-flight changes; a full v3.0.0 release notes
section will be stamped when the version ships to crates.io.

Performance is neutral on the realistic ISO 20022 → SwiftMT-103 workload
(230K msg/s, P50 23 μs). The new dyn-Any dispatch path for custom handlers
adds ~1.2 μs/call of framework overhead — well below typical handler I/O
latency.

### Added

- **`AsyncFunctionHandler::Input`** — typed associated input. Handlers declare
  `type Input: DeserializeOwned` instead of matching on `FunctionConfig::Custom
  { input, .. }`. The engine pre-parses each task's input JSON into the typed
  shape at `Engine::new()` — config-shape errors now fail at startup, not on
  first message.
- **`TaskContext<'a>`** — per-call context handed to every handler. Typed
  accessors (`data()`, `metadata()`, `temp_data()`, `get(path)`),
  audit-trail-aware setters (`set(path, value)` records a `Change`
  automatically when `capture_changes` is on), and `add_error(...)`.
  Replaces the raw `&mut Message + &FunctionConfig + Arc<DatalogicEngine>`
  argument trio.
- **`TaskOutcome` enum** — `Success` / `Status(u16)` / `Skip` / `Halt`.
  Replaces the `(usize, Vec<Change>)` tuple, removes the magic-number contract
  for filter skip / halt signals.
- **`BoxedFunctionHandler`** type alias (= `Box<dyn DynAsyncFunctionHandler +
  Send + Sync>`). Hides the dyn-trait name from user code.
- **`Engine::builder()`** returning `EngineBuilder`. `.register("name", h)`,
  `.register_boxed(...)`, `.with_workflow(w)`, `.with_workflows(iter)`,
  `.build() -> Result<Engine>`.
- **`Message::builder()`** returning `MessageBuilder`. Collapses the historical
  `new` / `with_id` / `from_value` / `without_change_capture` four-way
  constructor split into one fluent shape.
- Read accessors on `Message`: `id()`, `payload()`, `payload_arc()`,
  `audit_trail()`, `errors()`, `capture_changes()`.
- **`dataflow_rs::prelude`** — re-exports the 14 types you need for the 90%
  case (Engine, EngineBuilder, Workflow, Task, Message, MessageBuilder,
  AuditTrail, Change, AsyncFunctionHandler, TaskContext, TaskOutcome, Result,
  DataflowError, ErrorInfo, WorkflowStatus).
- **`#[must_use]`** on `EngineBuilder`, `MessageBuilder`, `ErrorInfoBuilder`
  so drop-on-floor mistakes during the migration are loud.
- **`examples/async_handler_benchmark.rs`** — measures the marginal cost of
  one custom-handler dispatch (`+1.2 μs/msg`, `−9% throughput` on a tight
  6-op pipeline; `+6%` total ops/sec because the extra task does useful work).

### Changed

- **`AsyncFunctionHandler::execute` signature**:
  - **Was**: `async fn execute(&self, &mut Message, &FunctionConfig,
    Arc<DatalogicEngine>) -> Result<(usize, Vec<Change>)>`
  - **Now**: `async fn execute(&self, &mut TaskContext<'_>, &Self::Input)
    -> Result<TaskOutcome>`
  - Removes the `match FunctionConfig::Custom { input, .. } | _ =>
    Err(...)` boilerplate, the manual `Change` construction, and the
    magic-number return tuple.
- **`Engine::new` signature**:
  - **Was**: `pub fn new(Vec<Workflow>, Option<HashMap<String, Box<dyn
    AsyncFunctionHandler + Send + Sync>>>) -> Result<Self>`
  - **Now**: `pub fn new(Vec<Workflow>, HashMap<String,
    BoxedFunctionHandler>) -> Result<Self>`
  - Use `HashMap::new()` for the no-handler case, or — preferred —
    `Engine::builder()`.
- **`Engine::process_message` error contract**: `message.errors()` is now the
  always-on view; `Result::Err` only signals "the engine stopped before
  processing further workflows". The `WORKFLOW_ERROR` wrapper is now pushed
  for **every** workflow failure (not only `continue_on_error: true`); a new
  `TASK_STATUS_ERROR` entry is pushed when a handler returns
  `TaskOutcome::Status(s)` with `s >= 500`. Wire format and audit-trail
  semantics are unchanged.
- **`Message` field encapsulation**: `id`, `payload`, `audit_trail`, `errors`,
  `capture_changes` are now `pub(crate)` with read accessors. `context`
  remains `pub` — it's the legitimate read surface (tests do
  `message.context["data"]["x"]` lookups). Mutate `errors` via
  `message.add_error(e)`; mutate `context` via `TaskContext::set(...)`.
- **`FunctionConfig::Custom`** gained a `compiled_input:
  Option<CompiledCustomInput>` field (skipped by serde; populated by the
  engine at construction time with the typed handler input).

### Removed

- **`Message::with_id`** — use `Message::builder().id(...).build()`.
- **`Message::without_change_capture`** — use
  `Message::builder().capture_changes(false).build()`.
- **`FILTER_STATUS_PASS`, `FILTER_STATUS_SKIP`, `FILTER_STATUS_HALT`**
  constants — `FilterConfig` returns `TaskOutcome::Success` /
  `TaskOutcome::Skip` / `TaskOutcome::Halt` directly. The on-the-wire halt
  status code (299) is preserved as `dataflow_rs::engine::task_outcome::HALT_STATUS_CODE`.

### Performance

- Realistic benchmark (500K msgs × 38 ops, M-series 10 cores, release):
  227.7K → 230.1K msg/s (within run-to-run noise). P50 23 μs unchanged.
- New async-handler benchmark: ~1.2 μs/call framework overhead for the
  dyn-Any dispatch path (typed-input downcast + TaskContext alloc +
  change-buffer drain + audit-entry write).

### Wire compatibility

- `Message`, `AuditTrail`, `Change`, `ErrorInfo`, `Workflow`, `Task`,
  `FunctionConfig` JSON shapes are **unchanged** within the v3.0.0 dev
  line. The `FunctionConfig::Custom.compiled_input` field is
  `#[serde(skip)]`; it round-trips through JSON as `None` and is
  re-populated when the workflow is loaded into the engine.

### Earlier v3.0.0 dev work (commit `c375ec6`)

Datalogic v5 integration, sync-stretch arena reuse, hot-path perf work,
and fail-loud `Engine::new` (compile every JSONLogic at startup, return
`Err` on any failure). See commits `c8775fd..c375ec6` for the full set.
These changes will be folded into the v3.0.0 release notes when the
version ships.
