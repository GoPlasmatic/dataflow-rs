# Performance

Dataflow-rs keeps per-message overhead low by moving every decision it can
to the moment a workflow is compiled, and by touching the message's context
as little as possible while a run is under way.

## Architecture for performance

### Everything compiles at build

A `Compiler` turns a definition into ops once. Every JSONLogic expression
(conditions, `map` mappings, every field of every function's input) is
compiled then, and an expression that reads nothing is evaluated once and
kept as a constant, so a literal parameter costs nothing per message. Every
function's config is compiled into its typed struct then too, so no JSON is
parsed, no key looked up and no type checked while a message runs. A
malformed definition is an issue at build, never a message-time error.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::{Engine, Message};
use serde_json::json;

// Compiles every expression and config here, once.
let engine = Engine::builder()
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "t", "function": {"name": "map", "input": {"mappings": [
            {"path": "data.total", "logic": {"*": [{"var": "data.price"}, {"var": "data.qty"}]}}
        ]}}}
    ]}))
    .build()
    .unwrap();

// Runs the compiled ops: no parsing, no compilation, no config lookup.
let mut message = Message::from_json(&json!({}));
let run = engine.run(&mut message).await;
assert_eq!(run.matched(), 1);
# });
```

### One view of the context per run

The expression evaluator reads an arena-allocated view of the context, with
the message's strings and keys borrowed and only its arrays' and objects'
spines built. A run builds that view once and keeps it for its whole length:
every write, a `map`'s or a host function's, is spliced into the view where
it lands, and a checkpoint reclaims the arena's garbage when it is due. A
task, a sweep of a loop and a host call all read the same view. Nothing
converts the context per task, so a run's cost tracks what it reads and
writes, not the bytes the message carries.

### Host calls read the view and write an overlay

A host function's `Ctx` reads the run's view directly; no view is built per
call. Its writes go to an overlay of the call's own, which the call reads
back at once and which the engine replays into the run, write by write,
once the call returns `Ok`. A fan-out binds each element beneath its call's
overlay over the same view, so 100 elements over a 2 MiB context copy
nothing. An `Interpolated` document renders by lending each hole's value
where it already is and serializing the skeleton around it, building no
tree.

### The recorder pays for what is watched

After every call the engine records the audit entry, the progress write and
the error record. Anything beyond that is paid for only when something asks:
no clock is read unless an observer asked for `Interest::TIMINGS` or a trace
for timings, no trace step is built without a trace, no change is captured
when neither the audit trail nor a trace will keep it, and a run with no
observer, trace or error context takes the plain recorder, which costs the
watched machinery one branch per call.

## Benchmarking

The crate has thirteen benchmark examples. Always run them in release; a
debug build measures nothing useful:

```bash
cargo run --example realistic_benchmark --release        # ISO 20022 -> SwiftMT-style workload
cargo run --example benchmark --release                  # Throughput + latency percentiles
cargo run --example micro_cond_bench --release           # Condition eval, trivially-true folding
cargo run --example micro_aggregate_bench --release      # Aggregate-heavy (reduce/map) mappings
cargo run --example micro_subtree_write_bench --release  # k map writes into one depth-2 subtree
cargo run --example micro_multiworkflow_bench --release  # N chained workflows, one message
cargo run --example async_handler_benchmark --release    # Marginal cost of one host function call
cargo run --example map_performance_test --release       # 10 chained mappings across all 3 roots
cargo run --example loop_scaling --release               # 1000-sweep append loop over a large context
cargo run --example large_context_handler --release      # Five expression fields over a 2 MiB context, per call
cargo run --example fan_out --release                    # for_each over 100 elements on a 2 MiB context
cargo run --example orion_connector --release            # A connector-shaped workload: documents, output, a layer
cargo run --example observed_run --release               # The realistic workload with a counting observer
```

Each source file documents what it isolates and why in its header comment,
and **each checks its result before it times anything**: a workload that
silently does less than it says still reports a plausible number.

The macro benchmarks run on a multi-threaded runtime and are dominated by
scheduling, so they cannot resolve a sub-100 ns change per message. The
`micro_*` benchmarks run a tight loop on a `current_thread` runtime instead,
so the effect under test is a measurable fraction of the total.
`micro_subtree_write_bench` and `micro_multiworkflow_bench` are regression
guards: they assert that a property stays flat (per-write cost roughly linear
in `k`; the three workflow layouts close together), and a sweep bending
upward is the signal.

Numbers vary by 2 to 3% run to run, with occasional transient P99 spikes.
Compare the mean of several runs rather than single results.

### The gate's method

**Interleave the two sides of a comparison.** Running every "before"
measurement and then every "after" one conflates the change with thermal
drift: a measured 8.5% improvement on `realistic_benchmark` collapsed to
0.2% once the same two binaries were alternated round-robin instead.

`bench/gate.py` does this mechanically, comparing two builds: a base commit
(A) against a candidate (B). Every gate benchmark prints one
machine-readable line, `GATE {...}`, and the script reads only that:

```bash
git switch --detach main && python3 bench/gate.py build --out /tmp/gate/base
git switch my-branch     && python3 bench/gate.py build --out /tmp/gate/candidate
python3 bench/gate.py compare --a /tmp/gate/base --b /tmp/gate/candidate --rounds 5
```

`build` builds the examples in release from a clean tree at `HEAD` and
copies the thirteen binaries out of `target/`, so a rebuild cannot clobber
one. `compare` runs
the two builds round-robin for the rounds asked plus one, alternating the
order within each pair, discards the first round as cold, and judges each
metric on the **median of its per-round ratios**, B over A, oriented so a
ratio above 1 means B is worse. The noise is the larger of 3% and half the
inter-quartile range of those ratios: B is WORSE above `1 + noise`, better
below `1 - noise`, and "not slower" between. It also reports the
instructions retired by each binary, which separate the work the code does
from the load on the machine; they move with code layout too, so attribute
a change to one item only with a same-binary toggle.

### What 4.0 measured against 3.15

Before 4.0's release, against 3.15, five rounds after a cold one, on a machine
with a busy container stack: 16 rows faster, 25 better, 6 not slower and 2
WORSE of 49 judged rows. The two WORSE rows were
`micro_subtree_write_bench`'s `k=100` (1.039) and `scaling` (1.104), with
instructions retired at 0.996; at seven rounds they read 1.006 and 1.026,
not slower. Where 3.15 paid per byte the message carried, 4.0 pays per
write:

| Benchmark | Metric | 4.0 over 3.15 |
|---|---|---|
| `realistic_benchmark` | `mean` latency | 0.76 |
| `realistic_benchmark` | `p99` latency | 0.27 |
| `benchmark` | `mean` latency | 0.51 |
| `micro_multiworkflow_bench` | `grouped.ns_per_msg` | 0.86 |
| `loop_scaling` | every row, 250 and 1,000 sweeps | 0.01 to 0.03 |
| `large_context_handler` | `per_field.mean` | 0.001 |
| `fan_out` | `per_element.mean` | 0.002 |
| `orion_connector` | `connector.mean` | 0.45 |

`loop_scaling` at about 2% of 3.15's time, `fan_out` and
`large_context_handler` at under 1%, are the one view and the overlay: 3.15
converted the whole context into the arena once per sync stretch and once
per fan-out element, and 4.0 does neither.

## Optimization tips

### 1. Let literals fold

A parameter written as a literal is compiled once and never evaluated. Only
a parameter that reads the message does per-message work, so prefer
`"timeout_ms": 5000` to an expression that computes the same constant.

### 2. Use conditions

Skip work with conditions, on a task, a group or the workflow. A condition
that is literally `true` compiles to nothing at all.

```json
{
    "id": "expensive_task",
    "condition": {"==": [{"var": "metadata.needs_processing"}, true]},
    "function": { "name": "map", "input": { "mappings": [] } }
}
```

### 3. Stage intermediate results in `temp_data`

Repeated pure subexpressions *within one mapping* are evaluated once by the
evaluator. Staging in `temp_data` still pays off when the same result is
reused across different mappings or tasks:

```json
{
    "mappings": [
        {"path": "temp_data.computed", "logic": {"var": "data.source.expensive"}},
        {"path": "data.result1", "logic": {"var": "temp_data.computed"}},
        {"path": "data.result2", "logic": {"var": "temp_data.computed"}}
    ]
}
```

### 4. Turn change capture off when nothing reads it

With change capture on (the default), every write copies its old and new
value into the audit entry: deep copies that dominate the profile in
mapping-heavy workloads and grow with every sweep of a loop. If you never
read `AuditEntry::changes`, turn it off per run, and drop the trail
altogether when nothing reads the entries either:

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

This is the single largest tuning lever in the hot path. See
[Audit Trails](audit-trails.md) for what you give up, and
[Memory in long loops](loops.md#memory-in-long-loops) for why it matters
more in a loop.

### 5. Observe rather than trace

A trace records every change of every step and copies the initial context;
an observer copies out what it keeps and nothing else. For counters and
timings, attach an `Observer`; ask for a trace when you need the context
after a step. Neither reads the clock unless asked for timings.

### 6. Filtered log tasks are free

`log` tasks check whether their level is enabled for the `dataflow::log`
target *before* evaluating any JSONLogic or formatting fields. With
production filtering like `RUST_LOG=dataflow::log=warn`, `debug` and `info`
log tasks short-circuit at near-zero cost, so you can leave diagnostic
logging in production workflows without paying for it.

### 7. Set a deadline only when you need one

A run with a deadline or a cancellation token checks it at every host call,
fan-out element and sweep, and every 64th op, and natively races each host
call against it on a Tokio timer. A run without either skips all of that.

## Concurrent processing

`Engine` is `Clone`, `Send` and `Sync`, and a clone runs the same program.
Process messages concurrently by spawning one task per message:

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

Each run owns its message and its arena; nothing is shared between runs but
the compiled program, behind an `Arc`. A reload (`Engine::with_program`)
gives a new engine running the new program while runs in flight on the old
one finish undisturbed.

## Memory

A run's arena holds the view's spines and each write's spliced value, and a
checkpoint reclaims what the splices left behind, so the arena stays within
a small multiple of the live view however long a loop runs. The context's
strings and keys are borrowed from the message, not copied.

The audit trail is the other half of a run's memory, and it is the host's
choice: see tip 4. A trace adds a copy of the initial context and of every
change, bounded by `TraceOptions::max_bytes` when a host asks.

## Production recommendations

1. **Build with `--release`.** Debug builds are many times slower.
2. **Compile once, run many.** Build the engine at startup; a definition
   that changes goes through `engine.compiler()` and `with_program`, not a
   rebuild per message.
3. **Monitor with an observer.** Counters and timings per task and per
   workflow, with `Interest::TIMINGS` only where you read them.
4. **Profile with the micro benchmarks** before changing the engine, and
   compare interleaved.
5. **Scale horizontally.** The engine holds no state between runs.
