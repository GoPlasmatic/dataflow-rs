# Performance

Dataflow-rs minimizes per-message latency and allocations by moving parsing,
type-checking, and expression compilation to build time, and by avoiding
unnecessary copies of the message context at runtime.

## Architectural Design

### 1. Ahead-of-Time Compilation

When you build a workflow with `Compiler` (or `Engine::builder().build()`), the
engine compiles the workflow definition into a flat sequence of execution
operations:

- Every JSONLogic expression—including conditions, `map` transformations, and
  function input fields—is compiled once at build time.
- Expressions that do not read from the message context or secrets are
  constant-folded at build time and stored as ready-made constants, so literal
  parameters have zero per-message evaluation cost.
- Every function's `input` is compiled into its typed `FunctionConfig` struct at
  build time, eliminating JSON parsing, key lookups, and type validation while
  processing messages.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::{Engine, Message};
use serde_json::json;

// Compiles all expressions and function configs once at build time:
let engine = Engine::builder()
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "t", "function": {"name": "map", "input": {"mappings": [
            {"path": "data.total", "logic": {"*": [{"var": "data.price"}, {"var": "data.qty"}]}}
        ]}}}
    ]}))
    .build()
    .unwrap();

// Executes pre-compiled operations with no runtime JSON parsing or compilation:
let mut message = Message::from_json(&json!({}));
let run = engine.run(&mut message).await;
assert_eq!(run.matched(), 1);
# });
```

### 2. Single Shared Context View per Run

The JSONLogic evaluator operates over an arena-allocated view of the message
context that borrows string and key data directly from the `Message` and only
allocates lightweight index spines for arrays and objects.

The engine constructs this view once at the start of a run and maintains it
across all workflows, tasks, and loop iterations:
- Writes from `map` tasks and custom host functions are spliced directly into
  the active view at the modified path.
- Periodic arena checkpoints reclaim memory from overwritten subtrees during
  long runs.
- Because the context is never re-converted between tasks or loop iterations,
  runtime cost scales with the data read and written rather than the total size
  of the message.

### 3. Zero-Copy Host Calls and Overlays

When invoking a custom host function:
- `Ctx` reads directly from the run's shared context view without building a new
  view per call.
- Writes made during the call are buffered in a lightweight per-call overlay that
  subsequent reads in the same call see immediately, and are committed to the
  message only if the call returns `Ok`.
- In a `for_each` fan-out, each element call binds its element and index in its
  own overlay on top of the shared context view—so fanning out across 100
  elements on a 2 MiB context performs zero full-context copies.
- `Interpolated` documents render by borrowing placeholder values in place and
  serializing the surrounding template structure directly without allocating an
  intermediate JSON tree.

### 4. Pay-Only-for-What-You-Use Observability

During normal execution, the engine only records what is enabled:
- No system clock readings are taken per task unless an `Observer` requests
  `Interest::TIMINGS` or `TraceOptions::timings(true)` is enabled.
- No `TraceStep` records are built unless tracing is requested on `RunOptions`.
- No `Change` snapshots (cloning old and new values) are allocated when both
  change capture (`RunOptions::capture_changes(false)`) and tracing are off.
- Runs without an observer, trace, or `ErrorContext` use a streamlined fast-path
  recorder.

## Running Benchmarks

The repository includes thirteen benchmark examples covering macro workloads and
targeted micro-benchmarks. Always run benchmarks with `--release`:

```bash
cargo run --example realistic_benchmark --release        # ISO 20022 -> SwiftMT-style pipeline
cargo run --example benchmark --release                  # Multi-threaded throughput and latency percentiles
cargo run --example micro_cond_bench --release           # Condition evaluation and constant folding
cargo run --example micro_aggregate_bench --release      # Aggregate-heavy (reduce/map) mappings
cargo run --example micro_subtree_write_bench --release  # k map writes into a depth-2 subtree
cargo run --example micro_multiworkflow_bench --release  # N chained workflows over a single message
cargo run --example async_handler_benchmark --release    # Marginal cost of a custom host function call
cargo run --example map_performance_test --release       # 10 chained mappings across all 3 context roots
cargo run --example loop_scaling --release               # 1,000-iteration append loop over a large context
cargo run --example large_context_handler --release      # Evaluating 5 expression fields over a 2 MiB context
cargo run --example fan_out --release                    # for_each over 100 elements on a 2 MiB context
cargo run --example orion_connector --release            # Connector workload: interpolated docs, outputs, and layers
cargo run --example observed_run --release               # Realistic workload with a counting observer attached
```

Each benchmark verifies its output before timing begins. The macro benchmarks
run on a multi-threaded Tokio runtime, while the `micro_*` benchmarks run a
tight loop on a `current_thread` runtime to isolate sub-microsecond engine
changes from thread-scheduling noise.

### Comparing Changes with `bench/gate.py`

Running all "before" benchmarks followed by all "after" benchmarks can easily
confound code changes with CPU thermal throttling. Instead, interleave runs
round-robin using `bench/gate.py`:

```bash
git switch --detach main && python3 bench/gate.py build --out /tmp/gate/base
git switch my-branch     && python3 bench/gate.py build --out /tmp/gate/candidate
python3 bench/gate.py compare --a /tmp/gate/base --b /tmp/gate/candidate --rounds 5
```

- `build` compiles all thirteen benchmark binaries in `--release` mode and copies
  them to the output directory.
- `compare` runs the base (`A`) and candidate (`B`) binaries round-robin (with a
  warmup round discarded first) and compares the median per-round ratio (`B / A`),
  where values below `1.0` mean `B` is faster.

### 4.0 vs. 3.15 Benchmark Highlights

Because 4.0 maintains a single context view and splices writes in place rather
than re-converting the entire context for each synchronous task stretch or
`for_each` element, workloads with large contexts or loops see dramatic speedups
compared to 3.15:

| Benchmark | Metric | 4.0 / 3.15 Ratio |
|---|---|---|
| `realistic_benchmark` | `mean` latency | `0.76` (24% faster) |
| `realistic_benchmark` | `p99` latency | `0.27` (3.7x faster) |
| `benchmark` | `mean` latency | `0.51` (2x faster) |
| `micro_multiworkflow_bench` | `grouped.ns_per_msg` | `0.86` (14% faster) |
| `loop_scaling` | 250 and 1,000 iterations | `0.01`–`0.03` (30x–100x faster) |
| `large_context_handler` | `per_field.mean` | `0.001` (~1,000x faster) |
| `fan_out` | `per_element.mean` | `0.002` (~500x faster) |
| `orion_connector` | `connector.mean` | `0.45` (2.2x faster) |

## Optimization Tips

### 1. Prefer Literal Parameters When Values Are Static

Any config field or expression that does not read from the message context is
constant-folded at compile time and never evaluated per message. Prefer plain
literals like `"timeout_ms": 5000` over dynamic expressions whenever a value is
fixed.

### 2. Gate Expensive Steps with Conditions

Use workflow, group, and task `condition` fields to skip unnecessary work early.
A literal `true` condition compiles to a no-op with zero runtime cost.

```json
{
    "id": "expensive_task",
    "condition": {"==": [{"var": "metadata.needs_processing"}, true]},
    "function": { "name": "map", "input": { "mappings": [] } }
}
```

### 3. Stage Shared Intermediate Values in `temp_data`

While the expression evaluator automatically deduplicates repeated pure
subexpressions *within a single mapping*, staging an expensive calculation in
`temp_data` avoids recomputing it across multiple mappings or tasks:

```json
{
    "mappings": [
        {"path": "temp_data.computed", "logic": {"var": "data.source.expensive"}},
        {"path": "data.result1", "logic": {"var": "temp_data.computed"}},
        {"path": "data.result2", "logic": {"var": "temp_data.computed"}}
    ]
}
```

### 4. Disable `capture_changes` When Old/New Snapshots Aren't Needed

When `capture_changes` is enabled (the default), every context write clones both
the old and new value into `AuditEntry::changes`. In mapping-heavy pipelines and
loops, those deep clones are often the largest part of per-message cost.

If your application does not inspect `AuditEntry::changes`, disable change
capture (and optionally disable the audit trail altogether if you don't read
`message.audit()`):

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::run::AuditMode;
use dataflow_rs::{Engine, Message, RunOptions};
use serde_json::json;
# let engine = Engine::builder()
#     .workflow_json(&json!({"id": "w", "tasks": [
#         {"id": "t", "function": {"name": "map", "input": {"mappings": [{"path": "data.a", "logic": 1}]}}}
#     ]}))
#     .build()
#     .unwrap();

let lean = RunOptions::default()
    .capture_changes(false)
    .audit(AuditMode::Off);
let mut message = Message::from_json(&json!({}));
engine.run_with(&mut message, &lean).await;
assert!(message.audit().is_empty());
# });
```

See [Audit Trails](./audit-trails.md) and
[Memory in Long Loops](./loops.md#memory-in-long-loops) for details.

### 5. Use `Observer` for Production Metrics Instead of Full Traces

An execution `Trace` clones the initial context and records every step's changes
for offline replay, whereas an `Observer` runs inline with minimal overhead. Use
`Observer` for production counters, histograms, and timings, and reserve
`TraceOptions` for debugging and diagnostics.

### 6. Leave Diagnostic `log` Tasks in Workflows

`log` tasks check `tracing::enabled!` for the `dataflow::log` target *before*
evaluating their `message` or `fields` expressions. When `RUST_LOG=dataflow::log=warn`
is set in production, `debug` and `info` log tasks short-circuit immediately at
near-zero cost.

### 7. Only Set Deadlines When Needed

Configuring a `deadline` or `cancel` token on `RunOptions` adds periodic check
points and wraps host function calls in a timer race. Omit them on runs that do
not require per-run timeouts or cancellation.

## Concurrent Message Processing

`Engine` is `Clone`, `Send`, and `Sync`. Cloning an `Engine` simply clones an
`Arc` to the compiled `Program`, so you can process messages concurrently across
Tokio tasks with zero lock contention:

```rust
# tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async {
use dataflow_rs::{Engine, Message};
use serde_json::json;

let engine = Engine::builder()
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "t", "function": {"name": "map", "input": {"mappings": [
            {"path": "data.seen", "logic": true}]}}}
    ]}))
    .build()
    .unwrap();

let handles: Vec<_> = (0..8)
    .map(|i| {
        let engine = engine.clone();
        tokio::spawn(async move {
            let mut message = Message::from_json(&json!({"i": i}));
            engine.run(&mut message).await.matched()
        })
    })
    .collect();

for handle in handles {
    assert_eq!(handle.await.unwrap(), 1);
}
# });
```

Each `engine.run` call manages its own message and evaluation arena. When hot-reloading
workflows via `engine.with_program(new_program)`, the new `Engine` serves new
runs while any in-flight runs on clones of the previous `Engine` complete
undisturbed.

## Production Checklist

1. **Always compile with `--release`.**
2. **Build `Engine` once and reuse it:** Use `engine.compiler()` and
   `engine.with_program()` for hot reloads rather than rebuilding from scratch
   per message.
3. **Disable `capture_changes` (or `AuditMode`)** on high-throughput pipelines
   that don't read audit change diffs.
4. **Use `Observer` for telemetry,** enabling `Interest::TIMINGS` only when you
   record task durations.
5. **Benchmark changes with interleaved runs** via `bench/gate.py`.
