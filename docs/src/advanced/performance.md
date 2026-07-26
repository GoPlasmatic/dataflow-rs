# Performance

Dataflow-rs is designed for high-performance rule evaluation and data processing with minimal overhead.

## Architecture for Performance

### Pre-compilation

All JSONLogic expressions are compiled once at engine startup:

```rust
# use dataflow_rs::{Engine, Message, Workflow};
# async fn _demo(workflows: Vec<Workflow>, mut message: Message)
#     -> dataflow_rs::Result<()> {
// Builder is the recommended construction path; compiles all
// JSONLogic at .build() and pre-parses Custom-task inputs into
// their typed Self::Input.
let engine = Engine::builder()
    .with_workflows(workflows)
    .build()?;

// Runtime processing uses pre-compiled logic — no parsing or
// compilation overhead.
engine.process_message(&mut message).await?;
# Ok(()) }
```

### Benefits of Pre-compilation

- **Zero runtime parsing** - No JSON parsing during message processing
- **Cached compiled logic** - O(1) access to compiled expressions
- **Early validation** - Invalid expressions caught at startup
- **Consistent latency** - Predictable performance per message

### Memory Efficiency

- **Arc-wrapped compiled logic** - Shared without copying
- **Immutable workflows** - Safe concurrent access
- **Context caching** - Avoids repeated JSON cloning

## Benchmarking

Run the included benchmarks:

```bash
cargo run --example benchmark --release             # Throughput + latency percentiles
cargo run --example realistic_benchmark --release   # ISO 20022 → SwiftMT-style workload
cargo run --example micro_aggregate_bench --release # Aggregate-heavy (reduce/map) mappings
```

### Microbenchmarks

The macro benchmarks above are dominated by Tokio scheduling and can't resolve a
sub-100ns/message change. The `micro_*` benchmarks run a tight loop on a
`current_thread` runtime instead, so the effect under test is a measurable
fraction of the total:

```bash
cargo run --example micro_cond_bench --release          # Condition eval, incl. trivially-true folding
cargo run --example micro_multiworkflow_bench --release # Chained workflows, per-workflow arena rebuild
cargo run --example micro_subtree_write_bench --release # k map writes into one subtree (write-path scaling)
```

Two more measure throughput on a multi-threaded runtime, so they carry the same
scheduling noise as the macro benchmarks:

```bash
cargo run --example async_handler_benchmark --release   # Marginal cost of one custom async handler
cargo run --example map_performance_test --release      # Sequential map mappings
```

Each source file documents what it isolates and why in its header comment.
Numbers vary ±2–3% run to run, so compare the mean of several runs rather than
single results.

### Sample Benchmark

```rust
# async fn _demo(workflow_json: &str, test_data: serde_json::Value)
#     -> dataflow_rs::Result<()> {
use dataflow_rs::{Engine, Workflow, Message};
use std::time::Instant;

// Setup
let workflow = Workflow::from_json(workflow_json)?;
let engine = Engine::builder().with_workflow(workflow).build()?;

// Benchmark
let iterations = 10_000;
let start = Instant::now();

for _ in 0..iterations {
    let mut message = Message::from_value(&test_data);
    engine.process_message(&mut message).await?;
}

let elapsed = start.elapsed();
println!("Processed {} messages in {:?}", iterations, elapsed);
println!("Average: {:?} per message", elapsed / iterations);
# Ok(()) }
```

## Optimization Tips

### 1. Minimize Mappings

Combine related transformations:

```json
// Less efficient: Multiple mappings
{
    "mappings": [
        {"path": "data.a", "logic": {"var": "data.source.a"}},
        {"path": "data.b", "logic": {"var": "data.source.b"}},
        {"path": "data.c", "logic": {"var": "data.source.c"}}
    ]
}

// More efficient: Single object mapping when possible
{
    "mappings": [
        {"path": "data", "logic": {"var": "data.source"}}
    ]
}
```

### 2. Use Conditions Wisely

Skip unnecessary processing with conditions:

```json
{
    "id": "expensive_task",
    "condition": {"==": [{"var": "metadata.needs_processing"}, true]},
    "function": { ... }
}
```

### 3. Order Rules by Frequency

Put frequently-executed rules earlier (lower priority):

```json
{"id": "common_rule", "priority": 1, ...}
{"id": "rare_rule", "priority": 100, ...}
```

### 4. Use temp_data

Store intermediate results to avoid recomputation:

```json
{
    "mappings": [
        {
            "path": "temp_data.computed",
            "logic": {"expensive": "computation"}
        },
        {
            "path": "data.result1",
            "logic": {"var": "temp_data.computed"}
        },
        {
            "path": "data.result2",
            "logic": {"var": "temp_data.computed"}
        }
    ]
}
```

> **Note:** since datalogic-rs 5.1, repeated pure subexpressions *within a
> single mapping's logic* are evaluated once automatically
> (common-subexpression elimination), and `reduce` over `map` is fused.
> `temp_data` staging still pays off when the same result is reused across
> different mappings or tasks.

### 5. Avoid Unnecessary Validation

Validate only what's necessary:

```json
// Validate at system boundaries
{
    "id": "input_validation",
    "condition": {"==": [{"var": "metadata.source"}, "external"]},
    "tasks": [
        {"id": "validate", "function": {"name": "validation", ...}}
    ]
}
```

### 6. Disable Change Capture When Unused

When change capture is on (the default), every mapping snapshots the old and
new value into the audit trail — deep copies that dominate the profile in
mapping-heavy workloads. If you never read `message.audit_trail()`, turn it
off per message:

```rust
# use dataflow_rs::Message;
# fn _demo(payload: serde_json::Value) {
let mut message = Message::builder()
    .payload_json(&payload)
    .capture_changes(false)
    .build();
# }
```

This is the single largest tuning lever in the hot path. See
[Audit Trails](audit-trails.md) for what you give up.

### 7. Filtered Log Tasks Are Free

`log` tasks check whether their level is enabled for the `dataflow::log`
target *before* evaluating any JSONLogic or formatting fields. With
production filtering like `RUST_LOG=dataflow::log=warn`, `debug`/`info` log
tasks short-circuit at near-zero cost — you can leave diagnostic logging in
production workflows without paying for it.

## Concurrent Processing

Process multiple messages concurrently:

```rust
# use dataflow_rs::{Engine, Message, Workflow};
# async fn _demo(workflows: Vec<Workflow>, messages: Vec<Message>)
#     -> std::result::Result<(), Box<dyn std::error::Error>> {
use std::sync::Arc;
use tokio::task;

let engine = Arc::new(Engine::builder().with_workflows(workflows).build()?);

let handles: Vec<_> = messages.into_iter()
    .map(|mut msg| {
        let engine = Arc::clone(&engine);
        task::spawn(async move {
            engine.process_message(&mut msg).await
        })
    })
    .collect();

// Wait for all
for handle in handles {
    handle.await??;
}
# Ok(()) }
```

### Thread Safety

- Engine is `Send + Sync`
- Compiled logic shared via `Arc`
- Each message processed independently

## Memory Considerations

### Large Messages

For very large messages, consider:

1. **Streaming** - Process chunks instead of entire payload
2. **Selective Loading** - Load only needed fields
3. **Cleanup temp_data** - Clear intermediate results when done

### Many Rules

For many rules:

1. **Organize by Domain** - Group related rules
2. **Use Conditions** - Skip irrelevant rules early
3. **Profile** - Identify bottleneck rules

## Profiling

### Enable Logging

```rust
env_logger::Builder::from_env(
    env_logger::Env::default().default_filter_or("debug")
).init();
```

### Custom Metrics

```rust,ignore
use std::time::Instant;

let start = Instant::now();
engine.process_message(&mut message).await?;
let duration = start.elapsed();

metrics::histogram!("dataflow.processing_time", duration);
```

## Production Recommendations

1. **Build with --release** - Debug builds are significantly slower
2. **Pre-warm** - Process a few messages at startup to warm caches
3. **Monitor** - Track processing times and error rates
4. **Profile** - Identify slow rules in production
5. **Scale Horizontally** - Engine is stateless, scale with instances
