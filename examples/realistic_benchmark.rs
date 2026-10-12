//! # Realistic Throughput Benchmark
//!
//! Mirrors the shape of a real dataflow-rs workload: an ISO 20022 pacs.008-
//! style inbound payload (4–5 levels deep, ~80 fields), transformed into a
//! SwiftMT-103-style outbound shape via ~25 map mappings, then validated
//! against ~12 rules. The map task exercises a realistic mix of operators:
//! `var`, `cat`, `if`, arithmetic, `!!`, `==`, `substr`, `length`.
//!
//! Compare numbers against `benchmark.rs` (the parse + 9-eval trivial workload) to
//! see how the per-message framework overhead (UUID, audit drop, message
//! construction) reweights when real eval work is added.
//!
//! Run with: `cargo run --example realistic_benchmark --release`

use dataflow_rs::{Engine, Message};
use datavalue::OwnedDataValue;
use futures::future::join_all;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[path = "shared/iso20022.rs"]
mod iso20022;

const TOTAL_MESSAGES: usize = 500_000;
const WARMUP_MESSAGES: usize = 5_000;

struct LatencyStats {
    measurements: Vec<Duration>,
}

impl LatencyStats {
    fn new() -> Self {
        Self {
            measurements: Vec::with_capacity(TOTAL_MESSAGES),
        }
    }

    fn add(&mut self, d: Duration) {
        self.measurements.push(d);
    }

    fn percentiles(&mut self) -> (Duration, Duration, Duration, Duration, Duration) {
        self.measurements.sort_unstable();
        let n = self.measurements.len();
        if n == 0 {
            let z = Duration::ZERO;
            return (z, z, z, z, z);
        }
        (
            self.measurements[n * 50 / 100],
            self.measurements[n * 90 / 100],
            self.measurements[n * 95 / 100],
            self.measurements[n * 99 / 100],
            self.measurements[std::cmp::min(n * 999 / 1000, n - 1)],
        )
    }

    fn average(&self) -> Duration {
        if self.measurements.is_empty() {
            return Duration::ZERO;
        }
        let sum: Duration = self.measurements.iter().sum();
        sum / self.measurements.len() as u32
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!("REALISTIC ENGINE BENCHMARK (ISO 20022 -> SwiftMT-shaped workload)");
    println!("=================================================================");
    println!("Total messages: {}", TOTAL_MESSAGES);
    println!("CPU cores: {}", num_cpus::get());
    println!("Tokio worker threads: {}", num_cpus::get());
    println!();

    // -- Workflow: realistic transformation + validation ----------------------
    // 1 parse_json (payload -> data.input)
    // 1 map task: 25 mappings exercising var/cat/if/arithmetic/substr/length
    // 1 validation task: 12 rules
    let workflow = iso20022::workflow();

    let engine = Arc::new(Engine::builder().with_workflow(workflow).build().unwrap());

    // -- Sample payload: pacs.008-shaped, ~80 fields, 4-5 levels deep ---------
    // Convert to `OwnedDataValue` ONCE at startup and share via `Arc`. Per-
    // message construction goes through `Message::new(Arc::clone(...))`
    // — a refcount bump, no serde_json clone or `OwnedDataValue::from(&Value)`
    // walk. This is what callers with already-parsed inputs (e.g. an HTTP
    // server holding parsed payloads) actually pay; the prior
    // `Message::from_value(&data)` path measured the harness's serde_json
    // churn as part of the engine cost.
    let sample_payload_json = iso20022::payload();
    let sample_payload: Arc<OwnedDataValue> = Arc::new(OwnedDataValue::from(&sample_payload_json));

    // Check the workload once before timing it: one that silently does less
    // than it says still reports a plausible number.
    let mut checked = Message::new(Arc::clone(&sample_payload));
    engine.process_message(&mut checked).await.unwrap();
    iso20022::check(&checked);

    // Warmup
    println!("Running warmup ({} messages)...", WARMUP_MESSAGES);
    let warmup_start = Instant::now();
    let warmup_handles: Vec<_> = (0..WARMUP_MESSAGES)
        .map(|_| {
            let engine = Arc::clone(&engine);
            let payload = Arc::clone(&sample_payload);
            tokio::spawn(async move {
                let mut message = Message::new(payload);
                engine.process_message(&mut message).await.unwrap();
            })
        })
        .collect();
    join_all(warmup_handles).await;
    println!("Warmup completed in {:?}\n", warmup_start.elapsed());

    println!("Workload per message: 1 parse_json + 25 mappings + 12 validations = 38 ops");
    println!();
    println!(
        "Configuration | Messages | Concurrency | Throughput (msg/s) | Avg (μs) | P50 (μs) | P90 (μs) | P95 (μs) | P99 (μs) | P99.9 (μs)"
    );
    println!(
        "--------------|----------|-------------|-------------------|----------|----------|----------|----------|----------|------------"
    );

    let mut latency_stats = LatencyStats::new();
    let benchmark_start = Instant::now();
    let mut handles = Vec::with_capacity(TOTAL_MESSAGES);

    for _ in 0..TOTAL_MESSAGES {
        let engine = Arc::clone(&engine);
        let payload = Arc::clone(&sample_payload);
        handles.push(tokio::spawn(async move {
            let msg_start = Instant::now();
            let mut message = Message::new(payload);
            engine.process_message(&mut message).await.unwrap();
            msg_start.elapsed()
        }));
    }

    let latencies = join_all(handles).await;
    for d in latencies.into_iter().flatten() {
        latency_stats.add(d);
    }

    let total_time = benchmark_start.elapsed();
    let throughput = TOTAL_MESSAGES as f64 / total_time.as_secs_f64();
    let avg = latency_stats.average();
    let (p50, p90, p95, p99, p999) = latency_stats.percentiles();

    println!(
        "{:^13} | {:^8} | {:^11} | {:^17.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^10.0}",
        "Realistic",
        TOTAL_MESSAGES,
        "Unlimited",
        throughput,
        avg.as_micros(),
        p50.as_micros(),
        p90.as_micros(),
        p95.as_micros(),
        p99.as_micros(),
        p999.as_micros()
    );

    println!();
    println!(
        "Throughput per JSONLogic op: {:.0} ops/sec",
        throughput * 38.0
    );
    println!("\n✅ Benchmark complete!");

    Ok(())
}
