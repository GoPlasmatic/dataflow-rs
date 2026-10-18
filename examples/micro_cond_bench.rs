//! # Single-threaded run microbenchmark
//!
//! Isolates the per-message CPU cost of `Engine::run` with **no tokio
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
//! This is v4's port of v3's `micro_cond_bench`: the same definition, count
//! and single-threaded loop, run through dataflow-rs 4.
//!
//! Run with: `cargo run --example micro_cond_bench --release`

use dataflow_rs::{Engine, Message};
use serde_json::{Value, json};
use std::time::Instant;

#[path = "shared/gate.rs"]
mod gate;

fn n() -> usize {
    gate::count(2_000_000, 1000)
}
fn warmup() -> usize {
    gate::count(100_000, 100)
}

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

    let workflow: Value = serde_json::from_str(workflow_json).unwrap();
    let engine = Engine::builder()
        .workflow_json(&workflow)
        .build()
        .expect("the engine builds");

    // The raw inbound document; the parse_json task loads it at `data.input`.
    let data = json!({
        "id": 12345, "name": "John Doe", "email": "john.doe@example.com", "age": 25, "department": "Engineering"
    });

    // Sanity-check once: the workflow must actually compute (payload.* var
    // references or unprefixed write paths would silently null-skip every
    // mapping and fail every validation without erroring the run).
    {
        let mut m = Message::from_json(&data);
        engine.run(&mut m).await.into_result().unwrap();
        let d = Value::from(m.context().data());
        assert!(
            d["user"]["id"].as_i64() == Some(12345) && m.errors().is_empty(),
            "workload is not computing: data={d}, errors={:?}",
            m.errors()
        );
    }

    for _ in 0..warmup() {
        let mut m = Message::from_json(&data);
        engine.run(&mut m).await.into_result().unwrap();
    }

    let start = Instant::now();
    for _ in 0..n() {
        let mut m = Message::from_json(&data);
        engine.run(&mut m).await.into_result().unwrap();
    }
    let el = start.elapsed();

    println!(
        "MICRO {} msgs in {:.3}s => {:.1} ns/msg => {:.0} msg/s",
        n(),
        el.as_secs_f64(),
        el.as_nanos() as f64 / n() as f64,
        n() as f64 / el.as_secs_f64()
    );

    gate::Gate::new("micro_cond_bench")
        .lower("ns_per_msg", el.as_nanos() as f64 / n() as f64, "ns/msg")
        .emit();
}
