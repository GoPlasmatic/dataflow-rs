//! # Single-threaded process_message microbenchmark
//!
//! Isolates the per-message CPU cost of `process_message` with **no tokio
//! task-spawn overhead** — the macro benchmarks (`benchmark.rs`) are dominated
//! by Tokio scheduling and can't resolve a sub-100ns/message change. This runs
//! a tight single-threaded loop so the signal of interest (condition-eval
//! work) is a measurable fraction of the total.
//!
//! Uses the same workflow shape as `benchmark.rs`: one workflow + three tasks
//! (parse + map + validate), all with the default `condition: true` — so it
//! directly exercises the "fold trivially-true condition to None" path
//! (4 folded conditions/message).
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
                "id": "load_payload",
                "name": "Parse Payload",
                "function": {
                    "name": "parse_json",
                    "input": { "source": "payload", "target": "input" }
                }
            },
            {
                "id": "transform_data",
                "name": "Transform Data",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            { "path": "data.user.id",   "logic": { "var": "data.input.id" } },
                            { "path": "data.user.name", "logic": { "var": "data.input.name" } },
                            { "path": "data.user.email","logic": { "var": "data.input.email" } },
                            { "path": "data.user.age",  "logic": { "+": [{ "var": "data.input.age" }, 1] } },
                            { "path": "data.user.status","logic": { "if": [ { ">": [{ "var": "data.input.age" }, 18] }, "adult", "minor" ] } },
                            { "path": "data.calculations.total","logic": { "*": [ { "+": [{ "var": "data.input.age" }, 10] }, { "/": [{ "var": "data.input.id" }, 100] } ] } }
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
                            { "path": "data.user.id",   "logic": { "!!": { "var": "data.user.id" } }, "message": "User ID is required" },
                            { "path": "data.user.email","logic": { "!!": { "var": "data.user.email" } }, "message": "User email is required" },
                            { "path": "data.calculations.total","logic": { ">": [{ "var": "data.calculations.total" }, 0] }, "message": "Total must be positive" }
                        ]
                    }
                }
            }
        ]
    }
    "#;

    let workflow = Workflow::from_json(workflow_json).unwrap();
    let engine = Arc::new(Engine::builder().with_workflow(workflow).build().unwrap());

    // The raw inbound document; the parse_json task loads it at `data.input`.
    let data = json!({
        "id": 12345, "name": "John Doe", "email": "john.doe@example.com", "age": 25, "department": "Engineering"
    });

    // Sanity-check once: the workflow must actually compute (payload.* var
    // references or unprefixed write paths would silently null-skip every
    // mapping and fail every validation without erroring the run).
    {
        let mut m = Message::from_value(&data);
        engine.process_message(&mut m).await.unwrap();
        let d = serde_json::to_value(m.data()).unwrap();
        assert!(
            d["user"]["id"].as_i64() == Some(12345) && m.errors().is_empty(),
            "workload is not computing: data={d}, errors={:?}",
            m.errors()
        );
    }

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
