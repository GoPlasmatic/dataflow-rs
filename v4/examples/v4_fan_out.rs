//! # Fan-out
//!
//! v4's port of v3's `fan_out`, a row of the v4 performance gate: one task
//! fans out over 100 elements with `for_each`, on a message that carries a
//! 2 MiB context. In 3.15 each call runs against its own fork of the
//! message, so the fan-out copies the whole context once per element. v4
//! binds each element beneath the call's own writes over the run's one view,
//! which no call copies, so the gate requires this row to be faster.
//!
//! The function is the pricing step of the v4 design's worked example: it
//! evaluates the element's `sku` and `qty` and writes a price to
//! `temp_data.price`, which `collect` names, and the fold gathers the prices
//! into `into` in element order.
//!
//! Run with: `cargo run -p dataflow-rs-v4 --example v4_fan_out --release`

// The derive emits `::dataflow_rs::` paths.
extern crate dataflow_rs_v4 as dataflow_rs;

use dataflow_rs_v4::datavalue::OwnedDataValue;
use dataflow_rs_v4::function::FunctionConfig;
use dataflow_rs_v4::{Ctx, Engine, Expr, Function, Message, Outcome, Path, Result};
use serde_json::{Value, json};
use std::sync::LazyLock;
use std::time::Instant;

#[path = "shared/bench.rs"]
mod bench;
#[path = "../../examples/shared/gate.rs"]
mod gate;

const CONTEXT_BYTES: usize = 2 * 1024 * 1024;
const ELEMENTS: usize = 100;
const MESSAGES: usize = 10;

/// Where every call writes its price, which `collect` names.
static PRICE: LazyLock<Path> = LazyLock::new(|| "temp_data.price".parse().expect("a path"));

/// The element's two fields, each an expression over its binding.
#[derive(FunctionConfig)]
struct PriceConfig {
    sku: Expr<String>,
    qty: Expr<u64>,
}

struct PriceItem;

impl Function for PriceItem {
    type Config = PriceConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &PriceConfig) -> Result<Outcome> {
        let sku = ctx.eval(&cfg.sku)?;
        let qty = ctx.eval(&cfg.qty)?;
        ctx.set(&PRICE, OwnedDataValue::from(&json!(price(&sku, qty))))?;
        Ok(Outcome::Ok)
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

fn workflow() -> Value {
    json!({
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
    })
}

fn message(data: &OwnedDataValue) -> Message {
    Message::from_parts(OwnedDataValue::Null, data.clone(), OwnedDataValue::Null)
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
    let prices: Path = "data.prices".parse().expect("a path");

    let engine = Engine::builder()
        .workflow_json(&workflow())
        .register("price_item", PriceItem)
        .build()
        .expect("the engine builds");

    // The check, which also warms the engine up.
    let mut m = message(&data);
    engine
        .run(&mut m)
        .await
        .into_result()
        .expect("the fan-out runs");
    assert!(m.errors().is_empty(), "errors: {:?}", m.errors());
    let expected: Vec<Value> = (0..ELEMENTS)
        .map(|i| json!(price(&format!("SKU-{:06}", i * 7), (i % 4 + 1) as u64)))
        .collect();
    assert_eq!(
        m.context().get(&prices).map(Value::from),
        Some(json!(expected)),
        "every element priced, in element order"
    );

    let mut samples = bench::Samples::with_capacity(MESSAGES);
    for _ in 0..MESSAGES {
        let mut m = message(&data);
        let start = Instant::now();
        engine
            .run(&mut m)
            .await
            .into_result()
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
