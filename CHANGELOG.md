# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **engine:** `Engine::can_dispatch` / `EngineBuilder::can_dispatch` — whether a
  task named `name` will actually run. `true` for a self-contained built-in and
  for any name with a registered handler, including an alias such as
  `validation`; `false` guarantees the opposite, that a task naming it fails
  with `FunctionNotFound` on the first message that reaches it. This closes the
  half of the question `builtin_function_kind` could not answer: it reports that
  `enrich` *needs* a handler, but not whether one is registered. A workflow
  using a config-only integration with nothing behind it still builds cleanly —
  that permissiveness is deliberate — so this is the check that catches it
  before activation rather than on the first request.
- **engine:** `Engine::dispatchable_functions` /
  `EngineBuilder::dispatchable_functions` — the full vocabulary an engine will
  dispatch, for completion tooling, admin catalogues and did-you-mean
  suggestions. Yields `DispatchableFunction { name, kind, aliases }`. Aliases
  are grouped, so `validate` appears once carrying `["validation"]` rather than
  twice; `kind` is `Option<BuiltinKind>`, where `None` means a registered custom
  handler — the same convention `builtin_function_kind` already uses, chosen so
  `BuiltinKind` need not gain a third variant and break every downstream
  `match`. Ordering is not meaningful, matching `BUILTIN_FUNCTION_NAMES`.

- **engine:** `walk_authored_steps` — a total walker over a workflow's authored
  `tasks` JSON, yielding every node with the coordinate the author typed
  (`tasks[1].tasks[0]`), its `StepKind` (`Leaf` / `Group` / `TooDeep`) and its
  nesting depth. Where parsing stops at the first bad element, this reports
  malformed elements, empty groups and over-deep nesting as nodes, so a
  validator collects every problem in one pass. Filtering to `Leaf` reproduces
  the engine's flattened `Workflow::tasks` exactly, pinned by an equivalence
  test.
- **engine:** `is_group` and `MAX_GROUP_DEPTH` are now public — the two facts a
  host validating authored JSON would otherwise mirror. A host that reads them
  follows a future change to either automatically.

### Changed

- **engine:** the step grammar moved to `src/engine/steps.rs`, which now holds
  both the parser and the public walker. `task.rs` is back to being about
  `Task`. No API change; `Workflow` parsing is unaffected.
- **ui:** `isTaskGroup` now tests for the presence of a `tasks` key rather than
  `Array.isArray(tasks)`, matching the engine's parser. The two had diverged: on
  `{"id": "x", "tasks": "oops"}` the engine reported a malformed *group* while
  the UI read a *task*. A new `groupMembers()` accessor supplies a group's
  members — empty when `tasks` is malformed — so renderers descend only into a
  real array, mirroring the walker.
- **ui:** `TaskGroup`, `Step`, `isTaskGroup`, `flattenSteps` and `groupMembers`
  are now exported from the package root. The 3.6.0 changelog described
  `flattenSteps` as consumer-facing, but it was never re-exported.
- **engine:** `TaskExecutor::has_function` now delegates to the same predicate
  `can_dispatch` exposes. Internal, but it is the point of the change: the
  question the engine answers when dispatching and the question a host asks when
  screening are one definition rather than two that happen to agree.
- **docs:** `built-in-functions/integrations.md` — the "detecting a missing
  handler" section previously stopped at classifying the name and advised
  requiring a registration for *every* `RequiresHandler` name, because the
  registry was unreachable. It now shows the real check.

### Notes

- Registering a handler under a self-contained built-in name (`map`) is inert:
  the deserializer routes it to the crate's own implementation, which never
  consults the registry. Such a name is reported once, as a built-in. Previously
  undocumented; unchanged in behaviour.

## [3.6.0] — 2026-08-24

Guard clauses. A workflow's `tasks` array now holds *steps* — a task or a group
of them — and any step can end the workflow, so a condition no longer has to
re-encode the negation of every branch above it.

### Added

- **task:** `terminal` — a task that, having run, ends the workflow. It is a
  statement about *position*, not outcome: a false `condition` or a
  `TaskOutcome::Skip` does not halt, but a task that *failed* under
  `continue_on_error: true` does — the author said "nothing after this runs".
  Halting scopes to the workflow, exactly like `TaskOutcome::Halt`; inside a
  workflow carrying a `loop` it breaks the whole loop, not one sweep. The audit
  entry keeps the task's **own** status (`200`, `404`, …) rather than
  `HALT_STATUS_CODE`, so a `map` that wrote a 404 response body does not report
  "a filter halted here".
- **workflow:** task groups. An element of `tasks` carrying a `tasks` key is a
  `TaskGroup` — `{id, condition, terminal, tasks}` — stating one condition for a
  contiguous run of tasks instead of repeating it on each. The condition is
  evaluated **once, on entry**: a false result skips the whole span without
  evaluating the members' own conditions, so a task inside the block that
  mutates what the condition reads cannot switch off its own siblings. Groups
  nest, up to 8 levels, and a group with `terminal: true` is the full guard
  clause — `if (…) { …; return; }`.
- **docs:** `advanced/control-flow.md`, covering both constructs and when each
  beats a `filter` + `on_reject: "halt"` pair.

### Changed

- **workflow:** `Workflow::validate` now rejects a group id that duplicates
  another group's or a task's. Groups share the task id namespace — both name a
  step, and both surface in traces.
- **ui:** `Workflow.tasks` is typed `Step[]` (`Task | TaskGroup`). A new
  `flattenSteps()` helper returns the leaf tasks for consumers that count or
  look up tasks; the flow diagram renders a group as one condition node gating
  the whole span, and marks terminal tasks.

### Compatibility

- **Wire format is additive.** Every existing workflow JSON parses unchanged and
  behaves identically: `terminal` defaults to `false`, and a workflow with no
  group objects records no spans.
- **`Task` gains three public fields** — `terminal`, plus the `#[doc(hidden)]`
  engine-internal `group_starts`. Struct-literal construction of `Task` breaks;
  field access does not. Same shape as 3.3.0 adding `Workflow::loop`.
- **Older engines.** A group sent to a pre-3.6.0 engine **fails loudly** — the
  group object has no `function`, so it is rejected at parse. A bare
  `terminal: true` is silently ignored there and every later task runs; gate on
  the engine version if you deploy definitions to engines you do not control.

## [3.5.0] — 2026-08-20

Failed-task error codes are now reachable from workflow logic, and the codes
themselves are worth branching on.

### Added

- **engine:** `EngineBuilder::with_error_context_path` — mirror per-task failure
  codes into a host-chosen path inside the message context, so a downstream
  `condition` or `map` can branch on *why* a task failed. `message.errors()` is
  `pub(crate)` and the JSONLogic evaluation context is exactly
  `{data, metadata, temp_data}`, so this was previously unreachable from a
  workflow; `metadata.progress` carries no reason and is overwritten by every
  task. One `{workflow_id, task_id, code, status}` record is appended per error a
  task contributes, covering handler `Err`s, 5xx outcomes, each failing
  `validation` rule, and `TaskContext::add_error` alike — the sync built-ins
  never reach the handler registry, so a host wrapper cannot see them. The error
  `message` and the operator-only `detail` are excluded, since `context` is
  serialized back to callers. Off unless called.
- **engine:** `EngineBuilder::with_error_context_limit` — cap the records
  retained (default 32, newest kept), so the cost stays independent of a looping
  workflow's iteration count.

### Changed

- **errors:** every `DataflowError` variant now contributes its own
  `ErrorInfo.code` on the live path — `TIMEOUT_ERROR`, `IO_ERROR`, `HTTP_ERROR`,
  `VALIDATION_ERROR`, and the rest. Previously only `Service` was lifted and
  every other variant collapsed to `TASK_ERROR`, so a timeout, a dropped
  connection and a rejected request were indistinguishable; the variant→code
  table existed in `ErrorInfo::new` but had no non-test caller. `Service` still
  passes its `kind` through verbatim and `DataflowError::Task` still maps to
  `TASK_ERROR`, so code matching on either is unaffected. Code matching
  `TASK_ERROR` to mean "the handler returned `Err`" should match the specific
  codes instead.

### Fixed

- **engine:** the sync stretch skipped its `metadata.progress` arena refresh when
  a task returned `Err`, because the refresh sat after the `?`. A task with
  `continue_on_error: false` inside a workflow with `continue_on_error: true`
  leaves `execute_sync_workflow_run` carrying the same `ArenaContext` into the
  next workflow, whose condition then evaluated against a stale
  `metadata.progress`. The refresh now runs before the `?`. Pinned by
  `a_task_error_still_advances_the_shared_arena_for_the_next_workflow` in
  `tests/engine_execution.rs`, which fails against 3.4.0.

### Compatibility

No function or method signature changes shape, no type changes shape, and no
struct gains or loses a field. Everything added is additive: two new
`EngineBuilder` methods, both opt-in, both inert unless called.

One **runtime behaviour change**, and it is the one to read before upgrading:
`ErrorInfo.code` now reports the failing `DataflowError`'s own variant code
rather than a blanket `TASK_ERROR`. Concretely, a handler returning
`DataflowError::Timeout` used to land `TASK_ERROR` in `message.errors()` and now
lands `TIMEOUT_ERROR`; the same applies to `Validation`, `Http`, `Io`,
`Deserialization`, `Unknown`, `FunctionNotFound`, `FunctionExecution` and
`LogicEvaluation`. Nothing fails to compile — code that switches on `code` keeps
switching, it just sees a more specific string.

Unaffected, deliberately:

- `DataflowError::Task` still maps to `TASK_ERROR`.
- `DataflowError::Service` still contributes its `kind` verbatim, including
  through a `FunctionExecution` wrapper.
- `TASK_STATUS_ERROR` (5xx outcomes), `WORKFLOW_ERROR` (the workflow wrapper) and
  the `validation` built-in's three codes are all unchanged.
- The wire JSON shape of `ErrorInfo` is unchanged in both directions.
- Nothing in `wasm/` or `ui/` reads `code`.

**What to check:** code that treats `TASK_ERROR` as meaning "the handler returned
`Err`". That reading was never guaranteed — the docs already described the code
list as not closed and told callers to switch with a default arm — but it did
happen to hold for engine-owned variants before this release. Match the specific
codes instead, or return `DataflowError::Task` where the generic code is what you
want. Note that a default `^3` Cargo requirement upgrades into this release
automatically, so the version bump alone will not gate it.

## [3.4.0] — 2026-08-19

Custom JSONLogic operators can now be registered on the engine, and the
`datalogic-rs` upgrade to 5.2 brings a new `ext-object` operator family.

### Added

- **engine:** `EngineBuilder::with_datalogic_operator` — register a custom
  JSONLogic operator (`datalogic_rs::CustomOperator`) under a host-chosen
  name (a built-in operator name always wins over a custom registration, so
  pick names no built-in uses). Registrations are retained on the engine, so an
  `Engine::with_new_workflows` hot reload re-registers them instead of
  silently dropping them. Under templating an *unregistered* name still
  echoes back as literal data, so registering a name converts previously-inert
  values into live operator calls — the same caveat the operator-family cargo
  features carry.
- **integrations:** `HttpCallConfig` gains `body_format` and `response_format`
  — uninterpreted passthrough fields whose value table belongs to the service
  layer, so new encodings need no release of this crate. Absent fields
  deserialize as `None`; misspelled field *names* still fail at parse time via
  `deny_unknown_fields`.
- **jsonlogic:** `ext-object` cargo feature, forwarding `datalogic-rs`'s new
  object take-apart family: `keys`, `values`, and `entries` (`entries` yields
  `[{key, value}]` rows the array vocabulary can iterate). Off by default like
  every family, and included in `all-operators` — which makes it live in the
  npm `@goplasmatic/dataflow-wasm` build.
- **jsonlogic:** `ext-array` now also unlocks `group_by` and `distinct`
  (datalogic-rs 5.2). Existing rules carrying `{"group_by": …}` or
  `{"distinct": …}` objects as literal data start evaluating them — the same
  non-additivity caveat that applies to enabling a family.
- **jsonlogic:** with `datetime` enabled, `format_date` and `parse_date` accept
  an optional trailing IANA timezone argument (backed by `chrono-tz`), and the
  format-token table gains month/weekday names (`MMM`, `MMMM`, `EEE`, `EEEE`).

### Changed

- **deps:** `datalogic-rs` 5.1 → 5.2.

## [3.3.0] — 2026-08-11

A workflow's task list can now run as a bounded loop, so a set of tasks can
process one array element per sweep instead of needing one workflow per item.

This release also fixes the npm `@goplasmatic/dataflow-wasm` artifact, which
**has never worked in a browser** — see *Fixed*.

### Added

- **workflow:** `loop` field (`LoopConfig`) turning a workflow's task list into
  a bounded `for` loop. Per sweep the engine writes the counter to `temp_data`,
  checks `counter < max` (half-open), re-evaluates the workflow condition, runs
  the task list unchanged, then advances by `increment`. Fields: `max`
  (**required**), `counter`, `init` (default `0`), `increment` (default `1`).
  `increment < 1`, `max <= init`, and a malformed `counter` path are rejected at
  `Engine::build()` rather than at runtime.
- **audit/trace:** `loop_counter` on `AuditTrail` and `ExecutionStep`, carrying
  the counter value for the sweep that produced the entry. Omitted entirely for
  workflows without a `loop`, and recorded even when the loop leaves its counter
  unnamed, so a trace can be grouped by iteration.
- **docs:** `docs/src/advanced/loops.md`, covering per-item processing, fixed
  repetition, repeat-until-false, breaking out mid-body, and audit volume.
- **ui:** looping workflows are now visually distinct. A `⟳ i: 0..n` badge on
  the workflow card, tree node and group-diagram node, and in the flow diagram a
  loop-guard diamond, an `i += 1` tail, and an animated back-edge. Adds the
  `LoopConfig` type and the `loopBadgeLabel` / `loopGuardLabel` / `loopStepLabel`
  / `loopDescription` helpers to the public exports, plus a Per-Item Loop sample.
- **ui:** `npm run wasm:local`, which builds this checkout's engine and overlays
  it onto the installed package — the dependency stays pinned to a published
  version so `npm ci` can resolve it. Without this the debugger silently runs
  against the last release.
- **ci:** `wasm/scripts/verify-wasm.mjs`, run before publishing and on every PR.
- **wasm:** `engine_version()`, returning the engine version compiled into the
  module.
- **ui:** an engine version handshake. `WasmEngineAdapter` throws when the
  loaded wasm is **older** than the `dataflow-ui` build using it, because
  `Workflow` does not set `deny_unknown_fields` — an older engine ignores
  fields it predates instead of rejecting them, so the debugger would otherwise
  run and quietly disagree with the workflows on screen. A newer engine passes
  silently; the dependency is a caret range, so npm may legitimately resolve
  one. Exported as `assertEngineVersion` for hosts that build their own engine.
  An engine too old to export `engine_version` at all is read off a namespace
  import rather than a named one, so it produces this same friendly
  version-mismatch error instead of an opaque module-link failure.
- **ci:** the release `validate` job now asserts that root `Cargo.toml`,
  `wasm/Cargo.toml` (and its `dataflow-rs` dependency), `ui/package.json`, and
  the `dataflow-rs = "X.Y"` snippets in `README.md` and `docs/src/` all agree.
  Only the root version was previously checked, and the npm versions are
  stamped from it at publish time, so in-repo drift was invisible.
- **ci:** CI and the release now resolve `@goplasmatic/dataflow-wasm`
  differently, because they answer different questions. The `ui` job builds
  the engine from the commit under test via `wasm:local`, so a UI change that
  depends on a wasm export added in the same commit can typecheck without a
  published release to resolve. `publish-ui` installs the version
  `publish-wasm` just published, so the release validates the exact artifact
  consumers will install rather than a local build that never reaches them.

### Fixed

- **wasm (critical):** every published `@goplasmatic/dataflow-wasm` from 2.1.3
  through 3.2.0 shipped a binary whose externref table declares
  `maximum == initial`, while the JS glue beside it calls `table.grow(4)` during
  init. Initialization threw `RangeError: WebAssembly.Table.grow(): failed to
  grow table by 4` on the first call, so the package could never start in any
  browser. `release.yml` was the only pipeline installing binaryen from apt, and
  its extra `wasm-opt` pass omitted `--enable-reference-types`; `ci.yml` and
  `docs.yml` let wasm-pack fetch its own binaryen and were unaffected.
  Optimization now lives once in `wasm/Cargo.toml` under
  `[package.metadata.wasm-pack.profile.release]`, wasm-pack is pinned rather
  than tracking whatever is latest on release day, and `verify-wasm.mjs`
  instantiates the built binary and fails the release if the table cannot grow
  or the glue and binary come from different builds.

### Changed

- **tests:** the single 4,408-line integration file is split into ten
  topic-scoped files under `tests/`, one binary each, with shared fixtures in
  `tests/common/mod.rs`.
- **ui:** `buildFlowGraph` no longer duplicates its task-emitting logic across
  separate with-condition and without-condition branches. Output for
  non-looping workflows is unchanged.

### Compatibility

- **`loop` is opt-in and costs nothing when absent.** A workflow without one
  takes the same code path it always did, with no added per-message checks.
- **Reaching `max` is normal completion, not an error.** The bound is always
  author-supplied. If the loop stops at `max` while the condition was still
  true, the engine logs a warning.
- **The workflow condition is a loop guard, not a per-sweep filter.** It is
  re-evaluated between sweeps, and going false ends the loop rather than
  skipping one iteration. Use a `filter` task with `on_reject: halt` to stop
  part-way through a sweep; that breaks the whole loop.
- **The engine owns the counter.** It is rewritten before every sweep, so a body
  task writing the same `temp_data` path has its value replaced at the next
  increment.
- **Looping workflows do not join the shared-arena fully-sync run.** The arena
  is a bump allocator that never frees mid-scope, so one scope per sweep keeps
  memory flat instead of growing with the iteration count.
- **Audit volume scales with sweeps.** A 1,000-sweep loop over 3 tasks records
  3,000 entries. `max` is what keeps that finite.
- Wire shapes are otherwise unchanged: `loop_counter` is omitted when absent,
  and a workflow JSON without `loop` deserializes exactly as before.

## [3.2.0] — 2026-07-31

`datalogic-rs` ships `default = []`, and this crate enabled only `serde_json`
and `templating` — so every extension operator (`upper`, `split`, `sort`, `abs`,
`try`, `parse_date`, …) was compiled out with no way for a consuming application
to turn one on except by declaring its own `datalogic-rs` dependency and relying
on cargo's feature unification. Each family is now a feature of this crate.

### Added

- **features:** opt-in passthrough features for the `datalogic-rs` operator
  families — `ext-string`, `ext-array`, `ext-math`, `ext-control`,
  `error-handling`, `datetime`, and the `all-operators` umbrella. Names mirror
  `datalogic-rs`'s own. All are **off by default**; `default = []` is unchanged,
  so nothing changes for existing dependents until they opt in.
- **wasm:** `dataflow-wasm` enables `all-operators`. The npm artifact's JS
  consumers have no cargo-feature knob, so the operator set is compiled in.
- **ci:** default-features clippy and test steps, plus a per-family isolation
  loop. Every previous job was `--all-features`, which left the configuration
  `cargo add dataflow-rs` produces entirely unbuilt.
- **tests:** `#[cfg]`-gated coverage of both sides of each family gate in
  `src/engine/compiler.rs`, including a tripwire pinning the `datetime`
  comparison change below.

### Compatibility

Enabling an operator family is **not** a backwards-compatible no-op. Read this
before turning one on.

- **Data keys become operator calls.** The engine always runs `datalogic-rs` in
  templating mode, where an unrecognised operator name is not an error — the
  object passes through as literal data. Enabling a family makes its names live,
  so a `map` mapping that carried `{"length": {...}}` as a *value* starts
  storing a number instead. Plausible-as-data names include `type`, `match`,
  `datetime`, `timestamp`, `length`, `split` and `sort`. Audit rules for these
  keys before enabling the corresponding family.
- **`datetime` changes core operators.** With it on, `==`, `<`, `<=`, `>` and
  `>=` parse plain date-shaped strings as instants rather than comparing bytes.
  `{"==": ["2024-01-15T00:00:00Z", "2024-01-15T01:00:00+01:00"]}` is `false`
  without the feature and `true` with it. `type` also starts reporting
  `"datetime"`/`"duration"` for such strings.
- **`--workspace` no longer exercises the default build.** `dataflow-wasm`
  requires `all-operators` and cargo unifies features across workspace members,
  so `cargo test --workspace` compiles `dataflow-rs` with every family on. Use
  `cargo test -p dataflow-rs` for the default configuration.

No new packages enter the dependency graph: the `datetime` family's `chrono` was
already a direct dependency, and the other five have no dependencies.

### Changed

- **deps:** `uuid` requirement `1.23` → `1.24`; lockfile refresh picks up
  `http` 1.5.0, `jiff` 0.2.35 and `tokio-macros` 2.7.2. All semver-compatible.
- **deps(ui):** lockfile refresh within the existing ranges — `dataflow-wasm`
  3.1.0, `vite` 8.2.0, `lucide-react` 1.28.0, `globals` 17.8.0, `@types/react`
  19.2.18, `@types/react-dom` 19.2.4, `@vitejs/plugin-react` 6.0.5. No declared
  range in `ui/package.json` changed. `typescript` is deliberately held at 6.x;
  7.0 is a major bump and `build:lib` runs `tsc`, so it needs its own change.
- **deps(ui):** dropped the unused `@microsoft/api-extractor` tree from the
  lockfile. `vite.lib.config.ts` calls `dts()` without `rollupTypes`, so it was
  never used by the build; `scripts/verify-dts.mjs` confirms the declaration
  bundle is unaffected.

### Fixed

- **docs:** `docs/src/advanced/jsonlogic.md` documented a `typeof` operator that
  does not exist; the operator is `type` and it requires `ext-control`.
- **docs:** `docs/src/built-in-functions/validation.md` documented a
  `regex_match` operator that does not exist in `datalogic-rs` under any
  feature. Replaced with an achievable substring check and a pointer to custom
  functions for real pattern matching; the neighbouring `length` example is now
  marked as requiring `ext-string`.
- **docs:** the installation page pinned `dataflow-rs = "3.0"`, two versions
  stale.

## [3.1.0] — 2026-07-30

Four defects from a boundary audit of the crate's deepest known consumer, plus
the capture-policy surface the trace API was missing. Minor rather than patch
because new public items ship and several existing behaviours change.

### Added

- **task-outcome:** `HALT_STATUS_CODE` is re-exported from the crate root.
  Public since v3 but only reachable as
  `dataflow_rs::engine::task_outcome::HALT_STATUS_CODE`.
- **tests:** operator-semantics coverage in `src/engine/compiler.rs`, pinning
  the `datalogic-rs` behaviour this crate's own code depends on against a live
  engine built the way `LogicCompiler` builds one — empty-operand results
  (`{"and":[]}` → `null`, `{"+":[]}` → `0`, …), a missing `var` path resolving
  to `null` rather than erroring (the mechanism behind the `payload.*` pitfall
  CLAUDE.md documents), the documented truthy/falsy table (previously an
  unverified `json` fence in the guide), and — the one that matters most —
  that an unrecognised or feature-gated operator name (`starts_with`, not
  enabled by this crate's `Cargo.toml`) is **not** an error under templating:
  it echoes back as a literal object. That silent-pass-through is exactly why
  a static "known operators" table was refused earlier in this audit; now it's
  a regression test instead of a comment.
- **functions:** `Template` and `TemplateCompiler`, plus a defaulted
  `AsyncFunctionHandler::compile_input` hook. Lets a custom handler declare a
  config field whose authored JSON is JSONLogic — the same `*_logic` pattern
  this crate's own `HttpCallConfig` / `EnrichConfig` / `PublishKafkaConfig` use
  internally — without hand-rolling the raw/compiled pair and the eager
  "fail loud at construction" plumbing each time. `compile_input` is called once
  per task at `Engine::new` / `Engine::builder().build()` /
  `Engine::with_new_workflows`, immediately after `parse_input`; a malformed
  expression fails there rather than on the first message that reaches the
  task, matching the existing stance for the built-in `*_logic` fields. The
  default is a no-op, so a handler with no `Template` field needs no override —
  verified by building every existing test handler, `mod tests` fixture, and
  example unchanged. `Template` fields nested inside a `Vec<T>` or a nested
  struct work by walking the collection inside `compile_input`. See *Changed*
  for the built-in integration configs' own migration onto this type. (#29)
- **integration:** `HttpMethod` is re-exported from the crate root (previously
  reachable only as
  `dataflow_rs::engine::functions::integration::HttpMethod`) and gains
  `ALL`, `as_str`, `is_idempotent`, `Display`, and the `Copy`/`PartialEq`/`Eq`/
  `Hash` derives. Because this crate does not implement `http_call`, every
  consumer converts this enum into their own client's method type and wrote the
  same five-arm match to do it; `as_str()` is the intended bridge, and the crate
  still takes no HTTP-client dependency. `ALL` is scoped to what an `http_call`
  task may name — not a general HTTP method list. (#24)
- **trace:** `TraceOptions` (re-exported from the crate root, with
  `AuditTrailScope`) plus `Engine::process_message_with_trace_options` and
  `Engine::process_message_for_channel_with_trace_options`. Bounds what a trace
  captures *at capture time*, which is the only place it can be bounded —
  trimming the result afterwards has already paid the peak memory. Knobs:
  `snapshots`, `mapping_contexts`, `changes`, `max_snapshot_bytes` (approximate
  in-memory size, not serialized length), `redact_paths`, and
  `snapshot_audit_trail`. `TraceOptions::timings_only()` is the metrics preset.
  The default reproduces the historical capture behaviour exactly. (#27)
- **trace:** `ExecutionStep` gains `started_at`, `duration_us` and `changes`;
  `ExecutionTrace` gains `truncated()`, `options()` and `with_options()`.
  Per-task timing now covers the **sync built-ins** — `map`, `validation`,
  `filter`, `parse_*`, `publish_*`, `log` are dispatched inside the executor and
  cannot be wrapped from outside the crate, so this is the only place their
  duration is observable. (#27)
- **error:** `DataflowError::Service { kind, message, detail, retryable }`, built
  through `DataflowError::service(..)` / `ServiceErrorBuilder`, plus
  `DataflowError::kind()` / `detail()` and a new `ErrorInfo::detail` field with a
  matching builder setter. A handler-owned classification channel: `kind` becomes
  `ErrorInfo::code` **verbatim** (not upper-cased, so the string a service writes
  is the string it switches on), `detail` is an operator-only field that `Display`
  never renders — `to_string()` stays safe for an untrusted caller — and
  `retryable` is declared rather than inferred from the variant. The engine never
  interprets any of it; `continue_on_error`, the audit entry and the `Result::Err`
  short-circuit are unchanged, and no built-in returns the variant. Lifted at the
  task site only, so the `WORKFLOW_ERROR` wrapper keeps its own code and counting
  errors by code does not double-count. (#31)
- **message:** `MessageBuilder::data` / `metadata` / `temp_data` and their
  `*_json` siblings seed the three context root fields directly, so a workflow
  condition reading `data.*` fires without a `parse_json` task first. Keys are
  taken literally (unlike `set_nested_value`, a dotted key stays one key and a
  leading `#` is not stripped) and a non-`Object` value is ignored, preserving the
  invariant that the three root fields are always objects. Seeding records no
  audit entry and no `Change`. (#30)
- **workflow:** `Workflow::rollout: Option<Rollout>` plus `Rollout` and
  `Message::routing_bucket()` / `MessageBuilder::routing_bucket()` — half-open
  bucket ranges over `0..100` giving a workflow a slice of its channel's traffic.
  The engine does not derive the bucket; how a caller maps to one stays its
  policy. A message with no bucket is admitted by every workflow, so every
  existing caller and the WASM entry points are unaffected. An excluded workflow
  is skipped exactly like a false condition — no audit entry, `metadata.progress`
  untouched, one workflow-level `Skipped` step in a trace — and the gate runs
  before any arena work. (#33)
- **task-context:** `TaskContext::context()` plus a value-returning evaluation
  surface — `eval` (→ `OwnedDataValue`), `eval_json` (projected straight from the
  arena to `serde_json::Value`, skipping the `from_value` rebuild) and
  `eval_to_plain_string`. Unlike `executor::evaluate_condition`, these return the
  value rather than collapsing it to a bool, and surface an evaluation failure as
  `Err` rather than `false` — a condition that fails should not run its task, but
  a handler reading a config value needs to know the read failed. All three
  evaluate on the worker thread's pooled arena rather than allocating a session
  per call. (#23)
- **integration:** `resolve_path` / `resolve_body` on `HttpCallConfig`,
  `resolve_path` on `EnrichConfig`, and `resolve_key` / `resolve_value` on
  `PublishKafkaConfig`. Each applies the logic-then-static fallback, coerces path
  and key results to a plain string (these end up in URLs and partition keys),
  and propagates an evaluation failure instead of silently substituting the
  static value. `resolve_value` returns `Option<Value>` rather than
  `Option<String>` on purpose, so a producer that serializes unconditionally is
  not forced through the key's coercion. (#23)
- **lib:** `datalogic_rs` and `datavalue` are re-exported from the crate root.
  Both are unavoidable for handler authors — `TaskContext::datalogic()` returns
  `&Arc<datalogic_rs::Engine>`, `HttpCallConfig::compiled_path_logic` is an
  `Option<Arc<datalogic_rs::Logic>>`, and the whole context/path surface is in
  terms of `datavalue::OwnedDataValue` — yet neither was reachable without a
  duplicate direct dependency, and `datavalue` is published under a different
  name (`datavalue-rs`) than it is used under, so the manifest line was hard to
  guess. Reaching them through here also locks their major version to whatever
  this crate depends on. Proven mechanically: the `docs-tests` crate's duplicate
  `datavalue` pin is deleted and its snippets now import through the re-export,
  so the only `datavalue` pin in the workspace is the root manifest. (#26)
- **utils:** `remove_nested_value` completes the dot-path helper API. Previously
  the closest available operation was `set_nested_value(path, Null)`, which is not
  removal — it leaves an explicit `null` that survives every serialization
  boundary, because `Message` emits `context` whole. Object removal preserves the
  order of surviving keys; array removal shifts the tail rather than leaving a
  hole. (#21)
- **functions:** `FunctionConfig::connector()`, `Workflow::connector_refs()` and
  `ConnectorRef`. Which configs carry a connector is this crate's fact, and every
  consumer that reimplements the set is a silent-breakage site the next time one
  is added. Covers the three typed integrations plus the `Custom`
  `input["connector"]` convention; the match is exhaustive so a future
  connector-bearing variant cannot be silently omitted. (#32)
- **observer:** `ExecutionObserver` and `TaskEvent`, attached via
  `EngineBuilder::with_observer` or `Engine::with_observer`. An always-on
  per-task callback for aggregation, as distinct from a trace you persist. This
  is the only way to time the eight sync built-ins, which are dispatched inside a
  private executor method and never reach the function registry — a host can wrap
  its own handlers but could not touch `map`, `validation`, `filter`, `parse_*`,
  `publish_*` or `log` at any price, and so could not tell engine time from
  handler time. Emitted before the error propagates, so failing tasks are
  reported rather than dropped; a task whose condition was false is not reported.
  Carried across `with_new_workflows`, so a hot reload does not silently stop
  reporting. With no observer attached the instrumentation and its clock reads
  stay out of the dispatch path entirely. (#28)
- **engine:** `EngineBuilder::with_handlers` takes a whole
  `HashMap<String, BoxedFunctionHandler>`, keeping any already registered. Without
  it, an embedder that builds the map in one place was pushed onto `Engine::new`
  and off the builder — and therefore out of reach of `with_observer`. (#28)
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

- **trace:** a step's per-task diff was only recoverable as
  `step.message.audit_trail.last()`, which is wrong whenever a task returns
  `TaskOutcome::Skip` — no audit entry is recorded for a skip, so the last entry
  belongs to a *different* task. Reachable with built-ins alone via `filter` with
  `on_reject: "skip"`. `TraceOptions::changes` reports each task's own writes,
  and the `dataflow-ui` helper now prefers it. (#27)
- **integration:** `HttpCallConfig`, `EnrichConfig` and `PublishKafkaConfig` now
  reject unknown keys (`deny_unknown_fields`). A misspelled field previously
  parsed cleanly and was discarded, so an `http_call` task would make its request
  and silently throw the response away — no error at
  `Engine::builder().build()`, none at dispatch. The failure now arrives when the
  workflow definition is parsed, naming the offending field. (#24)

### Changed

- **integration:** the built-in integration configs migrate onto `Template`.
  `HttpCallConfig::path_logic` / `body_logic`, `EnrichConfig::path_logic`, and
  `PublishKafkaConfig::key_logic` / `value_logic` are now `Option<Template>`
  (were `Option<Value>`), and the five matching `compiled_*` slots — marked
  `#[doc(hidden)]` earlier in this release specifically so this could follow —
  are removed, since each raw/compiled pair collapses into the one field that
  used to be raw-only. `LogicCompiler`'s three near-identical `compile_*_logic`
  methods (~64 lines) shrink to one `Template::compile` call per field.
  `resolve_path` / `resolve_body` / `resolve_key` / `resolve_value` — the
  sanctioned read — keep their exact signatures, so any caller already using
  them is unaffected. The wire JSON is unaffected too: `Template::deserialize`
  accepts the same shape `Value` did, so `{"path_logic": {...}}` parses
  identically. `Template` gains `eval_to_plain_string`, mirroring
  `TaskContext::eval_to_plain_string`, so `resolve_path` / `resolve_key` keep
  their non-string-coerces-to-compact-JSON behaviour. See *Compatibility* for
  what this breaks. (#29)
- **engine:** `DataflowError::FunctionNotFound`'s message is now documented as
  free-form and explicitly unpinned, with the new classifier as the supported
  programmatic route. No test asserts on its wording, and none should — pinning
  it would cement the scraping workaround #22 exists to remove.

### Compatibility

No existing **function or method signature** changes shape. Two structs gain
fields (`ExecutionStep`, `ExecutionTrace` — see *Notes for trace consumers*).

One field-level break, from the `Template` migration above: `HttpCallConfig` /
`EnrichConfig` / `PublishKafkaConfig` retype their four `*_logic` fields from
`Option<Value>` to `Option<Template>`, and drop the five `compiled_*` fields
entirely. This breaks source code that reads `cfg.path_logic` (or the sibling
fields) directly and expects `Option<Value>`, or that reads any `compiled_*`
field at all — those were already `#[doc(hidden)]` and documented as not part
of the stable API earlier in this same release. Code going through
`resolve_path` / `resolve_body` / `resolve_key` / `resolve_value` — the
documented, sanctioned read — is unaffected: those methods keep their exact
signatures and behaviour. Nothing in `wasm/` or `ui/` touches these fields.
The wire JSON format is unaffected in both directions.

Everything else in this release is additive. Behaviours that change at runtime:

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

### Notes for trace consumers

`DataflowError` and `ErrorInfo` are now `#[non_exhaustive]`, so a downstream
`match` over the error enum needs a wildcard arm and `ErrorInfo` must be built
through `builder()` / `new()` / `simple()`. Both gained a variant or field in this
release, which already broke exhaustive matching and struct literals; marking them
makes future additions non-breaking. Nothing outside `src/` in this repository
matched the enum exhaustively or constructed `ErrorInfo` literally. `Workflow`
deliberately did **not** get the marking — it would forbid `..Default::default()`
cross-crate, which is the shape this repo's own integration tests use.

`ExecutionStep` and `ExecutionTrace` are now `#[non_exhaustive]`. Both gained
fields in this release, which already broke out-of-crate struct-literal
construction and exhaustive destructuring; marking them makes future field
additions non-breaking. Field reads, `..` patterns, the three step constructors
and the `with_*` chain are unaffected, and nothing in this repository or the wasm
bindings constructed either type by literal. `StepResult` is deliberately left
exhaustive — a new variant there should break downstream `matches!` sites at
compile time rather than silently reclassify them.

Under default options the serialized trace gains `started_at` and `duration_us`
per executed step. `truncated` is serialized only when `true`, so a complete
trace keeps the historical wire shape. Neither `ExecutionStep` nor `Message` sets
`deny_unknown_fields`, so old and new payloads deserialize in both directions.

`ui/`: `ExecutionStep` gains the three optional fields, `ExecutionTrace` gains
`truncated?`, `getChangesAtStep` prefers `step.changes` with the audit-trail read
kept as a fallback, and a new `traceHasSnapshots()` helper reports whether a trace
carries state to inspect at all.

Workspace test count 184 → 369.

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
