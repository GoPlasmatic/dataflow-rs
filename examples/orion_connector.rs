//! # Orion-shaped connector
//!
//! v4's port of v3's `orion_connector`, a row of the v4 performance gate: a
//! host function shaped like Orion 1.12's `mongo_read`, its most common kind
//! of call, behind a layer shaped like Orion's `Connector<H>`:
//!
//! - three literal fields: `connector`, `database`, `collection`;
//! - three document fields, `filter`, `projection` and `sort`, each an
//!   `Interpolated` whose `{"var": …}` holes `Ctx::render` looks up once per
//!   call, keeping their `$` keys, where v3 walks and clones each document;
//! - two numeric `Expr` fields, `limit` and `skip`, and an `Output` that
//!   defaults to `data`, folded to a path at build when literal;
//! - the rendered documents serialized straight into the wire buffer for the
//!   driver, with `serde_json` where Orion calls `bson::to_document`, building
//!   no tree, and the rows written through the `Output`;
//! - a layer: a `CallLayer` on the registry, added through
//!   `EngineBuilder::layer`, times every host call into shared counters, as
//!   Orion's connector shell does for its metrics.
//!
//! The driver answers from memory, so the figure is the engine's and the
//! connector's own share of a call. The gate requires this row to be no
//! slower than v3's.
//!
//! Run with: `cargo run --example orion_connector --release`

use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::{Interpolated, Output};
use dataflow_rs::function::{BoxFuture, CallLayer, FunctionConfig, Next};
use dataflow_rs::{Ctx, Engine, Error, Expr, Function, Message, Outcome, Path, Result};
use serde_json::{Value, json};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::Instant;

#[path = "shared/bench.rs"]
mod bench;
#[path = "shared/gate.rs"]
mod gate;

const TOTAL_MESSAGES: usize = 500_000;
const WARMUP_MESSAGES: usize = 5_000;

/// Where the driver reads the customer: the path the filter's `customer_id`
/// hole reads.
static CUSTOMER: LazyLock<Path> =
    LazyLock::new(|| "data.request.customer.id".parse().expect("a path"));

/// The call's config, compiled once at build: the documents' holes found,
/// `skip` folded, the literal output folded to a path.
#[derive(FunctionConfig)]
struct FindConfig {
    connector: String,
    database: String,
    collection: String,
    filter: Interpolated,
    projection: Interpolated,
    sort: Interpolated,
    limit: Expr<u64>,
    skip: Expr<u64>,
    #[output(default = "data")]
    output: Output,
}

/// The driver, answering from memory: `limit` rows for the customer, after
/// `skip`. The customer is what the call read through `Ctx::get`, borrowed
/// from the context, so a read that found nothing shows in the rows.
fn driver_find(customer: Option<&OwnedDataValue>, limit: u64, skip: u64) -> OwnedDataValue {
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

/// Render the documents into the wire, ask the driver, write the rows through
/// `output`.
struct MongoFind {
    /// The first call's wire, which the check parses once.
    first_wire: Arc<OnceLock<Vec<u8>>>,
}

impl Function for MongoFind {
    type Config = FindConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &FindConfig) -> Result<Outcome> {
        let limit = ctx.eval(&cfg.limit)?;
        let skip = ctx.eval(&cfg.skip)?;

        // The document fields, each hole looked up once and serialized in
        // place, straight into the wire.
        let mut wire = Vec::with_capacity(256);
        for doc in [&cfg.filter, &cfg.projection, &cfg.sort] {
            serde_json::to_writer(&mut wire, &ctx.render(doc))
                .map_err(|e| Error::service("VALIDATION_ERROR", e.to_string()))?;
        }
        self.first_wire.get_or_init(|| wire.clone());
        black_box((&cfg.connector, &cfg.database, &cfg.collection, &wire));

        let rows = driver_find(ctx.get(&CUSTOMER).as_deref(), limit, skip);
        ctx.emit(&cfg.output, rows)?;
        Ok(Outcome::Ok)
    }
}

/// The layer: times every host call into shared counters.
struct Metered {
    calls: Arc<AtomicU64>,
    nanos: Arc<AtomicU64>,
}

impl CallLayer for Metered {
    fn call<'a>(&'a self, ctx: &'a mut Ctx<'_>, next: Next<'a>) -> BoxFuture<'a, Result<Outcome>> {
        Box::pin(async move {
            let start = Instant::now();
            let outcome = next.run(ctx).await;
            self.nanos
                .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
            self.calls.fetch_add(1, Ordering::Relaxed);
            outcome
        })
    }
}

fn workflow() -> Value {
    json!({
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
    })
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
    let first_wire = Arc::new(OnceLock::new());
    let engine = Engine::builder()
        .workflow_json(&workflow())
        .register(
            "mongo_find",
            MongoFind {
                first_wire: Arc::clone(&first_wire),
            },
        )
        .layer(Metered {
            calls: Arc::clone(&calls),
            nanos: Arc::clone(&nanos),
        })
        .build()
        .expect("the engine builds");
    let payload = Arc::new(OwnedDataValue::from(&request()));

    // The check: the rows landed at `output`, the driver read the customer,
    // the map read the rows, the layer saw the call, and the wire, parsed
    // once, holds the documents with their holes folded.
    let mut m = Message::new(Arc::clone(&payload));
    engine
        .run(&mut m)
        .await
        .into_result()
        .expect("the workflow runs");
    assert!(m.errors().is_empty(), "errors: {:?}", m.errors());
    let data = Value::from(m.context().data());
    assert_eq!(
        data["orders"].as_array().map(Vec::len),
        Some(3),
        "limit rows at output"
    );
    assert_eq!(
        data["orders"][0]["customer_id"],
        json!("CUST-1042"),
        "the driver read the customer"
    );
    assert_eq!(
        data["summary"]["count"].as_f64(),
        Some(3.0),
        "the map read the rows"
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1, "the layer saw the call");
    let wire = first_wire.get().expect("the call wrote its wire");
    let docs = serde_json::Deserializer::from_slice(wire)
        .into_iter::<Value>()
        .collect::<std::result::Result<Vec<_>, _>>()
        .expect("the wire parses");
    assert_eq!(
        docs,
        [
            json!({
                "customer_id": "CUST-1042",
                "status": { "$in": ["open", "pending"] },
                "created_at": { "$gte": { "$date": "2026-01-01T00:00:00Z" } },
                "region": "eu"
            }),
            json!({ "_id": 0, "id": 1, "total": 1, "status": 1, "customer_id": 1 }),
            json!({ "created_at": -1 }),
        ],
        "the var holes were folded, the $ keys kept"
    );

    println!("ORION-SHAPED CONNECTOR: parse_json + mongo_find behind a timing layer + map");
    println!("Messages: {TOTAL_MESSAGES}, cores: {}\n", num_cpus::get());
    let (mut samples, throughput) =
        bench::spawned(&engine, &payload, WARMUP_MESSAGES, TOTAL_MESSAGES).await;
    let row = samples.row("parse + connector + map", 1, "message");
    println!("Throughput: {throughput:.0} msg/s");
    let n = calls.load(Ordering::Relaxed);
    println!(
        "Inside the layer: {:.2} µs per call, over {n} calls",
        nanos.load(Ordering::Relaxed) as f64 / n as f64 / 1000.0
    );
    gate::Gate::new("orion_connector")
        .row("connector", row, "µs/message")
        .higher("connector.throughput", throughput, "msg/s")
        .emit();
}
