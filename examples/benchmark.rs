//! # Async Throughput Benchmark
//!
//! This benchmark demonstrates the performance of the async dataflow-rs engine
//! using Tokio's multi-threaded runtime. It measures throughput and latency
//! percentiles when processing messages concurrently.
//!
//! Run with: `cargo run --example benchmark --release`

use dataflow_rs::{Engine, Message, Workflow};
use futures::future::join_all;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TOTAL_MESSAGES: usize = 1_000_000;
const WARMUP_MESSAGES: usize = 10_000;

/// Stores latency measurements for analysis
struct LatencyStats {
    measurements: Vec<Duration>,
}

impl LatencyStats {
    fn new() -> Self {
        Self {
            measurements: Vec::with_capacity(TOTAL_MESSAGES),
        }
    }

    fn add(&mut self, duration: Duration) {
        self.measurements.push(duration);
    }

    fn calculate_percentiles(&mut self) -> (Duration, Duration, Duration, Duration, Duration) {
        self.measurements.sort_unstable();
        let len = self.measurements.len();
        if len == 0 {
            return (
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
            );
        }

        let p50 = self.measurements[len * 50 / 100];
        let p90 = self.measurements[len * 90 / 100];
        let p95 = self.measurements[len * 95 / 100];
        let p99 = self.measurements[len * 99 / 100];
        let p999 = self.measurements[std::cmp::min(len * 999 / 1000, len - 1)];

        (p50, p90, p95, p99, p999)
    }

    fn calculate_average(&self) -> Duration {
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

    println!("ASYNC ENGINE BENCHMARK (Tokio Multi-threaded Runtime)");
    println!("=====================================================");
    println!("Total messages: {}", TOTAL_MESSAGES);
    println!("CPU cores: {}", num_cpus::get());
    println!("Tokio worker threads: {}", num_cpus::get());
    println!();

    // Create workflow
    let workflow_json = r#"
    {
        "id": "benchmark_workflow",
        "name": "Benchmark Workflow",
        "tasks": [
            {
                "id": "transform_data",
                "name": "Transform Data",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "user.id", 
                                "logic": { "var": "payload.input.id" }
                            },
                            {
                                "path": "user.name", 
                                "logic": { "var": "payload.input.name" }
                            },
                            {
                                "path": "user.email", 
                                "logic": { "var": "payload.input.email" }
                            },
                            {
                                "path": "user.age",
                                "logic": { "+": [{ "var": "payload.input.age" }, 1] }
                            },
                            {
                                "path": "user.status",
                                "logic": { 
                                    "if": [
                                        { ">": [{ "var": "payload.input.age" }, 18] },
                                        "adult",
                                        "minor"
                                    ]
                                }
                            },
                            {
                                "path": "calculations.total",
                                "logic": {
                                    "*": [
                                        { "+": [{ "var": "payload.input.age" }, 10] },
                                        { "/": [{ "var": "payload.input.id" }, 100] }
                                    ]
                                }
                            }
                        ]
                    }
                }
            },
            {
                "id": "validate_data",
                "name": "Validate Data",
                "function": {
                    "name": "validation",
                    "input": {
                        "rules": [
                            {
                                "path": "user.id",
                                "logic": { "!!": { "var": "data.user.id" } },
                                "message": "User ID is required"
                            },
                            {
                                "path": "user.email",
                                "logic": { "!!": { "var": "data.user.email" } },
                                "message": "User email is required"
                            },
                            {
                                "path": "calculations.total",
                                "logic": { ">": [{ "var": "data.calculations.total" }, 0] },
                                "message": "Total must be positive"
                            }
                        ]
                    }
                }
            }
        ]
    }
    "#;

    let workflow = Workflow::from_json(workflow_json)?;
    let engine = Arc::new(Engine::new(vec![workflow], None));

    let sample_data = json!({
        "input": {
            "id": 12345,
            "name": "John Doe",
            "email": "john.doe@example.com",
            "age": 25,
            "department": "Engineering"
        }
    });

    // Warmup phase
    println!("Running warmup ({} messages)...", WARMUP_MESSAGES);
    let warmup_start = Instant::now();
    let warmup_handles: Vec<_> = (0..WARMUP_MESSAGES)
        .map(|_| {
            let engine = Arc::clone(&engine);
            let data = sample_data.clone();
            tokio::spawn(async move {
                let mut message = Message::new(&data);
                engine.process_message(&mut message).await.unwrap();
            })
        })
        .collect();

    join_all(warmup_handles).await;
    println!("Warmup completed in {:?}\n", warmup_start.elapsed());

    // Benchmark different concurrency levels
    println!(
        "Configuration | Messages | Concurrency | Throughput (msg/s) | Avg (μs) | P50 (μs) | P90 (μs) | P95 (μs) | P99 (μs) | P99.9 (μs)"
    );
    println!(
        "--------------|----------|-------------|-------------------|----------|----------|----------|----------|----------|------------"
    );

    let mut latency_stats = LatencyStats::new();

    let benchmark_start = Instant::now();
    let mut handles = Vec::with_capacity(TOTAL_MESSAGES);

    for i in 0..TOTAL_MESSAGES {
        let engine = Arc::clone(&engine);
        let data = sample_data.clone();

        let handle = tokio::spawn(async move {
            let msg_start = Instant::now();

            let mut message = Message::new(&data);
            message.metadata = json!({ "iteration": i });

            engine.process_message(&mut message).await.unwrap();

            msg_start.elapsed()
        });

        handles.push(handle);
    }

    // Collect all results
    let latencies = join_all(handles).await;
    for result in latencies {
        if let Ok(duration) = result {
            latency_stats.add(duration);
        }
    }

    let total_time = benchmark_start.elapsed();
    let throughput = TOTAL_MESSAGES as f64 / total_time.as_secs_f64();
    let avg = latency_stats.calculate_average();
    let (p50, p90, p95, p99, p999) = latency_stats.calculate_percentiles();

    println!(
        "{:^13} | {:^8} | {:^11} | {:^17.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^10.0}",
        "Async",
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

    println!("\n✅ Benchmark complete!");

    Ok(())
}
