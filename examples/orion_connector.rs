//! # Orion-shaped connector
//!
//! A row of the v4 performance gate: a host function shaped like Orion
//! 1.12's `mongo_read`, its most common kind of call, behind a wrapper shaped
//! like Orion's `Connector<H>`:
//!
//! - three literal fields: `connector`, `database`, `collection`;
//! - three document fields, `filter`, `projection` and `sort`, whose
//!   `{"var": …}` holes are folded by walking and cloning the document on
//!   every call, as Orion's `resolve_value` does, keeping their `$` keys;
//! - two numeric `Template` fields, `limit` and `skip`, and an `output` path
//!   `Template` that defaults to `data`, resolved in one `with_view`;
//! - the folded documents serialized for the driver, with `serde_json` where
//!   Orion calls `bson::to_document`, and the rows written to `output`;
//! - a layer: the wrapper times every call into shared counters, as Orion's
//!   connector shell does for its metrics. 3.15 has no around-call hook, so
//!   the wrapper is registered in the handler's place.
//!
//! The driver answers from memory, so the figure is the engine's and the
//! connector's own share of a call. v4 replaces the walk with an
//! `Interpolated` rendered straight into the serializer, the output path with
//! an `Output`, and the wrapper with a `CallLayer`; the gate requires this row
//! to be no slower.
//!
//! Run with: `cargo run --example orion_connector --release`

use async_trait::async_trait;
use dataflow_rs::{
    AsyncFunctionHandler, DataflowError, Engine, Message, Result, TaskContext, TaskOutcome,
    Template, TemplateCompiler, Workflow,
};
use datavalue::OwnedDataValue;
use serde::Deserialize;
use serde_json::{Value, json};
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

#[path = "shared/bench.rs"]
mod bench;

const TOTAL_MESSAGES: usize = 500_000;
const WARMUP_MESSAGES: usize = 5_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FindInput {
    connector: String,
    database: String,
    collection: String,
    #[serde(default)]
    filter: Value,
    #[serde(default)]
    projection: Value,
    #[serde(default)]
    sort: Value,
    limit: Template,
    skip: Template,
    #[serde(default)]
    output: Template,
}

/// Orion's `resolve_value`: fold every `{"var": …}` node of a document and
/// clone the rest, on every call. A value read from the message is not
/// scanned again.
fn fold(doc: &Value, ctx: &TaskContext<'_>) -> Value {
    match doc {
        Value::Object(o) => {
            if o.len() == 1
                && let Some(spec) = o.get("var")
            {
                return lookup(spec, ctx);
            }
            Value::Object(o.iter().map(|(k, v)| (k.clone(), fold(v, ctx))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(|v| fold(v, ctx)).collect()),
        other => other.clone(),
    }
}

/// A `var` node's value: the string form, or the array form with a default.
fn lookup(spec: &Value, ctx: &TaskContext<'_>) -> Value {
    let (path, default) = match spec {
        Value::String(path) => (path.as_str(), Value::Null),
        Value::Array(a) => match a.first().and_then(Value::as_str) {
            Some(path) => (path, a.get(1).cloned().unwrap_or(Value::Null)),
            None => return Value::Null,
        },
        _ => return Value::Null,
    };
    ctx.get(path).map(Value::from).unwrap_or(default)
}

/// The driver, answering from memory: `limit` rows for the filter's customer,
/// after `skip`. It reads the folded filter, so a fold that resolved nothing
/// shows in the rows.
fn driver_find(filter: &Value, limit: u64, skip: u64) -> OwnedDataValue {
    let customer = &filter["customer_id"];
    let rows: Vec<Value> = (skip..skip + limit)
        .map(|i| {
            json!({
                "id": format!("ORD-{i:04}"),
                "customer_id": customer,
                "status": "open",
                "total": 100.0 + i as f64 * 12.5
            })
        })
        .collect();
    OwnedDataValue::from(&Value::Array(rows))
}

/// Parse the call, ask the driver, write the rows to `output`.
struct MongoFind;

#[async_trait]
impl AsyncFunctionHandler for MongoFind {
    type Input = FindInput;

    fn compile_input(input: &mut FindInput, c: &TemplateCompiler) -> Result<()> {
        input.limit.compile(c, "limit")?;
        input.skip.compile(c, "skip")?;
        input.output.compile(c, "output")
    }

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &FindInput) -> Result<TaskOutcome> {
        // The expression fields, in one view, as Orion's connector shell does.
        let (limit, skip, output) = ctx.with_view(|view| -> Result<(u64, u64, String)> {
            let output = match input.output.resolve_in(view)? {
                OwnedDataValue::Null => "data".to_string(),
                OwnedDataValue::String(path) if !path.is_empty() => path,
                other => {
                    return Err(DataflowError::Validation(format!(
                        "output must resolve to a path, got {other:?}"
                    )));
                }
            };
            Ok((
                input.limit.resolve_u64_in(view, "limit")?,
                input.skip.resolve_u64_in(view, "skip")?,
                output,
            ))
        })?;

        // The document fields, walked and cloned, then serialized for the driver.
        let filter = fold(&input.filter, ctx);
        let projection = fold(&input.projection, ctx);
        let sort = fold(&input.sort, ctx);
        let mut wire = Vec::with_capacity(256);
        for doc in [&filter, &projection, &sort] {
            serde_json::to_writer(&mut wire, doc)
                .map_err(|e| DataflowError::Validation(e.to_string()))?;
        }
        black_box((&input.connector, &input.database, &input.collection, &wire));

        ctx.set(&output, driver_find(&filter, limit, skip));
        Ok(TaskOutcome::Success)
    }
}

/// The layer: times every call into shared counters.
struct Metered<H> {
    inner: H,
    calls: Arc<AtomicU64>,
    nanos: Arc<AtomicU64>,
}

#[async_trait]
impl<H: AsyncFunctionHandler> AsyncFunctionHandler for Metered<H> {
    type Input = H::Input;

    fn parse_input(input: &Value) -> Result<H::Input> {
        H::parse_input(input)
    }

    fn compile_input(input: &mut H::Input, c: &TemplateCompiler) -> Result<()> {
        H::compile_input(input, c)
    }

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &H::Input) -> Result<TaskOutcome> {
        let start = Instant::now();
        let outcome = self.inner.execute(ctx, input).await;
        self.nanos
            .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.calls.fetch_add(1, Ordering::Relaxed);
        outcome
    }
}

fn workflow() -> Workflow {
    let workflow = json!({
        "id": "orders_lookup",
        "name": "Orders lookup",
        "tasks": [
            { "id": "load", "name": "Parse request", "function": {
                "name": "parse_json", "input": { "source": "payload", "target": "request" } } },
            { "id": "find", "name": "Find open orders", "function": { "name": "mongo_find", "input": {
                "connector": "orders_db",
                "database": "shop",
                "collection": "orders",
                "filter": {
                    "customer_id": { "var": "data.request.customer.id" },
                    "status": { "$in": ["open", "pending"] },
                    "created_at": { "$gte": { "$date": { "var": "data.request.since" } } },
                    "region": { "var": ["data.request.region", "eu"] }
                },
                "projection": { "_id": 0, "id": 1, "total": 1, "status": 1, "customer_id": 1 },
                "sort": { "created_at": -1 },
                "limit": { "var": "data.request.page_size" },
                "skip": 0,
                "output": "data.orders"
            }}},
            { "id": "summarise", "name": "Summarise", "function": { "name": "map", "input": {
                "mappings": [
                    { "path": "data.summary.count", "logic":
                        { "reduce": [{ "var": "data.orders" }, { "+": [{ "var": "accumulator" }, 1] }, 0] } },
                    { "path": "data.summary.first_total", "logic": { "var": "data.orders.0.total" } }
                ]
            }}}
        ]
    });
    Workflow::from_json(&workflow.to_string()).expect("the workflow parses")
}

fn request() -> Value {
    json!({
        "customer": { "id": "CUST-1042", "name": "Acme Global Payments GmbH", "tier": "gold" },
        "since": "2026-01-01T00:00:00Z",
        "page_size": 3
    })
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let calls = Arc::new(AtomicU64::new(0));
    let nanos = Arc::new(AtomicU64::new(0));
    let engine = Arc::new(
        Engine::builder()
            .with_workflow(workflow())
            .register(
                "mongo_find",
                Metered {
                    inner: MongoFind,
                    calls: Arc::clone(&calls),
                    nanos: Arc::clone(&nanos),
                },
            )
            .build()
            .expect("the engine builds"),
    );
    let payload = Arc::new(OwnedDataValue::from(&request()));

    // The check: the rows landed at `output`, the filter's hole was folded,
    // the map read them, and the layer saw the call.
    let mut m = Message::new(Arc::clone(&payload));
    engine
        .process_message(&mut m)
        .await
        .expect("the workflow runs");
    assert!(m.errors().is_empty(), "errors: {:?}", m.errors());
    let data = Value::from(&m.context["data"]);
    assert_eq!(
        data["orders"].as_array().map(Vec::len),
        Some(3),
        "limit rows at output"
    );
    assert_eq!(
        data["orders"][0]["customer_id"],
        json!("CUST-1042"),
        "the var hole was folded"
    );
    assert_eq!(
        data["summary"]["count"].as_f64(),
        Some(3.0),
        "the map read the rows"
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1, "the layer saw the call");

    println!("ORION-SHAPED CONNECTOR: parse_json + mongo_find behind a timing layer + map");
    println!("Messages: {TOTAL_MESSAGES}, cores: {}\n", num_cpus::get());
    let (mut samples, throughput) =
        bench::spawned(&engine, &payload, WARMUP_MESSAGES, TOTAL_MESSAGES).await;
    samples.row("parse + connector + map", 1, "message");
    println!("Throughput: {throughput:.0} msg/s");
    let n = calls.load(Ordering::Relaxed);
    println!(
        "Inside the layer: {:.2} µs per call, over {n} calls",
        nanos.load(Ordering::Relaxed) as f64 / n as f64 / 1000.0
    );
}
