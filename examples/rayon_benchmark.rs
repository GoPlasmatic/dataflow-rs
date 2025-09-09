use dataflow_rs::{Message, RayonEngine, Workflow};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const ITERATIONS: usize = 1000_000;
const WARMUP_ITERATIONS: usize = 1_000;

/// Stores latency measurements for analysis
#[derive(Clone)]
struct LatencyStats {
    measurements: Vec<Duration>,
}

impl LatencyStats {
    fn new() -> Self {
        Self {
            measurements: Vec::with_capacity(ITERATIONS),
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
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("RAYONENGINE ASYNC BENCHMARK");
    println!("============================");
    println!("Iterations: {}", ITERATIONS);
    println!("CPU cores: {}", num_cpus::get());
    println!();
    println!("Threads | Throughput | P50 (μs) | P90 (μs) | P95 (μs) | P99 (μs) | P99.9 (μs)");
    println!("--------|------------|----------|----------|----------|----------|------------");

    // Use same workflow as benchmark.rs
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
                                "path": "data.user.id", 
                                "logic": { "var": "temp_data.id" }
                            },
                            {
                                "path": "data.user.name", 
                                "logic": { "var": "temp_data.name" }
                            },
                            {
                                "path": "data.user.email", 
                                "logic": { "var": "temp_data.email" }
                            },
                            {
                                "path": "data.user.age",
                                "logic": { "+": [{ "var": "temp_data.age" }, 1] }
                            },
                            {
                                "path": "data.user.status",
                                "logic": { 
                                    "if": [
                                        { ">": [{ "var": "temp_data.age" }, 18] },
                                        "adult",
                                        "minor"
                                    ]
                                }
                            },
                            {
                                "path": "data.calculations.total",
                                "logic": {
                                    "*": [
                                        { "+": [{ "var": "temp_data.age" }, 10] },
                                        { "/": [{ "var": "temp_data.id" }, 100] }
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
                    "name": "validate",
                    "input": {
                        "rules": [
                            {
                                "path": "data",
                                "logic": { "!!": { "var": "data.user.id" } },
                                "message": "User ID is required"
                            },
                            {
                                "path": "data",
                                "logic": { "!!": { "var": "data.user.email" } },
                                "message": "User email is required"
                            },
                            {
                                "path": "data",
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
    let sample_data = json!({
        "id": 12345,
        "name": "John Doe",
        "email": "john.doe@example.com",
        "age": 25,
        "department": "Engineering"
    });

    // Test different thread configurations
    let thread_configs = vec![2, 4, 8, 10];

    for threads in thread_configs {
        let (throughput, mut latency) =
            run_async_benchmark(workflow.clone(), &sample_data, threads).await?;

        let (p50, p90, p95, p99, p999) = latency.calculate_percentiles();

        println!(
            "{:^7} | {:^10.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^8.0} | {:^10.0}",
            threads,
            throughput,
            p50.as_micros(),
            p90.as_micros(),
            p95.as_micros(),
            p99.as_micros(),
            p999.as_micros()
        );
    }

    Ok(())
}

async fn run_async_benchmark(
    workflow: Workflow,
    sample_data: &serde_json::Value,
    thread_count: usize,
) -> Result<(f64, LatencyStats), Box<dyn std::error::Error>> {
    let engine = Arc::new(RayonEngine::with_thread_count(
        vec![workflow],
        None,
        None,
        thread_count,
    ));

    let latencies = Arc::new(tokio::sync::Mutex::new(LatencyStats::new()));
    let success_count = Arc::new(AtomicUsize::new(0));

    // Use optimal number of async tasks (2x threads)
    let task_count = thread_count * 2;
    let messages_per_task = ITERATIONS / task_count;

    // Warm-up
    for _ in 0..WARMUP_ITERATIONS {
        let message = Message::new(&json!({}));
        let _ = engine.process_message(message).await;
    }

    let start = Instant::now();
    let mut handles = Vec::new();

    for task_id in 0..task_count {
        let engine = Arc::clone(&engine);
        let sample_data = sample_data.clone();
        let success = Arc::clone(&success_count);
        let latencies = Arc::clone(&latencies);

        let handle = tokio::spawn(async move {
            for i in 0..messages_per_task {
                let mut message = Message::new(&json!({}));
                message.temp_data = sample_data.clone();
                message.metadata = json!({
                    "iteration": task_id * messages_per_task + i
                });

                let msg_start = Instant::now();
                if engine.process_message(message).await.is_ok() {
                    success.fetch_add(1, Ordering::Relaxed);
                }
                latencies.lock().await.add(msg_start.elapsed());
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        handle.await?;
    }

    let elapsed = start.elapsed();
    let actual_messages = success_count.load(Ordering::Relaxed);
    let throughput = actual_messages as f64 / elapsed.as_secs_f64();

    Ok((throughput, latencies.lock().await.clone()))
}
