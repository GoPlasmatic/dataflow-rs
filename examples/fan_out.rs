//! # Fan-out
//!
//! A row of the v4 performance gate: one task fans out over 100 elements with
//! `for_each`, on a message that carries a 2 MiB context. In 3.15 each call
//! runs against its own fork of the message, so the fan-out copies the whole
//! context once per element. v4 isolates an element with a binding layer and
//! an overlay over the run's one view, so the gate requires this row to be
//! faster.
//!
//! The handler is the pricing step of the v4 design's worked example: it
//! resolves the element's `sku` and `qty` in one view and writes a price to
//! `collect`, and the fold gathers the prices into `into` in element order.
//!
//! Run with: `cargo run --example fan_out --release`

use async_trait::async_trait;
use dataflow_rs::{
    AsyncFunctionHandler, DataflowError, Engine, Message, Result, TaskContext, TaskOutcome,
    Template, TemplateCompiler, Workflow,
};
use datavalue::OwnedDataValue;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Instant;

#[path = "shared/bench.rs"]
mod bench;
#[path = "shared/gate.rs"]
mod gate;

const CONTEXT_BYTES: usize = 2 * 1024 * 1024;
const ELEMENTS: usize = 100;
const MESSAGES: usize = 10;

#[derive(Deserialize)]
struct PriceInput {
    sku: Template,
    qty: Template,
}

struct PriceItem;

#[async_trait]
impl AsyncFunctionHandler for PriceItem {
    type Input = PriceInput;

    fn compile_input(input: &mut PriceInput, c: &TemplateCompiler) -> Result<()> {
        input.sku.compile(c, "sku")?;
        input.qty.compile(c, "qty")
    }

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &PriceInput) -> Result<TaskOutcome> {
        let (sku, qty) = ctx.with_view(|view| {
            Ok::<_, DataflowError>((
                input.sku.resolve_string_in(view)?,
                input.qty.resolve_u64_in(view, "qty")?,
            ))
        })?;
        ctx.set(
            "temp_data.price",
            OwnedDataValue::from(&json!(price(&sku, qty))),
        );
        Ok(TaskOutcome::Success)
    }
}

/// A unit price from the SKU's number, times the quantity.
fn price(sku: &str, qty: u64) -> f64 {
    let n: u64 = sku
        .trim_start_matches("SKU-")
        .parse()
        .expect("a numbered SKU");
    qty as f64 * ((n % 50) as f64 + 0.5)
}

fn item(i: usize) -> Value {
    json!({ "sku": format!("SKU-{:06}", i * 7), "qty": i % 4 + 1 })
}

fn workflow() -> Workflow {
    let workflow = json!({
        "id": "price_batch",
        "name": "Price a batch",
        "tasks": [{
            "id": "price",
            "name": "Price each item",
            "for_each": {
                "over": { "var": "data.batch.items" },
                "as": "item",
                "collect": "temp_data.price",
                "into": "data.prices"
            },
            "function": { "name": "price_item", "input": {
                "sku": { "var": "temp_data.item.sku" },
                "qty": { "var": "temp_data.item.qty" }
            }}
        }]
    });
    Workflow::from_json(&workflow.to_string()).expect("the workflow parses")
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let catalog = bench::large_document(CONTEXT_BYTES);
    println!("FAN-OUT: for_each over {ELEMENTS} elements, one host call each");
    println!(
        "Context: {} at data.catalog, read by no call\n",
        bench::describe(&catalog)
    );
    let items: Vec<Value> = (0..ELEMENTS).map(item).collect();
    let data = OwnedDataValue::from(&json!({ "catalog": catalog, "batch": { "items": items } }));

    let engine = Engine::builder()
        .with_workflow(workflow())
        .register("price_item", PriceItem)
        .build()
        .expect("the engine builds");

    // The check, which also warms the engine up.
    let mut m = Message::builder().data(data.clone()).build();
    engine
        .process_message(&mut m)
        .await
        .expect("the fan-out runs");
    assert!(m.errors().is_empty(), "errors: {:?}", m.errors());
    let prices = Value::from(&m.context["data"]["prices"]);
    let expected: Vec<Value> = (0..ELEMENTS)
        .map(|i| json!(price(&format!("SKU-{:06}", i * 7), (i % 4 + 1) as u64)))
        .collect();
    assert_eq!(
        prices,
        json!(expected),
        "every element priced, in element order"
    );

    let mut samples = bench::Samples::with_capacity(MESSAGES);
    for _ in 0..MESSAGES {
        let mut m = Message::builder().data(data.clone()).build();
        let start = Instant::now();
        engine
            .process_message(&mut m)
            .await
            .expect("the fan-out runs");
        samples.push(start.elapsed());
    }
    let per_message = samples.row("one fan-out, per message", 1, "message");
    let per_element = samples.row("one fan-out, per element", ELEMENTS, "element");
    gate::Gate::new("fan_out")
        .row("per_message", per_message, "µs/message")
        .row("per_element", per_element, "µs/element")
        .emit();
}
