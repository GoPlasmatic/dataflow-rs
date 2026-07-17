//! # Aggregate-heavy mapping microbenchmark (checkout-style)
//!
//! Measures the per-message cost of a map task dominated by array aggregates —
//! the workload shape datalogic-rs 5.1 optimizes with `reduce(map(...))` fusion
//! (fold runs directly over the map input, no intermediate arena array) and
//! common-subexpression elimination (repeated pure aggregate subtrees within
//! one mapping's logic are memoized per evaluation, since JSONLogic has no
//! `let` bindings to share them).
//!
//! The workload mirrors datalogic's `macro/checkout-40` benchmark: a 40-item
//! cart where subtotal = `reduce(map(items, price*qty))` is recomputed by
//! every derived mapping (tax, shipping, discount), and the `data.cart.total`
//! mapping inlines the subtotal expression five times in a single logic tree
//! — the CSE target. `micro_cond_bench.rs` covers the aggregate-free shape;
//! only this bench should move when the aggregate paths change.
//!
//! Same methodology as the other micro benches: `current_thread` runtime,
//! tight single-threaded loop, no task-spawn overhead. Uses the canonical
//! parse-then-map idiom from `realistic_benchmark.rs` (payload → `data.input`,
//! mappings read `data.input.*` and write `data.*`).
//!
//! Run with: `cargo run --example micro_aggregate_bench --release`

use dataflow_rs::{Engine, Message, Workflow};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Instant;

const N: usize = 200_000;
const WARMUP: usize = 10_000;
const ITEMS: usize = 40;

/// The repeated pure aggregate: subtotal = sum(items[i].price * items[i].qty),
/// written as the reduce(map(...)) pipeline users produce without `let`.
fn subtotal() -> Value {
    json!({
        "reduce": [
            { "map": [ { "var": "data.input.items" }, { "*": [{ "var": "price" }, { "var": "qty" }] } ] },
            { "+": [{ "var": "current" }, { "var": "accumulator" }] },
            0
        ]
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let workflow_json = json!({
        "id": "checkout_workflow",
        "name": "Checkout Aggregates Workflow",
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
                "id": "derive_cart",
                "name": "Derive Cart Totals",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            { "path": "data.cart.subtotal", "logic": subtotal() },
                            { "path": "data.cart.tax", "logic": { "*": [subtotal(), 0.08] } },
                            { "path": "data.cart.shipping", "logic": { "if": [ { ">": [subtotal(), 100] }, 0, 9.99 ] } },
                            { "path": "data.cart.discount", "logic": { "if": [ { ">": [subtotal(), 500] }, { "*": [subtotal(), 0.05] }, 0 ] } },
                            // The CSE showcase: one logic tree inlining the subtotal aggregate 5x.
                            { "path": "data.cart.total", "logic": { "-": [
                                { "+": [
                                    subtotal(),
                                    { "*": [subtotal(), 0.08] },
                                    { "if": [ { ">": [subtotal(), 100] }, 0, 9.99 ] }
                                ] },
                                { "if": [ { ">": [subtotal(), 500] }, { "*": [subtotal(), 0.05] }, 0 ] }
                            ] } },
                            { "path": "data.cart.item_count", "logic": {
                                "reduce": [
                                    { "map": [ { "var": "data.input.items" }, { "var": "qty" } ] },
                                    { "+": [{ "var": "current" }, { "var": "accumulator" }] },
                                    0
                                ]
                            } }
                        ]
                    }
                }
            },
            {
                "id": "validate_cart",
                "name": "Validate Cart Totals",
                "function": {
                    "name": "validation",
                    "input": {
                        "rules": [
                            { "path": "data.cart.subtotal", "logic": { ">": [{ "var": "data.cart.subtotal" }, 0] }, "message": "Subtotal must be positive" },
                            { "path": "data.cart.total", "logic": { ">=": [{ "var": "data.cart.total" }, { "var": "data.cart.subtotal" }] }, "message": "Total must cover subtotal" }
                        ]
                    }
                }
            }
        ]
    });

    let workflow = Workflow::from_json(&workflow_json.to_string()).unwrap();
    let engine = Arc::new(Engine::builder().with_workflow(workflow).build().unwrap());

    let items: Vec<Value> = (0..ITEMS)
        .map(|i| {
            json!({
                "sku": format!("SKU-{i:04}"),
                "price": 5.0 + (i as f64) * 1.25,
                "qty": (i % 4) + 1
            })
        })
        .collect();
    let data = json!({ "items": items, "cart_id": "cart-42" });

    // Sanity-check once: the aggregates must actually compute (a silent
    // null-skip in the map task would turn this bench into a no-op).
    {
        let mut m = Message::from_value(&data);
        engine.process_message(&mut m).await.unwrap();
        let ctx = serde_json::to_value(m.data()).unwrap();
        let subtotal = ctx["cart"]["subtotal"].as_f64().unwrap_or(0.0);
        assert!(
            subtotal > 0.0 && m.errors().is_empty(),
            "workload is not computing: subtotal={subtotal}, errors={:?}",
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
        "AGGREGATE {} msgs ({} items, parse + 6 mappings + 2 validations) in {:.3}s => {:.1} ns/msg => {:.0} msg/s",
        N,
        ITEMS,
        el.as_secs_f64(),
        el.as_nanos() as f64 / N as f64,
        N as f64 / el.as_secs_f64()
    );
}
