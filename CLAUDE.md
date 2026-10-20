# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Dataflow-rs is a lightweight, rule-driven workflow engine for building data
processing pipelines and nanoservices in Rust. Workflows are JSON definitions
whose tasks run built-in or host functions over a message's context, with
JSONLogic conditions and expressions compiled once at build.

This is a **Cargo workspace with four published artifacts**, version-locked to
the engine's version, authored once in `[workspace.package] version` and
inherited by the published Rust crates:

| Path | Artifact | Registry |
|---|---|---|
| `.` | `dataflow-rs`: the engine | crates.io |
| `derive/` | `dataflow-rs-derive`: `#[derive(FunctionConfig)]`, published first | crates.io |
| `wasm/` | `@goplasmatic/dataflow-wasm`: the root crate on `wasm32-unknown-unknown` with `wasm-web`, `datetime` and `ExprEngineBuilder::all_operators` | npm |
| `ui/` | `@goplasmatic/dataflow-ui`: the React debugger, on the wasm package's generated types | npm |

`wasm/` and `derive/` are workspace members; `ui/` is a separate npm project
driven by the release workflow. A release bumps `[workspace.package] version`
and `ui/package.json`'s version; `scripts/check-versions.sh` holds the rest and
runs on every PR.

The other workspace members are not published: `docs-tests/` (the book's and
the README's doctests, against the root) and `corpus/` (the workflow cases the
engine's tests load, and independent models of its path and replay rules). See
"The corpus" below.

**`main` is 4.x**, and holds only v4: 3.x
survives as the published crates and the `v3.*` tags, and nothing on `main`
builds or tests against it. Design questions are decided for Orion, the main
host, in priority order: performance, then developer experience, then a
compact engine. The 4.0 port of Orion is tracked in
GoPlasmatic/Orion#385.

## Layout

- `src/lib.rs`: the crate doc, the features, and the sixteen root re-exports
  (`Engine`, `Message`, `Run`, `RunOptions`, `Error`, `ErrorKind`, `Issue`, `Result`,
  `WorkflowDef`, `Expr`, `Path`, `Ctx`, `Function`, `Outcome`, `Registry`,
  `Observer`). `extern crate self as dataflow_rs`, so the derive's
  `::dataflow_rs::` paths resolve inside the crate.
- `src/base/`: private, below everything: `Error` and `ErrorKind`,
  `ErrorRecord`, `Change`, `Context`, `Issue` and its codes, the ids
  (`ProgramId`, `WorkflowIx`, `TaskIx`), `CancelToken`, `Instant`, the typo
  distance, and the `ts` feature's hand-written declarations.
- `src/def/`: definitions. `parse` and `parse_owned` share the reader and linter; `types.rs`
  the read-only data (`WorkflowDef`, `TaskDef`, `GroupDef`, `LoopDef`,
  `ForEachDef`, the policies, `defaults`), crate-private fields behind read
  accessors, so nothing edits a def after `parse`; `reader.rs` the compat reader;
  `keys.rs` the known-key tables; `ser.rs` the v4-spelling serializer.
- `src/expr/`: expressions, only compiled. `ExprEngine` (operators, secrets,
  the `$` escape, the ops budget), `Expr<T>`, `Interpolated` documents,
  `Output`, `Path`, `Secrets`, `ConfigField` and `CompileCx`, the operator
  facts (datalogic's own, through the families this crate's features turn
  on), the arena, and `rules`: the path rules stated once (`step` per
  segment, the append target, value shapes, the one-segment primitives),
  which the overlay and the write log apply and the two hot walks (the owned
  `path::set` and the splice) inline for speed, held to the rules' reference
  walk on every small tree and path (`state::tests::splice_property`).
- `src/function/`: `Function`, `FunctionConfig`, `Ctx` and `CallInfo`,
  `Outcome`, the overlay, `CallLayer` and `layers::Answer`, `Registry`,
  `FunctionSpec`, `Templated`, `testing`, and `builtins/` (the eight built-in
  configs, invoked synchronously over the crate-private `SyncCtx`).
- `src/program/`: `Compiler` (`check`, `check_steps`, `compile_one`, `link`),
  the checks, the lowering into six ops, fingerprints, `Program`.
- `src/run/`: `Engine`, `EngineBuilder`, `EngineConfig`, `ErrorContext`,
  `RunOptions`, `Message` and `AuditMode`, `Run`, the interpreter, the
  recorder, host calls, fan-out, the deadline and token, observers, the trace
  capture.
- `src/state/`: `RunContext`, the one writer: the run view, the write log,
  the splice and the checkpoint, with its model tests.
- `src/observe/`: `Observer`, `Interest`, `Event`, `Trace`, `TraceStep`,
  `TraceOptions`.
- `src/compat/`: `migrate_v3`.
- `derive/src/lib.rs`: the derive.
- `tests/`: `it/`, one binary with a module per topic (the table under
  "Testing") and `it/support/` the shared fixtures; `secrets_isolation.rs`
  and `ts_types.rs`, their own binaries.
- `examples/`: the thirteen gate benchmarks, the six functional examples, and
  `examples/shared/` (not an example).
- `bindings/dataflow.d.ts`: the wire shapes as TypeScript, generated under the
  `ts` feature and held by a drift test; the wasm package embeds it.
- `corpus/`, `bench/`, `wasm/`, `ui/`, `docs/`, `docs-tests/`: below.

## Development Commands

### Build and Test

```bash
cargo build                              # the root
cargo test                               # the root's default build
cargo test --workspace --all-features    # what CI runs
cargo test -- --nocapture                # with output
```

A bare cargo command at the root selects the root package. Every other member
takes `-p`, as in `cargo test -p dataflow-corpus` or
`cargo test -p dataflow-docs-tests`.

### Code Quality

Run all of these before handing back any Rust change. CI enforces them and
treats warnings as errors:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
```

The doc check is required. Clippy does not lint rustdoc, so nothing else catches
a broken intra-doc link, a link from public docs into a private item, or an
unbackticked generic like `Arc<Logic>` that rustdoc parses as an HTML tag.
`src/lib.rs` warns on `missing_docs` and `missing_debug_implementations`, and
CI's `-D warnings` makes both errors: every public item needs a doc comment and
every public type a `Debug` impl (a manual one with `finish_non_exhaustive()`
where a field is not `Debug`). Public doc comments cite reasons, never a design
document's IDs, which no docs.rs reader can follow.

Lint *policy* lives in `[workspace.lints]` in the root `Cargo.toml`, inherited
by every member, so a bare `cargo clippy` and rust-analyzer enforce what CI
enforces. Adding a lint there means fixing every existing violation in the same
change: CI's `-D warnings` gives no grace period.

`--all-targets` covers examples, tests and benches; `--all-features` covers the
`wasm-web` feature, which cargo otherwise skips. Fix all clippy warnings.

The third line lints the root's **default** build, which `cargo add dataflow-rs`
delivers. `--workspace` cannot do this: `dataflow-wasm` depends on the root with
`wasm-web` and `datetime`, and cargo unifies features across the members one
invocation selects, so under `--workspace` the root itself has `datetime` on.
The same applies to `cargo test`.

The root's own checks, beside the workspace-wide ones above:

```bash
cargo clippy --all-targets -- -D warnings
cargo clippy -p dataflow-rs-derive --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown --features wasm-web -- -D warnings
cargo clippy --target wasm32-unknown-unknown --features wasm-web,datetime -- -D warnings
cargo clippy -p dataflow-wasm --target wasm32-unknown-unknown --all-targets -- -D warnings
cargo test
cargo test --features ts --test ts_types
cargo test -p dataflow-corpus
cargo test -p dataflow-docs-tests
for f in datetime tensor budget; do
  cargo test --no-default-features --features "$f"
done
cargo +1.98 check --workspace --all-targets --all-features --locked
cargo deny check
```

**MSRV is 1.98 and CI enforces it.** The floor is *inherited, not chosen*:
`datalogic-rs` 5.8 and `datavalue-rs` 0.3 both declare
`rust-version = "1.98"`, so it moves when they move. Let-chains are fine.

**`cargo test` at the root must stay under a minute, rebuild included.** The
run-context model test runs once, in the default build, at 150 sequences per
tuning; `DATAFLOW_DEEP=1 cargo test --lib state::` runs its full 2,000
(about a minute on its own). Run it after any change under `src/state/`. Time
the suite like for like, in a scratch worktree with its own target directory,
and time a control commit beside the head whenever a reading crosses the
minute: the owner's Docker stack doubled a reading once (34.6 s against 1:09,
with `main` at 1:05 minutes later). In the repository's own `target/`, macOS
scans each newly linked test binary on its first launch, which can add minutes.

### Test targets

`cargo test --workspace --all-features` runs the root library and its three
integration binaries (`it`, `secrets_isolation`, `ts_types`), the derive crate,
the corpus crate, and `dataflow-docs-tests` (which compiles the book's and
READMEs' Rust blocks against a default-feature build). Because `datetime`,
`tensor` and `budget` are `#[cfg]`-gated, the root's test count varies by
feature set (`--all-features` vs default). Do not hard-code absolute test counts
in docs or tests.

### Examples

The root's examples are the thirteen gate benchmarks and the six functional
examples. Always run a benchmark with `--release`; a debug build measures
nothing useful:

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
cargo run --example orion_connector --release            # Orion-shaped connector: document folds, output, a layer
cargo run --example observed_run --release               # The realistic workload with a counting observer
```

The six functional examples run in a debug build and print a walk-through:
`hello_world`, `rules_engine`, `complete_workflow`, `custom_function`,
`error_handling` and `migration` (a 3.x definition through `migrate_v3`, and a
3.x-style handler as a `Function`).

`micro_subtree_write_bench`, `micro_multiworkflow_bench` and
`map_performance_test` are regression guards for optimizations that landed in
3.x (the arena write-through splice and the shared-arena workflow run). Read
them as "this must stay flat". `loop_scaling`, `large_context_handler`,
`fan_out`, `orion_connector` and `observed_run` measure what a message costs
per byte or node it carries, or per clock read: where 4.0's design pays off,
and where a regression would show first.

**A benchmark checks its result before it times anything.** A workload that
silently does less than it says still reports a plausible number:
`realistic_benchmark` once wrote `data.MT103.20`, an array index, which turned
`data.MT103` into a null-padded array and failed 8 of its 12 rules until the
paths were spelled `#20`. Numbers carry ±2-3% run-to-run noise: compare the
mean of 3+ runs, and **interleave A/B runs**, since all "before" then all
"after" conflates the change with thermal drift.

**CI runs every example, smoke.** Under `DATAFLOW_BENCH_SMOKE=1` a benchmark
runs its startup check in full, then a few iterations (`gate::count(full,
smoke)` in `examples/shared/gate.rs`; only repetition counts shrink, never a
shape a check depends on), and marks its `GATE` line `"smoke": true`.
`python3 bench/gate.py smoke --bins target/debug/examples` runs all nineteen
(about 15 s in debug): a benchmark must print exactly that line, any other
example exit 0 with none. `gate.py baseline` and `compare` refuse to start
with the variable set and refuse a smoke line, so a smoke number is never a
measurement (`bench/gate_test.py`). A new benchmark reads its counts through
`gate::count` and joins `BENCHES`. CI's weekly `deep` job (also on demand and
on a release tag) runs the model test at full depth and the smoke run on the
release build.

### The gate

**The gate compares two builds of the engine,** a base commit against a
candidate, through the thirteen benchmarks, each keeping its workload,
startup check, `GATE` name and metric names. Each prints one machine-readable
line, `GATE {...}`, last (`examples/shared/gate.rs`), which `bench/gate.py`
reads:

```bash
git switch --detach <base>  && python3 bench/gate.py build --out /tmp/gate/a
git switch <candidate>      && python3 bench/gate.py build --out /tmp/gate/b
python3 bench/gate.py compare --a /tmp/gate/a --b /tmp/gate/b --rounds 5
```

`build` builds HEAD from a clean tree. `compare` alternates the two builds
within each round and judges each metric on the median of its per-round
ratios, with noise `max(3%, half the IQR)`, and reports instructions retired
(`/usr/bin/time -l`), which separate load from work on a busy machine; they
move with codegen too, so attribute a change to one item only with a
same-binary toggle. `baseline --bins DIR --rounds 5 --out FILE` writes every
run with the build, machine and method. A metric's name is a contract: a
benchmark keeps it. On this machine the 3% noise floor holds for one-thread
rows only; with the owner's Docker stack busy, the spawned throughputs spread
10 to 18%: read the one-thread rows and the instructions retired, and re-run a
spawned row at seven rounds before calling it WORSE.

Orion's baseline is `bench/baseline/orion-1.12.0.json`, from `bench/orion.sh`
(`ORION_TAG=v1.12.0`), which runs Orion's own `bench.sh` scenarios with the
server's peak RSS; `ORION_COMMIT=<sha>` benches a branch instead, and
`bench/orion_compare.py` judges a run against the baseline. Its
`cluster` scenario brings Orion's HA stack up as its own compose project on a
free loopback port, never 8080.

### The corpus

**The corpus** (`corpus/`, see its README) is a set of workflow cases,
`corpus/cases/<area>/<name>.json`, each with its definitions, engine and
steps, many in 3.x spellings the compat reader rewrites. `dataflow-corpus`
loads them (`load_cases`) and holds the case format, the wire normalisation
and independent models of the path rules and the trace replay, free of
`dataflow-rs`. The root's tests read them: `tests/it/corpus.rs` runs every
case through the driver (`tests/it/support/corpus_driver.rs`: `script` and
`shout` registered, each run plain and traced, the two agreeing, the trace
replaying to the message by both replays, everything reading back) and holds
it to `tests/golden/corpus/<area>/<name>.json`; `compat.rs` migrates every
definition; `ts_types.rs` runs the same driver and holds every key a run
writes to its declaration; `repository_definitions.rs` parses each one; and
`path.rs`, `trace.rs` and `error_context.rs` hold the engine to the models.
A case with run steps fails on an `UNKNOWN_FUNCTION` refusal or an engine
that compiled nothing; a refusal case carries no run step. Review a golden
diff against each case's description.

### Golden files

A test that pins a whole output, too large for a literal, reads it from a
golden file under `tests/golden/` (the lib's own tests under
`tests/golden/lib/`, outside `src/` so nothing ships to crates.io), compared
as text, key order included. `DATAFLOW_GOLDEN_WRITE=1` rewrites them; review
the diff as a behaviour change. The crate turns on serde_json's
`preserve_order`, so a `json!` literal keeps its source order; the lib's
golden harness still sorts the seeds, payloads and inputs it is given, so a
golden does not move when a test reorders a literal.

### The documentation

Every Rust example in `docs/src/**.md` and `README.md` compiles against the
root crate through `docs-tests` (`cargo test -p dataflow-docs-tests`), a
separate member because `docs/` is not in the published crate's `include`
list. Its `tests/coverage.rs` fails if a page is missing from the page list in
`docs-tests/src/lib.rs`. When editing docs: fragments get a hidden `# `
preamble rather than an `ignore` tag; an unlabelled fence is compiled as Rust,
so tag JSON `json` and diagrams `text`. JSON definitions in the docs are not
compiled: copy them from a corpus case or a test that runs. `mdbook build docs`
builds the book; `docs/build-with-debugger.sh` adds the UI.

## Architecture

**Three types for three phases.** A `WorkflowDef` is raw data, entered only
through `def::parse`, which is also the linter: one walk over the authored JSON
reports every issue at the path the author typed, at v3's codes for v3's rules,
after the compat reader has rewritten 3.x spellings (each an Advisory). A
`Program` is compiled and immutable: a `Registry` (functions over an
`ExprEngine`) gives a `Compiler`, `compile_one` compiles one workflow so a host
owns quarantine, and the infallible `link` sorts by priority and numbers tasks.
A `Run` is one message: `Engine::run_with` returns one on every outcome, with a
typed `RunStatus`, each workflow's `Selection` and `WorkflowExit`, `matched`,
`retryable`, `partial` and the trace.

**One interpreter over six ops** (`Cond`, `Call`, `FanOut`, `LoopInit`,
`LoopNext`, `Halt`): groups, loops and fan-outs are jumps, and one recorder
writes, in order, the audit entry, the progress write, the error records and
their mirror, the trace step and the observer event. With no observer, trace
or error context, a call takes the plain recorder: the unobserved path pays
one branch per call.

**One writer, one view.** After the run-start stamps, every context write goes
through `state::RunContext`: one arena view of the context, built at the run's
first read; every write spliced into it and marked in a dirty-path trie; a
checkpoint that converts only what changed. No step, call, sweep or fan-out
element rebuilds the view.

**Functions.** A host function implements `Function` (`type Config:
FunctionConfig`, `spec`, `prepare`, `async fn call(&self, ctx: &mut Ctx<'_>,
cfg: &Self::Config)`); its writes go to a per-call overlay, replayed at their
paths in order on `Ok` and dropped whole on `Err`. The same overlay is the
fan-out fork, so isolation costs no clone. The eight built-ins run inline on
the crate-private `SyncCtx` and write through immediately. Layers wrap
every host call at registry scope (`Registry::layer`, `EngineBuilder::layer`)
and at run scope (`RunOptions::layers`); the built-ins are never wrapped.

**Expressions exist only compiled.** `Expr<T>` holds JSONLogic; a folded
constant is borrowed, never cloned. `Interpolated` holds documents (Mongo
filters, SQL parameters) that substitute `var` holes and keep their `$` keys,
rendered as a borrowed view straight into the host's serializer. A config's
field type is its position: `#[derive(FunctionConfig)]` yields `compile`
(every issue at its exact path, unknown keys refused), `visit`, `fields`,
`primary_output` and `resolve`.

**Observation.** An `Observer` declares its `interest()` once per run and gets
`on(&Event)` inline; no clock is read without `TIMINGS`. A trace is the
redacted initial context plus one `TraceStep` per call, skip, failure and
engine write, each with its changes, so `Trace::contexts` replays to the
context after every step and `Trace::mapping_contexts` to the context after
each of a `map` step's mappings; the wasm package exports both, and the UI
keeps no copy of the rules.

## Rules that must hold

Each rule lives once in the code; the test named beside it is what pins it.

- **The eval context is `{data, metadata, temp_data}` only.** `payload` is a
  field of `Message`, not of the context: a literal `var` path rooted at
  `payload` is a `PAYLOAD_VAR` Defect at compile, and `parse_json` with
  `"source": "payload"` is how the payload reaches `data` (`tests/it/checks.rs`).
- **`metadata.progress` is written after every recorded call** that is not a
  `Skip`, as `{workflow_id, task_id, status_code}`; cross-workflow chaining
  reads it. Never gate or skip the write (`tests/it/run_basics.rs`, the corpus).
- **A policy applies after the status is classified.** A task's `on_failure`
  (`fail`, `continue`, `halt`) covers a 5xx or an `Err`, `on_rejection`
  (`continue`, `halt`) a 4xx; `terminal` ends the workflow after either. A
  failing `validation` returns 400 and records `VALIDATION_ERROR` per rule;
  the next task runs unless `on_rejection: halt` (`src/run/policy.rs`,
  `tests/it/run_basics.rs`, `corpus/cases/builtins/validation.json`).
- **A `null` `map` result is skipped,** so `{"if": [c, v, null]}` means "set
  or keep"; removal is `unset: true` or `on_null: "unset"`. `mode: "append"`
  and `"extend"` stay O(entry): one `Change` per element at `path.<index>`,
  never the whole array. A literal path outside the three roots is
  `INVALID_PATH`; a root mapping merges; a path field given a non-string is
  `INPUT_PARSE` (`src/function/builtins/map.rs`).
- **A fan-out isolates every call and folds in element order.** Each element
  runs over its own bindings and overlay; the fold applies overlays in element
  order and stops at the first element that fails the task or halts, so
  `max_concurrency` can never change results. `for_each` is refused on a
  built-in (`tests/it/fan_out.rs`, `results_are_identical_at_every_concurrency`).
- **A loop's slots are the engine's.** Per sweep the engine writes the counter
  (and `as`, `scratch`) to `temp_data`, checks `counter < max`, re-evaluates
  the condition and runs the task list; reaching `max` is normal completion; a
  `Halt` ends the whole loop. A body task writing the counter's path is
  overwritten at the next increment (`tests/it/run_loops.rs`).
- **The deadline and the token are checked** at every host call, fan-out
  element and sweep, and every 64th op while one is set; natively a host call
  also races them, so a run with a deadline needs a Tokio runtime with its time
  driver. Only the run's own limits end it `TimedOut` or `Cancelled`; a
  function's own `TimedOut`, a child run's included, ends it `Stopped` when its
  policies stop the run, and a host call that fails at or after the run's limit
  is the limit's (`tests/it/deadline.rs`).
- **A root takes only an object.** `Ctx::set`, `emit` and a replayed overlay
  assign; at `data`, `metadata` or `temp_data` anything but an object is a
  `Config` error. `map` merges an object at a root; a constant non-object
  `logic` at a literal root is `INVALID_MAPPING`, any other non-object result
  fails the mapping (`tests/it/host_calls.rs`, `src/function/builtins/map.rs`).
- **An array index stops at `MAX_ARRAY_INDEX`** (2^20 − 1): a larger bare
  numeric segment is a key, so no path from a trace or a computed value can grow
  an array without bound (`src/expr/path.rs`, the wasm `contexts` test).
- **Error codes come from one classifier,** `Error::code`: `LOGIC_ERROR`,
  `BUDGET_EXCEEDED`, `TIMEOUT_ERROR`, `CANCELLED`, `CONFIG_ERROR`, `COMPILATION_ERROR`, and a
  `Service` code verbatim (an empty one is `TASK_ERROR`). The record codes
  `TASK_STATUS_ERROR`, `VALIDATION_ERROR`, `EVALUATION_ERROR` and
  `MAPPING_ERROR` are the recorder's and the built-ins': a failed `map`
  mapping records `MAPPING_ERROR` at its destination before the recorder's
  `TASK_STATUS_ERROR`, and a built-in over the ops budget fails its task.
  There is no `WORKFLOW_ERROR` wrapper (`src/base/error.rs`'s unit tests;
  `tests/it/run_basics.rs`, `src/function/builtins/map.rs`, `tests/it/ops_budget.rs`).
- **Reserved names cannot be registered.** `Registry::register` panics on a
  name in `def::BUILTIN_FUNCTIONS` and on `validate`, 3.x's alias the compat
  reader rewrites to `validation`; and on a malformed spec (a `Reference` that
  reaches no field, `runtime_fields` on a config that is not `Templated`)
  (`tests/it/registry.rs`).
- **Every config parameter is JSONLogic, and the static spelling is free.** A
  literal folds at compile and costs nothing per call; `Expr::constant`
  borrows it. Resolution borrows: `Ctx::eval` lends a folded constant and
  `Ctx::render` builds no tree (`tests/it/expr.rs`, `tests/it/documents.rs`).
- **The template-key escape (`$`) is always on:** `{"$cat": …}` is the literal
  object, `{"cat": …}` the operator; exactly one prefix is stripped from every
  template key, so a template emitting `$`-prefixed keys doubles them. A
  single-key object with an unknown key is not inert: it is the ordinary
  output-template shape, which is why there is no "unknown operator" lint
  (the escape in `tests/it/checks.rs`; the shape in `src/expr/engine.rs`'s
  `an_unrecognised_operator_is_not_an_error_under_templating`).
- **Operator families are opt-in at runtime, and enabling one is not a no-op.**
  The engine evaluates in templating mode, so an operator whose family is off is
  literal data; turning a family on (`ExprEngineBuilder::family`, `families`,
  `all_operators`) makes `{"length": …}` live, and the `datetime` cargo feature
  changes `==` on date-shaped strings. `OperatorFamily::Tensor` (behind the
  `tensor` feature) is deliberately outside `all_operators`: its names (`shape`,
  `cast`, `concat`) are ordinary JSON keys in workflows that already ship.
  `budget` is a config knob, not a family: `ExprEngineBuilder::ops_budget`
  installs the ceiling, exceeded as `ErrorKind::Budget` (`src/expr/engine.rs`'s
  gated-operator tests, `tests/it/operators.rs`, `tests/it/ops_budget.rs`).
- **Secrets are an operator, not a context root.** `{"secret": "name"}` reads
  the `ExprEngine`'s `Secrets`; nothing is composed into the message, a `map`
  mapping, a `log` expression and an `Output` may not read one, `Ctx::input()`
  writes every secret-reading field `null` and never evaluates it, and a
  context with a `secrets` key is refused (`tests/secrets_isolation.rs`).
- **The wire shapes read back.** `Message`, `Context` and `Trace` read what
  they write, byte for byte, a whole-number float staying a float (serde_json
  with `float_roundtrip`); a context holds only the three roots and refuses a
  fourth key, a non-object and a root twice, and a message's roots are
  objects (a trace's context takes a redacted root's `null`); the reader
  refuses an object holding a key twice, anywhere, and `parse_json` folds
  one to its first position and last value (`base::context`); `Run`
  serializes as `{status, error?, retryable, partial, matched, workflows}`.
  Key order is the source's everywhere: the crate turns on serde_json's
  `preserve_order`, so an `Interpolated` document (a Mongo `$sort`) renders
  as authored and issues come out in the definition's order, and pretty
  `publish_json`, `publish_xml` and `parse_xml` keep it through
  `OwnedDataValue` (`tests/it/wire.rs`, `tests/it/run_json.rs`,
  `tests/it/documents.rs`).
- **`Change::replaced` is the trace's alone;** an audit entry keeps v3's bytes.
  A trace records its own changes whatever `capture_changes` says
  (`tests/it/trace.rs`).
- **The error context is the engine's slot:** `ErrorContext::new(path, limit)`
  refuses a root, a path at or below an engine `metadata` slot, and a limit of
  0; the engine removes the key at run start and mirrors each failing call's
  records there (`tests/it/error_context.rs`).
- **The bindings are held by a drift test.** `bindings/dataflow.d.ts` is
  rendered from the wire types under `ts` (`DATAFLOW_TS_WRITE=1 cargo test
  --features ts --test ts_types`); a doc edit on a wire type means regenerating
  it, and a wire check holds every key a corpus message, trace and `Run`
  writes to its declaration (`tests/ts_types.rs`).
- **`def::parse` reports every unknown key.** A near miss of a known key is a
  `LIKELY_TYPO` Defect; any other unknown key an `UNKNOWN_KEY` Advisory, kept
  in the node's `ext`; a v3 policy key beside a v4 one is refused; with the
  reader off every unknown key is Rejected (`tests/it/parse.rs`, `tests/it/compat.rs`,
  `tests/it/repository_definitions.rs` over every definition in the repository).

## The wasm and UI packages

`wasm/` is a thin binding over `Engine::run_with` (`wasm/src/lib.rs`): one
`WasmEngine` with `new(workflows, options?)`, `run(payload, options?)`,
`check(definition)`, `workflow_count` and `workflow_ids`, and the free
functions `engine_version`, `engine_facts`, `parse`, `contexts(trace)` and
`mapping_contexts(trace, step)`, plus `TraceReplay` for on-demand replay.
JSON crosses the boundary as text through
`js-sys` (`JSON.stringify` in, `JSON.parse` out), so no serde-wasm crate is in
the lock and `null` stays `null`. `run` **resolves on every status** with
`{message, run, trace?}`; what the package throws is an `EngineError`, a JS
`Error` with a `code`: `REJECTED` with `issues` from the constructor,
`INVALID_ARGUMENT` from any export handed an argument that is not what it
takes, and `INVALID_TRACE` from the replay exports alone. The package's
`.d.ts` embeds `bindings/dataflow.d.ts` whole through a
`typescript_custom_section`, so the generated wire types reach the UI from
one package; `wasm/scripts/verify-wasm.mjs` checks the declarations are
there. **The replay lives once, in Rust** (`Replay`, `Trace::contexts`,
`Trace::mapping_contexts`): the UI calls the exports and keeps no TypeScript
copy of the rules. `wasm-pack test --node` in `wasm/` runs the package's
tests, the one run of v4 on wasm32.

`ui/` imports every wire type from `@goplasmatic/dataflow-wasm` and keeps the
authored definition shape as its own (`src/types/workflow.ts`, in v4
spellings with v3's still rendered). The debugger holds the trace, the run,
the message and one `TraceReplay` cursor, released when its run is cleared
or replaced. `context_at(i)` reads after step `i`, `context_at(-1)` before
step 0; detail views request only their selected mapping. No array of full
contexts is retained. A `failed` step and an executed
step with `errors` are errors; a step with `task: null` is the workflow's own
(its skip, its failure, or the engine's counter write). The engine's facts
(`engine_facts()`: the built-in names, the group depth, the loop defaults)
replace the UI's hand-kept mirrors, so `await init()` of the wasm package
comes before the UI reads them. The UI's checks are `npm ci`, `npm run
wasm:local`, `npm run lint`, `npm test` (vitest over the overlaid engine),
`npm run build:lib` and `npm run build`.
Both builds verify their artifacts: the library checks declarations and loads
ESM/CJS chunks, while the application checks its manifest keeps logic editors
and workflow diagrams outside the initial import graph. Those views use
React lazy loading and Suspense; their CSS stays in the public stylesheet.
The UI's `@goplasmatic/dataflow-wasm`
requirement stays at the last published version; `wasm:local` overlays this
checkout's build, and `release.yml` rewrites the requirement at publish.

## Testing

Unit tests live in `mod tests` blocks beside the code they cover; `src/state/`
has its model tests under `src/state/tests/`. The integration suite is one
binary, `tests/it/`, with a module per topic and the shared fixtures in
`tests/it/support/` (`golden` for every golden file, `run_helpers` for the
end-to-end runs), so a build links the crate once rather than once per
topic. Two files stay their own binaries: `tests/secrets_isolation.rs`
installs the process's logger and asserts over everything it logged, and
`tests/ts_types.rs` is named in the bindings' header and builds only under
`ts`. Run one topic with `cargo test --test it run_basics::`:

| File | Covers |
|---|---|
| `acceptance.rs` | Two acceptance configs: an Orion Mongo connector and a plugin's `Templated` |
| `checks.rs` | The compile checks: secrets, template keys, `payload` vars, outputs |
| `compat.rs` | The compat reader: every 3.x spelling, `migrate_v3`, mixed spellings refused |
| `compile.rs` | `Compiler::check`, `check_steps`, `compile_one`, the oracle |
| `config_fields.rs` | Every `ConfigField` type and the derive's attributes |
| `corpus.rs` | Every corpus case through the driver, held to `tests/golden/corpus/` |
| `deadline.rs` | The run's deadline and token, a child run's, a host call at the limit |
| `derive.rs` | The derive: nested configs, defaults, outputs, `resolve` |
| `documents.rs` | `Interpolated` rendering end to end |
| `error_context.rs` | The error context: the mirror, the reset, the refused paths |
| `expr.rs` | `Expr<T>` and `FromValue`: the conversions and their errors |
| `fan_out.rs` | `for_each`: isolation, element order, `null` on failure, halting, every concurrency |
| `fingerprint.rs` | `Fingerprint`: stable, scheme-tagged, the golden value |
| `function_testing.rs` | `function::testing::call` and `config` |
| `host_calls.rs` | Host functions end to end: the overlay, layers, roots, `Ctx` |
| `observer.rs` | `Observer`: interest, the events, timings |
| `operators.rs` | The operator table and the families' switches |
| `ops_budget.rs` | `ops_budget` and `BUDGET_EXCEEDED` (needs `budget`) |
| `parse.rs` | `def::parse`: every issue code at its path |
| `path.rs` | `Path` against v3's rules, by random paths |
| `program.rs` | `link`: order, numbering, channels, statuses |
| `registry.rs` | `Registry`: registration, reserved names, the catalogue |
| `repository_definitions.rs` | Every definition in the repository parses and compiles |
| `run_basics.rs`, `run_loops.rs` | The engine end to end: policies, groups, progress; loops |
| `run_json.rs` | `Run` as JSON |
| `trace.rs` | The trace: steps, changes, redaction, the budget, replay |
| `wire.rs` | Messages and traces read back byte for byte |
| `main.rs` | The modules, and the crate version |
| `../secrets_isolation.rs` | The never-recorded guarantee, exit by exit (its own binary) |
| `../ts_types.rs` | The bindings' drift and the wire check (its own binary, needs `ts`) |

A host tests one function with `function::testing::call(f, json!({..}))`,
which runs the real compile, overlay and layers with no engine assembled, and
`testing::config::<C>(json)` for a config alone.

## Release Process

Releases are **tag-gated**, not branch-gated. Pushing to `main` runs CI and
`docs.yml`; it never publishes crates or npm packages.
`.github/workflows/release.yml` fires only on a `v*` tag, validates that the
tag matches the workspace version and runs `scripts/check-versions.sh` (the
same agreement check CI runs on every PR), then publishes the derive, the crate,
the wasm package and the UI package, and cuts a GitHub release whose notes are
`CHANGELOG.md`'s section for the version (`validate` fails without one). The
derive goes first, under its own crates.io check: the root's published
manifest names it from crates.io at exactly the root's version, and a run
that failed between the two resumes at the root. A crate's first release
needs the `CRATES_IO_TOKEN` secret to carry the `publish-new` scope. `wasm/`
is `publish = false`: it ships through npm only.

`scripts/check-versions.sh` (run by CI on every PR and by `release.yml` against
the tag) checks that the workspace version, the internal crates' pins, `ui/`'s
versions and the install snippets' minor all agree, and (outside a `-dev`
version) that `CHANGELOG.md` has the version's section.

## The machine

The owner's Docker stack runs permanently and is never stopped, paused or
pruned; its services have held 127.0.0.1:8080, so nothing here binds 8080.
It is the machine's permanent load: judge benchmarks on paired ratios with the
instructions retired beside them. The repository's `target/` grows past 90 GB;
below 20 GiB free, delete `target/debug/incremental` (a cache that rebuilds)
between cargo runs, never during one, and check `df -h /` before a long build.
