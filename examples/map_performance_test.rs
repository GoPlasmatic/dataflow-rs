//! # Sequential map-chain throughput
//!
//! Ten `map` mappings in one task, each reading the value the previous one
//! wrote, across `data` / `temp_data` / `metadata`. Measures per-message cost
//! when every mapping resolves and writes.
//!
//! This is v4's port of v3's `map_performance_test`: the same definition,
//! iteration counts and loop, run through dataflow-rs 4.
//!
//! Run with: `cargo run --example map_performance_test --release`

use dataflow_rs::{Engine, Message};
use serde_json::{Value, json};
use std::time::Instant;

#[path = "shared/gate.rs"]
mod gate;

#[tokio::main]
async fn main() {
    // Create a workflow with multiple sequential mappings
    let workflows_json = json!([
        {
            "id": "perf_test",
            "name": "Performance Test",
            "priority": 1,
            "condition": true,
            "tasks": [
                {
                    // Without this the mappings below read nothing: the eval
                    // context is `{data, metadata, temp_data}` and `payload`
                    // is not part of it, so `{"var": "data.source"}` would
                    // resolve to missing and every mapping would be a no-op.
                    "id": "load",
                    "name": "Load payload into data",
                    "function": {
                        "name": "parse_json",
                        "input": { "source": "payload", "target": "input" }
                    }
                },
                {
                    "id": "multi_map",
                    "name": "Multiple Sequential Mappings",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {"path": "data.field1", "logic": {"var": "data.input.source"}},
                                {"path": "data.field2", "logic": {"var": "data.field1"}},
                                {"path": "data.field3", "logic": {"var": "data.field2"}},
                                {"path": "data.field4", "logic": {"var": "data.field3"}},
                                {"path": "data.field5", "logic": {"var": "data.field4"}},
                                {"path": "temp_data.temp1", "logic": {"var": "data.field5"}},
                                {"path": "temp_data.temp2", "logic": {"var": "temp_data.temp1"}},
                                {"path": "data.result", "logic": {"var": "temp_data.temp2"}},
                                {"path": "metadata.meta1", "logic": {"var": "data.result"}},
                                {"path": "metadata.meta2", "logic": {"var": "metadata.meta1"}}
                            ]
                        }
                    }
                }
            ]
        }
    ]);

    let engine = workflows_json
        .as_array()
        .unwrap()
        .iter()
        .fold(Engine::builder(), |b, w| b.workflow_json(w))
        .build()
        .expect("the engine builds");

    // Warmup
    println!("Warming up...");
    for _ in 0..gate::count(1000, 10) {
        let mut message = Message::from_json(&json!({
            "source": "test_value"
        }));
        engine.run(&mut message).await.into_result().unwrap();

        // Fail loudly rather than reporting a throughput number for ten
        // mappings that resolved to nothing. `data.result` is the end of the
        // chain, so it only holds the seed value if all ten fired.
        assert_eq!(
            Value::from(message.context().data())["result"],
            json!("test_value"),
            "the mapping chain did not resolve — this benchmark would be measuring no-ops"
        );
    }

    // Benchmark
    let iterations = gate::count(100_000, 100);
    println!(
        "Running {} iterations with 10 sequential mappings each...",
        iterations
    );

    let start = Instant::now();
    for _ in 0..iterations {
        let mut message = Message::from_json(&json!({
            "source": "test_value"
        }));
        engine.run(&mut message).await.into_result().unwrap();
    }
    let duration = start.elapsed();

    let throughput = iterations as f64 / duration.as_secs_f64();
    let avg_time = duration.as_micros() as f64 / iterations as f64;

    println!("\nPerformance Results:");
    println!("====================");
    println!("Total iterations: {}", iterations);
    println!("Total time: {:?}", duration);
    println!("Throughput: {:.0} messages/second", throughput);
    println!("Average time per message: {:.2} μs", avg_time);
    println!(
        "Total mappings processed: {} (10 per message)",
        iterations * 10
    );
    println!(
        "Mapping throughput: {:.0} mappings/second",
        throughput * 10.0
    );

    gate::Gate::new("map_performance_test")
        .higher("throughput", throughput, "msg/s")
        .lower(
            "mean",
            duration.as_secs_f64() * 1e6 / iterations as f64,
            "µs",
        )
        .emit();
}
