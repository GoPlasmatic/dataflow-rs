# Performance

Dataflow-rs is designed for high-performance rule evaluation and data processing with minimal overhead.

## Architecture for Performance

### Pre-compilation

All JSONLogic expressions are compiled once at engine startup:

```rust
// This compiles all logic at creation time
let engine = Engine::new(workflows, None);

// Runtime processing uses pre-compiled logic
// No parsing or compilation overhead
engine.process_message(&mut message).await?;
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

Run the included benchmark:

```bash
cargo run --example benchmark --release
```

### Sample Benchmark

```rust
use dataflow_rs::{Engine, Workflow, Message};
use std::time::Instant;

// Setup
let workflow = Workflow::from_json(workflow_json)?;
let engine = Engine::new(vec![workflow], None);

// Benchmark
let iterations = 10_000;
let start = Instant::now();

for _ in 0..iterations {
    let mut message = Message::new(&test_data);
    engine.process_message(&mut message).await?;
}

let elapsed = start.elapsed();
println!("Processed {} messages in {:?}", iterations, elapsed);
println!("Average: {:?} per message", elapsed / iterations);
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

## Concurrent Processing

Process multiple messages concurrently:

```rust
use std::sync::Arc;
use tokio::task;

let engine = Arc::new(Engine::new(workflows, None));

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

```rust
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
