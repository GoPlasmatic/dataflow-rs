# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [3.1.0] — 2026-07-30

Three defects from a boundary audit of the crate's deepest known consumer. Minor
rather than patch because new public items ship and two existing behaviours
change.

### Added

- **integration:** `HttpMethod` is re-exported from the crate root (previously
  reachable only as
  `dataflow_rs::engine::functions::integration::HttpMethod`) and gains
  `ALL`, `as_str`, `is_idempotent`, `Display`, and the `Copy`/`PartialEq`/`Eq`/
  `Hash` derives. Because this crate does not implement `http_call`, every
  consumer converts this enum into their own client's method type and wrote the
  same five-arm match to do it; `as_str()` is the intended bridge, and the crate
  still takes no HTTP-client dependency. `ALL` is scoped to what an `http_call`
  task may name — not a general HTTP method list. (#24)
- **integration:** `HttpCallConfig::response_path` accepts `output` as an alias,
  so a service layer can present one destination-field name across its whole
  function catalogue. Supplying both keys is a `duplicate field` error, not a
  precedence rule. The alias is specific to `http_call`; `enrich`'s `merge_path`
  and publish's `target` do not take it. (#24)

- **engine:** `Engine::process_message_tracing` and
  `Engine::process_message_for_channel_tracing`, which record into a
  caller-owned `ExecutionTrace` instead of returning one. Steps are appended, so
  a trace can accumulate across a chain of calls. See *Fixed* for why this
  exists. (#25)
- **functions:** `BuiltinKind` (`SelfContained` / `RequiresHandler`),
  `builtin_function_kind` and `is_builtin_function`, re-exported from the crate
  root, so a service layer that gates workflow authoring on a closed function
  set can classify a name programmatically. `RequiresHandler` covers
  `http_call`, `enrich` and `publish_kafka` — the three that ship as typed
  config only and need a registered handler. Deliberately not
  `#[non_exhaustive]`: callers match on this to accept or reject workflow
  definitions, so a future third kind should break those matches at compile time
  rather than fall silently into a `_` arm. (#22)
- **functions:** `BUILTIN_FUNCTION_NAMES` is now `pub` (was `pub(crate)`), with
  a documented stability contract — names are added in a minor release and
  removed only in a major one, and ordering is not meaningful. Previously the
  only public surface for this set was the free-form text of
  `DataflowError::FunctionNotFound`, leaving consumers to scrape an untested
  `Display` format. (#22)

### Fixed

- **engine:** `process_message_with_trace` and
  `process_message_for_channel_with_trace` built the trace as a function-local
  and moved it into the `Ok` arm, so a hard failure discarded every step that
  had already run — a debugging API dropped its output on the one input class it
  exists to explain. The steps were already in memory: every layer beneath the
  two entry points threaded the trace by reference and appended incrementally,
  making this an API-shape inversion at exactly two functions. Both keep their
  exact signatures and behaviour and are now thin wrappers over the new
  caller-owned methods. Note the failing task's own step is still not recorded
  (the engine propagates before appending it), so a retained trace ends at the
  last known-good step. (#25)
- **functions:** `TaskExecutor::has_function` returned `true` for `http_call`,
  `enrich` and `publish_kafka` without consulting the handler registry, so the
  API shaped like "can this engine run this task?" answered it wrongly and
  nothing answered it at all. It now routes through `builtin_function_kind` and
  returns `false` for those three unless a handler is registered. `Engine::new`
  stays permissive on purpose — a host screening stored definitions one row at a
  time must not be stopped from booting by a single unusable row — so the fix is
  the introspection needed to detect and quarantine the gap instead. (#22)
- **functions:** routing `has_function` through the classifier removes the third
  in-crate copy of the built-in name list; the const, the deserializer dispatch
  and `has_function`'s match arm had all spelled out the same twelve names, with
  only the first two tied together by a test.

- **integration:** `HttpCallConfig`, `EnrichConfig` and `PublishKafkaConfig` now
  reject unknown keys (`deny_unknown_fields`). A misspelled field previously
  parsed cleanly and was discarded, so an `http_call` task would make its request
  and silently throw the response away — no error at
  `Engine::builder().build()`, none at dispatch. The failure now arrives when the
  workflow definition is parsed, naming the offending field. (#24)

### Changed

- **engine:** `DataflowError::FunctionNotFound`'s message is now documented as
  free-form and explicitly unpinned, with the new classifier as the supported
  programmatic route. No test asserts on its wording, and none should — pinning
  it would cement the scraping workaround #22 exists to remove.

### Compatibility

No public type, enum, struct or function signature changes shape, so all existing
downstream code keeps compiling. Three behaviours change at runtime:

`has_function`'s answer changes for `http_call` / `enrich` / `publish_kafka` when
no handler is registered. In-repo the only caller was its own test, and `Engine`
exposes no accessor for its executor (`workflow_executor` is private), so the
method is reachable only by a caller that constructed a `TaskExecutor` itself.

Two changes affect how existing workflow **input JSON** is interpreted, neither
visible as a compile error downstream:

| `http_call` input | Before | Now |
|---|---|---|
| `{"response_path": "a"}` | `Some("a")` | `Some("a")` — unchanged |
| `{"output": "b"}` | `None`, silently | `Some("b")` |
| `{"response_path": "a", "output": "b"}` | `Some("a")`, `output` ignored | `Err`: duplicate field, either key order |
| `{"outputs": "b"}` (typo) | `None`, silently | `Err`: unknown field `outputs` |

The last row is the defect being closed and also the upgrade risk: a stored
workflow document carrying a stray or misspelled key inside an `http_call`,
`enrich` or `publish_kafka` input loaded before and now fails. The failure is at
`Workflow::from_json`, per definition, so a host screening stored rows one at a
time sees that row fail its own parse rather than losing the whole set — but any
such document must be corrected before upgrading.

Workspace test count 184 → 216.

## [3.0.4] — 2026-07-26

Dependency refresh, documentation corrections, and repository hardening. One
behaviour-preserving `src/` change (see *Fixed*); no public API changes.

### Added

- **docs:** `CONTRIBUTING.md` and `SECURITY.md`. The security policy documents
  private vulnerability reporting, the supported-version window, and the
  engine's trust model (workflow definitions are trusted configuration; message
  payloads are untrusted data).
- **ci:** MSRV job pinned to 1.85. `rust-version` was previously a promise
  nothing verified — see *Fixed*.
- **ci:** UI job running `npm ci` and `build:lib`, with an assertion that the
  emitted `dist/lib.d.ts` is non-empty and contains exports. `ui/` is published
  to npm on every release but had no CI coverage at all; dataflow-ui v2.1.3
  shipped an empty declaration file this would have caught.
- **ci:** `cargo-deny` job (advisories, licenses, sources, bans) plus a weekly
  scheduled run, so a new RUSTSEC advisory against unchanged code surfaces
  without someone noticing by hand. Policy lives in `deny.toml`.
- **ci:** `.github/dependabot.yml` covering cargo, npm (`ui/`), and
  github-actions, with minor/patch updates grouped into single PRs.
- **docs:** README Rust snippets are now compiled as doctests through a
  `ReadmeDoctests` hook in `src/lib.rs`.
- **docs:** the mdBook guide's Rust examples are compiled too, via a new
  `dataflow-docs-tests` workspace member (`publish = false`). `mdbook test`
  cannot do this — it only passes `-L` to rustdoc, while an edition-2018+
  `use dataflow_rs::…` needs `--extern`, which mdBook has no flag for; routing
  the pages through `#[doc = include_str!(…)]` lets Cargo wire it up. 57 of the
  book's 84 Rust blocks now compile; the remaining 27 are API signature
  listings and are tagged `ignore` explicitly rather than silently unverified.
  Fragments that assume an `engine` or `message` binding got hidden `#`
  preambles — compiled by rustdoc, hidden from readers by mdBook — so nothing
  readers see changed. Workspace test count 123 → 183.
- **ui:** working eslint setup — `eslint.config.js` (flat config, eslint 10 +
  typescript-eslint + react-hooks) and the toolchain as devDependencies. The
  `lint` script had been calling a binary that was never installed and had no
  config file, so it failed for anyone who ran it; it is now green and enforced
  in CI. Five pre-existing `react-hooks/set-state-in-effect` violations are
  marked with per-site `eslint-disable-next-line` plus a `TODO(react-hooks)`
  explaining each, so the rule still fails CI for newly-written code.

### Fixed

- **README:** the Getting Started example did not work. It built a message with
  `Message::from_value`, which populates `payload`, but its rule condition and
  mappings read `data.order.total` — and `payload` is not part of the JSONLogic
  evaluation context, which is `{data, metadata, temp_data}`. The example
  printed `null` rather than the documented `150` / `1350`. It now shows the
  two-rule chain it always described: an intake rule that `parse_json`s the
  payload into `data.order`, then the conditioned discount rule. The corrected
  version runs as a doctest and asserts both values, so it cannot silently break
  again. Also documents that a rule's condition is evaluated *before* its own
  tasks run, which is why the parse cannot live inside the conditioned rule.
- **MSRV:** `rust-version = "1.85"` was not true — `write_progress_metadata` in
  `src/engine/workflow_executor.rs` used let-chains, stable only since Rust
  1.88, so the crate had never built on its advertised minimum. Rewritten as
  nested `if let` (behaviour identical, no API change) and 1.85 restored as a
  working, CI-enforced minimum.
- **docs:** `getting-started/installation.md` told new users to depend on
  `dataflow-rs = "2.1"` while the surrounding snippets used 3.x builder API.
- **docs:** the Quick Start example did not compile. It declared
  `async fn main() -> Result<(), Box<dyn std::error::Error>>` while importing
  `dataflow_rs::prelude::*`, whose `Result<T>` alias takes a single type
  parameter and shadows `std::result::Result`. Now `Result<()>`, with a note
  about the shadowing. Compiled *and executed* as a doctest.
- **docs:** `core-concepts/engine.md` called `workflow.name.as_deref()`;
  `Workflow::name` is a `String`, not an `Option<String>`.
- **docs:** `built-in-functions/log.md` had a shell command
  (`RUST_LOG=… cargo run`) inside a ```` ```rust ```` block; split into `bash`
  and `rust` blocks.
- **docs:** five ASCII diagrams and one sample-output block used unlabelled
  fences, which rustdoc treats as Rust. Tagged `text`.
- **docs:** `CLAUDE.md` had drifted from the code in eight places, including the
  pre-3.0 handler signature `Result<(usize, Vec<Change>)>`, a
  `register_task_function()` entry point that no longer exists, a
  `with_preserve_structure()` call absent from the tree, a retry/backoff
  mechanism that was never implemented, and a release process described as
  branch-triggered when it is tag-gated.

### Changed

- **deps:** refreshed the lockfile to latest compatible versions — serde
  `1.0.228 → 1.0.229`, serde_json `1.0.150 → 1.0.151`, tokio
  `1.52.3 → 1.53.1`, uuid `1.23.3 → 1.24.0`, thiserror `2.0.18 → 2.0.19`,
  log `0.4.32 → 0.4.33`, async-trait `0.1.89 → 0.1.91`, datalogic-rs
  `5.1.0 → 5.1.1`, futures `0.3.32 → 0.3.33`, env_logger
  `0.11.10 → 0.11.11`, wasm-bindgen `0.2.125 → 0.2.126`. Total lockfile
  packages 165 → 145: wasm-bindgen 0.2.126 drops its
  `wit-bindgen`/`wasm-encoder`/`wasmparser` tooling chain.
- **ci:** clippy and tests now run with `--workspace --all-features`, so
  `dataflow-wasm` and the `wasm-web` feature are covered instead of silently
  skipped. Added a `wasm` job that lints against `wasm32-unknown-unknown`
  and runs the wasm test suite under Node.
- `Cargo.lock` is now tracked, making CI runs reproducible.
- **ci:** the release workflow's validation gate now mirrors CI
  (`--workspace --all-features`) instead of running a narrower
  `cargo clippy --all-targets` / `cargo test`. A wasm-only regression could
  previously fail PR CI and still pass the release gate. `cargo publish`,
  `clippy`, and `test` all run `--locked` so the published build resolves the
  tracked lockfile.

### Removed

- **deps:** dropped the direct `getrandom` dependency from both crates and
  from the `wasm-web` feature. It was inert: uuid's own `getrandom` is
  target-gated *off* for `wasm32-unknown-unknown` (it routes through
  wasm-bindgen/js-sys via `uuid/js`), and the pin was `0.3` while uuid uses
  `0.4` — so `getrandom/wasm_js` could never affect uuid, and it put two
  `getrandom` majors in the tree.
- The published crate no longer ships `/ui`, `/docs`, or `CLAUDE.md`. These
  are the npm-published React debugger, the GitHub Pages book, and
  repo-local contributor guidance — none are needed to build the crate.
  Package contents drop 175 → 50 files (270 KiB → 130 KiB compressed).

### Fixed

- `wasm/tests/web.rs` did not compile: an `unwrap_err()` required
  `WasmEngine: Debug`, and three tests referenced a `parse` function that
  had been split into `parse_json`/`parse_xml` with required
  `source`/`target` config. The suite is also no longer browser-gated, so it
  runs under `wasm-pack test --node` without a driver. 0 → 8 passing tests.

## [3.0.3] — 2026-07-18

Security dependency update.

### Changed

- **deps:** quick-xml `0.37 → 0.41` to clear RUSTSEC-2026-0194 and
  RUSTSEC-2026-0195 (DoS in attribute checking and `NsReader`).

## [3.0.2] — 2026-07-17

Performance-focused release: adopts datalogic-rs 5.1 and lands a hot-path
optimization pass. On the 10-core reference machine the `benchmark` example
runs at ~640K msg/s (P50 6 μs, P99 51 μs); the realistic ISO 20022 →
SwiftMT-103 workload improved ~242K → ~259K msg/s (+7%) with materially
tighter tails (P99.9 ~215–270 μs → ~135–155 μs).

### Added

- `examples/micro_aggregate_bench.rs` — end-to-end benchmark for
  aggregate-heavy (`reduce`/`map`) mappings; quantifies the datalogic 5.1
  CSE + fusion wins (1.68× on a checkout-style workload).
- `examples/micro_subtree_write_bench.rs` — scaling benchmark for many map
  mappings targeting the same subtree (k = 5/25/100 writes).

### Changed

- **deps:** datalogic-rs `5.0 → 5.1` (common-subexpression elimination for
  repeated pure aggregates, `reduce(map(...))` fusion) and datavalue-rs
  `0.2.2 → 0.2.3`. API-compatible — no dataflow code changes required;
  aggregate-heavy mappings improve substantially, scalar pipelines are flat.
- **deps:** `uuid` enables `fast-rng` on non-wasm targets — default v7
  message ids draw from a thread-local PRNG seeded once from the OS instead
  of paying one `getrandom` syscall per message (+3% throughput, tighter
  P99.9). Appropriate for v7 ids, which need uniqueness, not secrecy. The
  wasm build keeps the lean `v7`+`std` feature set.
- **Source compatibility (semver-minor):** `ParseConfig` and `PublishConfig`
  gained engine-internal precomputed fields (`#[doc(hidden)]`,
  `#[serde(skip)]`, not part of the stable API). Struct-literal construction
  must now spread a default — e.g.
  `ParseConfig { source, target, ..Default::default() }`. `ParseConfig`
  derives `Default`; `PublishConfig` has a manual `Default` with
  `root_element = "root"` (matching the serde default). JSON wire shape and
  `from_json` construction are unchanged.

### Performance

- Map hot loop: arena-side write-through — consecutive mappings writing into
  the same subtree no longer re-clone it per mapping; k same-subtree writes
  now do O(k) total work instead of O(k²) (k=100 microbench: −27% ns/msg,
  and the win grows with written-subtree size).
- Workflow-condition evaluation folded into the first sync task stretch for
  mixed sync+async workflows — one context walk instead of two
  (`async_handler_benchmark`: +5.7% throughput; marginal cost of one custom
  handler dispatch 0.56 → 0.09 μs/msg).
- `publish_json` / `publish_xml` serialize from a borrowed source instead of
  deep-cloning the source subtree first — hundreds of avoided allocations
  per publish on ISO 20022-shaped payloads.
- Per-task `metadata.progress` write updates the existing slot in place and
  refreshes only that arena child instead of re-arenaing the whole
  `metadata` tree.
- `log` tasks short-circuit before evaluating any JSONLogic when their level
  is filtered out for the `dataflow::log` target — filtered log tasks are
  effectively free in production.
- `parse_*` / `publish_*` target paths precomputed at engine build time (no
  per-execution `format!` / path split / `Arc<str>` alloc); default v7
  message ids stored in an inline buffer instead of a heap `String`.

### Fixed

- `benchmark` and `micro_cond_bench` referenced `{"var": "payload.input.*"}`,
  which is outside the evaluation context (`data` / `metadata` /
  `temp_data`) — every mapping silently evaluated to null and the published
  numbers measured no-op work. Both now use the canonical `parse_json` →
  `data.input.*` idiom, and every bench asserts at startup that its workload
  actually computes. README performance numbers restated from the fixed
  workload.

## [3.0.1] — 2026-06-13

### Performance

- One bump arena shared across consecutive fully-sync workflows instead of
  one arena rebuild per workflow.
- Trivially-true workflow conditions fold to `None` at compile time,
  skipping per-message condition evaluation entirely.

### Changed

- deps: datavalue-rs `0.2.2` and bumpalo `3.20` minimums aligned with the
  lockfile.

### Added

- `examples/micro_cond_bench.rs` — single-threaded `process_message`
  microbenchmark.
- `examples/micro_multiworkflow_bench.rs` — quantifies per-workflow arena
  rebuild cost.

## [3.0.0] — 2026-05-15

Major redesign of the custom-function API surface on the datalogic v5 core.

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

### Earlier v3.0.0 work (commit `c375ec6`)

Datalogic v5 integration, sync-stretch arena reuse, hot-path perf work,
and fail-loud `Engine::new` (compile every JSONLogic at startup, return
`Err` on any failure). See commits `c8775fd..c375ec6` for the full set.
