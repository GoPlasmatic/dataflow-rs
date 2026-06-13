//! # Single-threaded process_message microbenchmark
//!
//! Isolates the per-message CPU cost of `process_message` with **no tokio
//! task-spawn overhead** — the macro benchmarks (`benchmark.rs`) are dominated
//! by Tokio scheduling and can't resolve a sub-100ns/message change. This runs
//! a tight single-threaded loop so the signal of interest (condition-eval
//! work) is a measurable fraction of the total.
//!
//! Uses the same workflow shape as `benchmark.rs`: one workflow + two tasks,
//! all with the default `condition: true` — so it directly exercises the
//! "fold trivially-true condition to None" path (3 folded conditions/message).
//!
//! Run with: `cargo run --example micro_cond_bench --release`

use dataflow_rs::{Engine, Message, Workflow};
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;

const N: usize = 2_000_000;
const WARMUP: usize = 100_000;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let workflow_json = r#"
    {
        "id": "benchmark_workflow",
        "name": "Benchmark Workflow",
        "tasks": [
            {
                "id": "transform_data",
                "name": "Transform Data",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            { "path": "user.id",   "logic": { "var": "payload.input.id" } },
                            { "path": "user.name", "logic": { "var": "payload.input.name" } },
                            { "path": "user.email","logic": { "var": "payload.input.email" } },
                            { "path": "user.age",  "logic": { "+": [{ "var": "payload.input.age" }, 1] } },
                            { "path": "user.status","logic": { "if": [ { ">": [{ "var": "payload.input.age" }, 18] }, "adult", "minor" ] } },
                            { "path": "calculations.total","logic": { "*": [ { "+": [{ "var": "payload.input.age" }, 10] }, { "/": [{ "var": "payload.input.id" }, 100] } ] } }
                        ]
                    }
                }
            },
            {
                "id": "validate_data",
                "name": "Validate Data",
                "function": {
                    "name": "validation",
                    "input": {
                        "rules": [
                            { "path": "user.id",   "logic": { "!!": { "var": "data.user.id" } }, "message": "User ID is required" },
                            { "path": "user.email","logic": { "!!": { "var": "data.user.email" } }, "message": "User email is required" },
                            { "path": "calculations.total","logic": { ">": [{ "var": "data.calculations.total" }, 0] }, "message": "Total must be positive" }
                        ]
                    }
                }
            }
        ]
    }
    "#;

    let workflow = Workflow::from_json(workflow_json).unwrap();
    let engine = Arc::new(Engine::builder().with_workflow(workflow).build().unwrap());

    let data = json!({
        "input": { "id": 12345, "name": "John Doe", "email": "john.doe@example.com", "age": 25, "department": "Engineering" }
    });

    for _ in 0..WARMUP {
        let mut m = Message::from_value(&data);
        engine.process_message(&mut m).await.unwrap();
    }

    let start = Instant::now();
    for _ in 0..N {
        let mut m = Message::from_value(&data);
        engine.process_message(&mut m).await.unwrap();
    }
    let el = start.elapsed();

    println!(
        "MICRO {} msgs in {:.3}s => {:.1} ns/msg => {:.0} msg/s",
        N,
        el.as_secs_f64(),
        el.as_nanos() as f64 / N as f64,
        N as f64 / el.as_secs_f64()
    );
}
