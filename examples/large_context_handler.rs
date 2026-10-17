//! # Large-context handler
//!
//! A row of the performance gate, its workload first written against 3.x: a
//! host function that evaluates five expression fields against a 2 MiB
//! context. In 3.x each view of the context cost its spines, one per node,
//! whether the fields read them or not. A function evaluates against the
//! run's single view and builds none per call, so a call's cost must not grow
//! with the context.
//!
//! The function is the only task of a 1000-sweep counter loop, so one message
//! makes 1000 calls against one large context, and every figure is per call.
//! Three functions write the same five values:
//!
//! - **write only** evaluates nothing: the floor of a call and a sweep;
//! - **per field** evaluates each field through `Ctx::eval`;
//! - **with_view** does the same: there is no view per call, so the two 3.x
//!   modes are one call shape here, and the names stay for the gate.
//!
//! Run with: `cargo run --example large_context_handler --release`

use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::Output;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::{Ctx, Engine, Expr, Function, Message, Outcome, Path, Result};
use serde_json::{Value, json};
use std::time::Instant;

#[path = "shared/bench.rs"]
mod bench;
#[path = "shared/gate.rs"]
mod gate;

const CONTEXT_BYTES: usize = 2 * 1024 * 1024;
const CALLS: usize = 1000;
const MESSAGES: usize = 5;

/// The five fields, and where every mode writes: v3's literal `data.out`,
/// folded to a path at build.
#[derive(FunctionConfig)]
struct Fields {
    order_id: Expr<String>,
    url: Expr<String>,
    tenant: Expr<String>,
    qty: Expr<u64>,
    tier: Expr<String>,
    #[output(default = "data.out")]
    target: Output,
}

#[derive(Clone, Copy)]
enum Mode {
    WriteOnly,
    PerField,
    WithView,
}

const MODES: &[(&str, Mode)] = &[
    ("write_only", Mode::WriteOnly),
    ("per_field", Mode::PerField),
    ("with_view", Mode::WithView),
];

struct Resolve(Mode);

/// What every mode writes.
fn output(order_id: &str, url: &str, tenant: &str, qty: u64, tier: &str) -> OwnedDataValue {
    OwnedDataValue::from(&json!({
        "order_id": order_id, "url": url, "tenant": tenant, "qty": qty, "tier": tier
    }))
}

fn expected() -> Value {
    json!({
        "order_id": "ORD-42",
        "url": "https://api.example.com/orders/ORD-42/lines",
        "tenant": "acme",
        "qty": 3,
        "tier": "priority"
    })
}

impl Function for Resolve {
    type Config = Fields;

    async fn call(&self, ctx: &mut Ctx<'_>, f: &Fields) -> Result<Outcome> {
        let out = match self.0 {
            Mode::WriteOnly => output(
                "ORD-42",
                "https://api.example.com/orders/ORD-42/lines",
                "acme",
                3,
                "priority",
            ),
            // One call shape for both: each field evaluated against the run's
            // view, which no call builds.
            Mode::PerField | Mode::WithView => output(
                &ctx.eval(&f.order_id)?,
                &ctx.eval(&f.url)?,
                &ctx.eval(&f.tenant)?,
                ctx.eval(&f.qty)?,
                &ctx.eval(&f.tier)?,
            ),
        };
        ctx.emit(&f.target, out)?;
        Ok(Outcome::Ok)
    }
}

fn workflow(function: &str) -> Value {
    json!({
        "id": "calls",
        "name": "One handler call per sweep",
        "loop": { "counter": "i", "max": CALLS },
        "tasks": [{ "id": "call", "name": "Five fields", "function": { "name": function, "input": {
            "order_id": { "var": "data.order.id" },
            "url": { "cat": ["https://api.example.com/orders/", { "var": "data.order.id" }, "/lines"] },
            "tenant": { "var": "metadata.tenant" },
            "qty": { "var": "data.order.qty" },
            "tier": { "if": [{ ">": [{ "var": "data.order.total" }, 100] }, "priority", "standard"] }
        }}}]
    })
}

fn message(data: &OwnedDataValue, metadata: &OwnedDataValue) -> Message {
    Message::from_parts(OwnedDataValue::Null, data.clone(), metadata.clone())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let catalog = bench::large_document(CONTEXT_BYTES);
    println!("LARGE-CONTEXT HANDLER: five Template fields per call, {CALLS} calls per message");
    println!(
        "Context: {} at data.catalog, read by no field\n",
        bench::describe(&catalog)
    );
    let data = OwnedDataValue::from(&json!({
        "catalog": catalog,
        "order": { "id": "ORD-42", "total": 250.75, "qty": 3 }
    }));
    let metadata = OwnedDataValue::from(&json!({ "tenant": "acme" }));
    let out: Path = "data.out".parse().expect("a path");

    let mut means = Vec::new();
    let mut gate = gate::Gate::new("large_context_handler");
    for &(name, mode) in MODES {
        let engine = Engine::builder()
            .workflow_json(&workflow(name))
            .register(name, Resolve(mode))
            .build()
            .expect("the engine builds");

        // The check, which also warms the engine up.
        let mut m = message(&data, &metadata);
        engine
            .run(&mut m)
            .await
            .into_result()
            .expect("the loop runs");
        assert!(m.errors().is_empty(), "errors: {:?}", m.errors());
        assert_eq!(
            m.context().get(&out).map(Value::from),
            Some(expected()),
            "{name}"
        );
        assert_eq!(m.audit().len(), CALLS, "one call per sweep");

        let mut samples = bench::Samples::with_capacity(MESSAGES);
        for _ in 0..MESSAGES {
            let mut m = message(&data, &metadata);
            let start = Instant::now();
            engine
                .run(&mut m)
                .await
                .into_result()
                .expect("the loop runs");
            samples.push(start.elapsed());
        }
        let row = samples.row(name, CALLS, "call");
        gate.row(name, row, "µs/call");
        means.push(row.0);
    }

    println!();
    println!(
        "Resolving the five fields, over the write-only floor: per field {:.2} µs, with_view {:.2} µs",
        means[1] - means[0],
        means[2] - means[0]
    );
    gate.emit();
}
