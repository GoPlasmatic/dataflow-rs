//! Measurement helpers for the benchmarks added as rows of the v4 performance
//! gate.
//!
//! Not an example: a benchmark pulls it in with
//! `#[path = "shared/bench.rs"] mod bench;`, and no single benchmark uses all
//! of it.

#![allow(dead_code)]

use dataflow_rs::{Engine, Message};
use datavalue::OwnedDataValue;
use futures::future::join_all;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Latencies, one per message, summarised as the three figures the gate
/// compares: the mean, P50 and P99.
pub struct Samples(Vec<Duration>);

impl Samples {
    pub fn with_capacity(n: usize) -> Self {
        Self(Vec::with_capacity(n))
    }

    pub fn push(&mut self, d: Duration) {
        self.0.push(d);
    }

    /// The mean, P50 and P99 in microseconds, each divided by `per`: the
    /// number of calls, sweeps or elements one message makes, so a row can
    /// report per call.
    pub fn summary(&mut self, per: usize) -> (f64, f64, f64) {
        assert!(!self.0.is_empty(), "no samples to summarise");
        self.0.sort_unstable();
        let n = self.0.len();
        let micros = |d: Duration| d.as_secs_f64() * 1e6 / per as f64;
        let at = |p: usize| self.0[(n * p / 100).min(n - 1)];
        let mean = self.0.iter().sum::<Duration>() / n as u32;
        (micros(mean), micros(at(50)), micros(at(99)))
    }

    /// Prints one row and returns its mean, P50 and P99.
    pub fn row(&mut self, label: &str, per: usize, unit: &str) -> (f64, f64, f64) {
        let (mean, p50, p99) = self.summary(per);
        println!("{label:<36} mean {mean:>10.2}   p50 {p50:>10.2}   p99 {p99:>10.2}   µs/{unit}");
        (mean, p50, p99)
    }
}

/// Runs `warmup` untimed messages, then `total` timed ones, each in its own
/// tokio task, as `realistic_benchmark` does. Returns the latencies and the
/// throughput in messages per second.
pub async fn spawned(
    engine: &Arc<Engine>,
    payload: &Arc<OwnedDataValue>,
    warmup: usize,
    total: usize,
) -> (Samples, f64) {
    let spawn = |count: usize| {
        (0..count)
            .map(|_| {
                let engine = Arc::clone(engine);
                let payload = Arc::clone(payload);
                tokio::spawn(async move {
                    let start = Instant::now();
                    let mut message = Message::new(payload);
                    engine.process_message(&mut message).await.unwrap();
                    start.elapsed()
                })
            })
            .collect::<Vec<_>>()
    };
    join_all(spawn(warmup)).await;

    let mut samples = Samples::with_capacity(total);
    let start = Instant::now();
    for latency in join_all(spawn(total)).await {
        samples.push(latency.expect("a benchmark task panicked"));
    }
    let throughput = total as f64 / start.elapsed().as_secs_f64();
    (samples, throughput)
}

/// One catalogue record: about 200 bytes of JSON in 14 nodes.
fn record(i: usize) -> Value {
    json!({
        "id": i,
        "sku": format!("SKU-{i:06}"),
        "name": format!("Catalogue item {i:06}, standard description"),
        "price": (i % 1000) as f64 + 0.99,
        "currency": "EUR",
        "tags": ["alpha", "beta", "gamma"],
        "attrs": { "color": "blue", "size": "M", "weight_g": i % 5000 }
    })
}

/// A deterministic document of about `bytes` bytes of JSON: catalogue records
/// under `"records"`. A row that needs a large context carries one, so the
/// row measures what each engine pays for bytes a message carries but does
/// not read.
pub fn large_document(bytes: usize) -> Value {
    let each = json_bytes(&record(0));
    json!({ "records": (0..bytes.div_ceil(each)).map(record).collect::<Vec<_>>() })
}

/// The size of `v` as compact JSON.
pub fn json_bytes(v: &Value) -> usize {
    serde_json::to_vec(v).expect("a document serializes").len()
}

/// The nodes in `v`, containers included. A view of a context costs per node,
/// a copy of it per node and per byte, so a header prints both.
pub fn json_nodes(v: &Value) -> usize {
    1 + match v {
        Value::Array(a) => a.iter().map(json_nodes).sum(),
        Value::Object(o) => o.values().map(json_nodes).sum(),
        _ => 0,
    }
}

/// The header line for a large context: its size in KiB and its node count.
pub fn describe(v: &Value) -> String {
    format!(
        "{} KiB of JSON in {}K nodes",
        json_bytes(v) / 1024,
        json_nodes(v) / 1000
    )
}
