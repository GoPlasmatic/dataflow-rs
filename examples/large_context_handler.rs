//! # Large-context handler
//!
//! A row of the v4 performance gate: a host handler that resolves five
//! `Template` fields against a 2 MiB context. In 3.15 each view of the context
//! costs its spines, one per node, whether the fields read them or not. A v4
//! handler evaluates against the run's single view and builds none per call,
//! so the gate requires v4 to beat this benchmark's own `with_view` row.
//!
//! #66 measured the same call on 3.15.0 at 174 µs per call, or 36 µs inside
//! one `with_view`, over a context whose shape it did not record. This one is
//! node-dense (catalogue records of 14 nodes each), so its figures are higher
//! and are the ones to compare against.
//!
//! The handler is the only task of a 1000-sweep counter loop, so one message
//! makes 1000 calls against one large context, and every figure is per call.
//! Three handlers write the same five values:
//!
//! - **write only** resolves nothing: the floor of a call and a sweep;
//! - **per field** resolves each field on its own, one view each;
//! - **with_view** resolves all five inside one view.
//!
//! Run with: `cargo run --example large_context_handler --release`

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

const CONTEXT_BYTES: usize = 2 * 1024 * 1024;
const CALLS: usize = 1000;
const MESSAGES: usize = 5;

#[derive(Deserialize)]
struct Fields {
    order_id: Template,
    url: Template,
    tenant: Template,
    qty: Template,
    tier: Template,
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
fn output(order_id: String, url: String, tenant: String, qty: u64, tier: String) -> OwnedDataValue {
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

#[async_trait]
impl AsyncFunctionHandler for Resolve {
    type Input = Fields;

    fn compile_input(f: &mut Fields, c: &TemplateCompiler) -> Result<()> {
        f.order_id.compile(c, "order_id")?;
        f.url.compile(c, "url")?;
        f.tenant.compile(c, "tenant")?;
        f.qty.compile(c, "qty")?;
        f.tier.compile(c, "tier")
    }

    async fn execute(&self, ctx: &mut TaskContext<'_>, f: &Fields) -> Result<TaskOutcome> {
        let out = match self.0 {
            Mode::WriteOnly => output(
                "ORD-42".into(),
                "https://api.example.com/orders/ORD-42/lines".into(),
                "acme".into(),
                3,
                "priority".into(),
            ),
            Mode::PerField => output(
                f.order_id.resolve_string(ctx)?,
                f.url.resolve_string(ctx)?,
                f.tenant.resolve_string(ctx)?,
                f.qty.resolve_u64(ctx, "qty")?,
                f.tier.resolve_string(ctx)?,
            ),
            Mode::WithView => ctx.with_view(|view| {
                Ok::<_, DataflowError>(output(
                    f.order_id.resolve_string_in(view)?,
                    f.url.resolve_string_in(view)?,
                    f.tenant.resolve_string_in(view)?,
                    f.qty.resolve_u64_in(view, "qty")?,
                    f.tier.resolve_string_in(view)?,
                ))
            })?,
        };
        ctx.set("data.out", out);
        Ok(TaskOutcome::Success)
    }
}

fn workflow(function: &str) -> Workflow {
    let workflow = json!({
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
    });
    Workflow::from_json(&workflow.to_string()).expect("the workflow parses")
}

fn message(data: &OwnedDataValue, metadata: &OwnedDataValue) -> Message {
    Message::builder()
        .data(data.clone())
        .metadata(metadata.clone())
        .build()
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

    let mut means = Vec::new();
    for &(name, mode) in MODES {
        let engine = Engine::builder()
            .with_workflow(workflow(name))
            .register(name, Resolve(mode))
            .build()
            .expect("the engine builds");

        // The check, which also warms the engine up.
        let mut m = message(&data, &metadata);
        engine.process_message(&mut m).await.expect("the loop runs");
        assert!(m.errors().is_empty(), "errors: {:?}", m.errors());
        assert_eq!(Value::from(&m.context["data"]["out"]), expected(), "{name}");
        assert_eq!(m.audit_trail().len(), CALLS, "one call per sweep");

        let mut samples = bench::Samples::with_capacity(MESSAGES);
        for _ in 0..MESSAGES {
            let mut m = message(&data, &metadata);
            let start = Instant::now();
            engine.process_message(&mut m).await.expect("the loop runs");
            samples.push(start.elapsed());
        }
        means.push(samples.row(name, CALLS, "call"));
    }

    println!();
    println!(
        "Resolving the five fields, over the write-only floor: per field {:.2} µs, with_view {:.2} µs",
        means[1] - means[0],
        means[2] - means[0]
    );
}
