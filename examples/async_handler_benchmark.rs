//! # Async Handler Benchmark
//!
//! Measures the marginal cost of one custom host function task on top of a
//! small sync-builtin pipeline. The delta isolates the framework overhead of
//! one host call: the boxed call future, the `Ctx` over the run's view, the
//! call's overlay and `Change`s, the settle into the run, and the audit
//! entry.
//!
//! There is **no I/O** in the function (no `tokio::time::sleep`, no
//! network) so the reported number reflects pure framework cost — not
//! external latency. To put the dispatch overhead in real-world context,
//! production handlers usually do tens to hundreds of microseconds of
//! actual work, which dwarfs the framework overhead measured here.
//!
//! Two configurations run back-to-back on the same payload:
//! - **Sync only**       — `parse_json` + `map` (3 mappings) + `validation` (2 rules)
//! - **Sync + custom**   — same + 1 custom `Echo` task at the end
//!
//! This is v4's port of v3's `async_handler_benchmark`: the same
//! definitions, payload, message counts and concurrency, run through
//! dataflow-rs 4, with v3's `EchoHandler` as a v4 `Function`.
//!
//! Run with: `cargo run --example async_handler_benchmark --release`

use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::Output;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::{Ctx, Engine, Function, Message, Outcome, Result};
use futures::future::join_all;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[path = "shared/gate.rs"]
mod gate;

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

/// The echo function's config, compiled once at build: the target is a
/// literal, so it folds to a path then.
#[derive(FunctionConfig)]
struct EchoConfig {
    #[output(default = "data.echoed")]
    target: Output,
    value: String,
}

/// Minimal function — one write to exercise the overlay and its settle,
/// then `Ok`. No `.await` work; we want to measure dispatch overhead, not
/// external latency.
struct Echo;

impl Function for Echo {
    type Config = EchoConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &EchoConfig) -> Result<Outcome> {
        // The literal target is lent, not evaluated.
        ctx.emit(&cfg.target, OwnedDataValue::String(cfg.value.clone()))?;
        Ok(Outcome::Ok)
    }
}

/// Common task list — three transformation mappings + two validations,
/// fed by `parse_json`. Kept short so a single host function task is a
/// large fraction of total work and the dispatch overhead is visible
/// in the delta. Append the `echo` task for the with-custom
/// configuration.
fn task_json(include_custom: bool) -> String {
    let mut tasks = String::from(
        r#"
        {
            "id": "load_payload",
            "name": "Parse Payload",
            "function": {
                "name": "parse_json",
                "input": { "source": "payload", "target": "input" }
            }
        },
        {
            "id": "transform",
            "name": "Transform",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        { "path": "data.user_id",  "logic": { "var": "data.input.user_id" } },
                        { "path": "data.amount",   "logic": { "var": "data.input.amount" } },
                        { "path": "data.currency", "logic": { "var": "data.input.currency" } }
                    ]
                }
            }
        },
        {
            "id": "validate",
            "name": "Validate",
            "function": {
                "name": "validation",
                "input": {
                    "rules": [
                        { "logic": { "!!": { "var": "data.user_id" } }, "message": "user_id required" },
                        { "logic": { ">":  [{ "var": "data.amount" }, 0] }, "message": "amount must be positive" }
                    ]
                }
            }
        }"#,
    );
    if include_custom {
        tasks.push_str(
            r#",
        {
            "id": "echo",
            "name": "Echo",
            "function": {
                "name": "echo",
                "input": { "target": "data.echoed", "value": "ok" }
            }
        }"#,
        );
    }
    tasks
}

fn build_workflow(include_custom: bool) -> Value {
    let id = if include_custom {
        "with_custom"
    } else {
        "sync_only"
    };
    let json = format!(
        r#"{{ "id": "{}", "name": "{}", "tasks": [ {} ] }}"#,
        id,
        id,
        task_json(include_custom)
    );
    serde_json::from_str(&json).expect("workflow parses")
}

fn build_payload() -> serde_json::Value {
    json!({
        "user_id": "user_42",
        "amount": 1500.50,
        "currency": "EUR",
        "metadata": { "source": "benchmark" }
    })
}

/// Runs one message and checks the workload did its work before anything
/// is timed: the mappings landed, both rules passed, and with the custom
/// task, its write landed and it has its audit entry.
async fn check(engine: &Engine, payload: &Arc<OwnedDataValue>, include_custom: bool) {
    let mut message = Message::new(Arc::clone(payload));
    engine
        .run(&mut message)
        .await
        .into_result()
        .expect("the workflow runs");
    let data = Value::from(message.context().data());
    assert!(
        message.errors().is_empty(),
        "errors: {:?}",
        message.errors()
    );
    assert_eq!(data["user_id"], json!("user_42"), "the mappings landed");
    assert_eq!(data["amount"], json!(1500.5), "the mappings landed");
    let tasks = if include_custom { 4 } else { 3 };
    assert_eq!(message.audit().len(), tasks, "one audit entry per task");
    if include_custom {
        assert_eq!(data["echoed"], json!("ok"), "the echo's write landed");
    } else {
        assert!(
            data.get("echoed").is_none(),
            "no echo without the custom task"
        );
    }
}

/// Run a benchmark over `total` messages against `engine`. Returns the
/// observed throughput and per-message latency stats.
async fn run_benchmark(
    label: &str,
    ops_per_msg: usize,
    engine: Engine,
    payload: Arc<OwnedDataValue>,
) -> (
    f64,
    Duration,
    (Duration, Duration, Duration, Duration, Duration),
) {
    println!("[{label}] Warmup ({} messages)...", WARMUP_MESSAGES);
    let warmup_handles: Vec<_> = (0..WARMUP_MESSAGES)
        .map(|_| {
            let engine = engine.clone();
            let payload = Arc::clone(&payload);
            tokio::spawn(async move {
                let mut message = Message::new(payload);
                engine.run(&mut message).await.into_result().unwrap();
            })
        })
        .collect();
    join_all(warmup_handles).await;

    let mut latency_stats = LatencyStats::new();
    let benchmark_start = Instant::now();
    let mut handles = Vec::with_capacity(TOTAL_MESSAGES);

    for _ in 0..TOTAL_MESSAGES {
        let engine = engine.clone();
        let payload = Arc::clone(&payload);
        handles.push(tokio::spawn(async move {
            let msg_start = Instant::now();
            let mut message = Message::new(payload);
            engine.run(&mut message).await.into_result().unwrap();
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
    let pcts = latency_stats.percentiles();
    println!(
        "[{label}] Done: {:.0} msg/s @ {} ops/msg ({} total ops/s)",
        throughput,
        ops_per_msg,
        format_int(throughput as u64 * ops_per_msg as u64)
    );
    (throughput, avg, pcts)
}

fn format_int(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!("ASYNC HANDLER BENCHMARK (dispatch overhead delta)");
    println!("=================================================");
    println!("Total messages per config: {}", TOTAL_MESSAGES);
    println!("CPU cores: {}", num_cpus::get());
    println!("Tokio worker threads: {}", num_cpus::get());
    println!();

    let payload_json = build_payload();
    let payload: Arc<OwnedDataValue> = Arc::new(OwnedDataValue::from(&payload_json));

    // Baseline: sync-only stretch (parse_json + map + validation = 6 ops).
    let sync_engine = Engine::builder()
        .workflow_json(&build_workflow(false))
        .build()
        .expect("the engine builds");
    check(&sync_engine, &payload, false).await;
    let (sync_tp, sync_avg, sync_pcts) =
        run_benchmark("sync only", 6, sync_engine, Arc::clone(&payload)).await;

    println!();

    // With custom: same + 1 host call to `Echo` (its `Ctx`, one write into
    // its overlay, the settle into the run).
    let async_engine = Engine::builder()
        .workflow_json(&build_workflow(true))
        .register("echo", Echo)
        .build()
        .expect("the engine builds");
    check(&async_engine, &payload, true).await;
    let (async_tp, async_avg, async_pcts) =
        run_benchmark("sync + custom", 7, async_engine, payload).await;

    // Report
    println!();
    println!(
        "Configuration | Throughput (msg/s) | Avg (μs) | P50 (μs) | P90 (μs) | P95 (μs) | P99 (μs) | P99.9 (μs)"
    );
    println!(
        "--------------|-------------------|----------|----------|----------|----------|----------|------------"
    );
    println!(
        "{:^13} | {:^17.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^10.0}",
        "Sync only",
        sync_tp,
        sync_avg.as_micros(),
        sync_pcts.0.as_micros(),
        sync_pcts.1.as_micros(),
        sync_pcts.2.as_micros(),
        sync_pcts.3.as_micros(),
        sync_pcts.4.as_micros()
    );
    println!(
        "{:^13} | {:^17.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^10.0}",
        "+ custom",
        async_tp,
        async_avg.as_micros(),
        async_pcts.0.as_micros(),
        async_pcts.1.as_micros(),
        async_pcts.2.as_micros(),
        async_pcts.3.as_micros(),
        async_pcts.4.as_micros()
    );

    // Marginal cost — average latency delta divided by 1 (one extra task).
    let avg_us = sync_avg.as_secs_f64() * 1_000_000.0;
    let async_us = async_avg.as_secs_f64() * 1_000_000.0;
    let delta_us = async_us - avg_us;
    println!();
    println!(
        "Per-message marginal cost of 1 custom handler dispatch: {:+.2} μs (avg latency)",
        delta_us
    );
    let tp_delta_pct = (async_tp - sync_tp) / sync_tp * 100.0;
    println!(
        "Throughput change vs sync-only: {:+.1}% ({:+.0} msg/s)",
        tp_delta_pct,
        async_tp - sync_tp
    );

    println!("\n✅ Benchmark complete!");

    let micros = |d: Duration| d.as_secs_f64() * 1e6;
    gate::Gate::new("async_handler_benchmark")
        .higher("sync_only.throughput", sync_tp, "msg/s")
        .lower("sync_only.mean", avg_us, "µs")
        .lower("sync_only.p99", micros(sync_pcts.3), "µs")
        .higher("with_custom.throughput", async_tp, "msg/s")
        .lower("with_custom.mean", async_us, "µs")
        .lower("with_custom.p99", micros(async_pcts.3), "µs")
        .lower("marginal", delta_us, "µs")
        .emit();
    Ok(())
}
