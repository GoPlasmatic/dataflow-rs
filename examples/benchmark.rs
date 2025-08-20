use dataflow_rs::{Engine, Workflow, engine::message::Message};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::path::Path;
use std::time::{Duration, Instant};

const BENCHMARK_LOG_FILE: &str = "benchmark_results.json";
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create the workflow engine (built-in functions are auto-registered)
    let mut engine = Engine::new();

    // Define a workflow that:
    // 1. Uses pre-loaded data instead of HTTP fetch
    // 2. Enriches the message with transformed data
    // 3. Demonstrates proper async workflow execution
    let workflow_json = r#"
    {
        "id": "benchmark_workflow",
        "name": "Benchmark Workflow Example",
        "description": "Demonstrates async workflow execution with data transformation",
        "condition": { "==": [true, true] },
        "tasks": [
            {
                "id": "transform_data",
                "name": "Transform Data",
                "description": "Map API response to our data model",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "data.user.id", 
                                "logic": { "var": "temp_data.body.id" }
                            },
                            {
                                "path": "data.user.name", 
                                "logic": { "var": "temp_data.body.name" }
                            },
                            {
                                "path": "data.user.email", 
                                "logic": { "var": "temp_data.body.email" }
                            },
                            {
                                "path": "data.user.address", 
                                "logic": {
                                    "cat": [
                                        { "var": "temp_data.body.address.street" },
                                        ", ",
                                        { "var": "temp_data.body.address.city" }
                                    ]
                                }
                            },
                            {
                                "path": "data.user.company", 
                                "logic": { "var": "temp_data.body.company.name" }
                            },
                            {
                                "path": "data.processed_at", 
                                "logic": { "cat": ["Processed at ", { "var": "metadata.timestamp" }] }
                            }
                        ]
                    }
                }
            }
        ]
    }
    "#;

    // Parse and add the workflow to the engine
    let workflow = Workflow::from_json(workflow_json)?;
    engine.add_workflow(&workflow);

    // Create sample user data (similar to what the HTTP endpoint would return)
    let sample_user_data = json!({
        "body": {
            "id": 1,
            "name": "Leanne Graham",
            "username": "Bret",
            "email": "Sincere@april.biz",
            "address": {
                "street": "Kulas Light",
                "suite": "Apt. 556",
                "city": "Gwenborough",
                "zipcode": "92998-3874",
                "geo": {
                    "lat": "-37.3159",
                    "lng": "81.1496"
                }
            },
            "phone": "1-770-736-8031 x56442",
            "website": "hildegard.org",
            "company": {
                "name": "Romaguera-Crona",
                "catchPhrase": "Multi-layered client-server neural-net",
                "bs": "harness real-time e-markets"
            }
        }
    });

    // Run async benchmark
    println!("=== ASYNC BENCHMARK ===");
    let async_results = run_async_benchmark(&engine, &sample_user_data, 1000).await?;

    // Run sync benchmark for comparison (using blocking approach)
    println!("\n=== SYNC BENCHMARK (for comparison) ===");
    let sync_results = run_sync_benchmark(&engine, &sample_user_data, 1000).await?;

    // Compare results
    println!("\n=== COMPARISON ===");
    println!("Async avg: {:?}", async_results.avg_time);
    println!("Sync avg:  {:?}", sync_results.avg_time);
    println!(
        "Async is {:.2}x {} than sync",
        if async_results.avg_time < sync_results.avg_time {
            sync_results.avg_time.as_nanos() as f64 / async_results.avg_time.as_nanos() as f64
        } else {
            async_results.avg_time.as_nanos() as f64 / sync_results.avg_time.as_nanos() as f64
        },
        if async_results.avg_time < sync_results.avg_time {
            "faster"
        } else {
            "slower"
        }
    );

    // Log results to file
    log_benchmark_results(
        async_results.iterations,
        async_results.min_time,
        async_results.max_time,
        async_results.avg_time,
        async_results.p95,
        async_results.p99,
        async_results.total_time,
        "async".to_string(),
    )?;

    log_benchmark_results(
        sync_results.iterations,
        sync_results.min_time,
        sync_results.max_time,
        sync_results.avg_time,
        sync_results.p95,
        sync_results.p99,
        sync_results.total_time,
        "sync".to_string(),
    )?;

    println!("\nBenchmark results saved to '{BENCHMARK_LOG_FILE}'");

    Ok(())
}

#[derive(Debug)]
struct BenchmarkResults {
    iterations: usize,
    min_time: Duration,
    max_time: Duration,
    avg_time: Duration,
    p95: Duration,
    p99: Duration,
    total_time: Duration,
}

async fn run_async_benchmark(
    engine: &Engine,
    sample_user_data: &Value,
    num_iterations: usize,
) -> Result<BenchmarkResults, Box<dyn std::error::Error>> {
    let mut total_duration = Duration::new(0, 0);
    let mut min_duration = Duration::new(u64::MAX, 0);
    let mut max_duration = Duration::new(0, 0);
    let mut all_durations = Vec::with_capacity(num_iterations);
    let mut error_count = 0;

    println!("Starting async benchmark with {num_iterations} iterations...");

    for i in 0..num_iterations {
        let mut message = Message::new(&json!({}));
        message.temp_data = sample_user_data.clone();
        message.data = json!({});
        message.metadata = json!({
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "iteration": i
        });

        let start = Instant::now();
        match engine.process_message(&mut message).await {
            Ok(_) => {
                let duration = start.elapsed();
                all_durations.push(duration);
                total_duration += duration;
                min_duration = min_duration.min(duration);
                max_duration = max_duration.max(duration);

                // Check for processing errors
                if message.has_errors() {
                    error_count += 1;
                    if error_count <= 5 {
                        // Only print first 5 errors
                        println!("Processing errors in iteration {}: {:?}", i, message.errors);
                    }
                }
            }
            Err(e) => {
                error_count += 1;
                if error_count <= 5 {
                    println!("Error in iteration {i}: {e:?}");
                }
                // Still record the time even for errors
                let duration = start.elapsed();
                all_durations.push(duration);
                total_duration += duration;
                min_duration = min_duration.min(duration);
                max_duration = max_duration.max(duration);
            }
        }

        if (i + 1) % 1000 == 0 {
            println!("Completed {} async iterations", i + 1);
        }
    }

    if error_count > 0 {
        println!("Total errors encountered: {error_count}");
    }

    // Sort durations for percentile calculations
    all_durations.sort();

    let p95_idx = (num_iterations as f64 * 0.95) as usize;
    let p99_idx = (num_iterations as f64 * 0.99) as usize;
    let p95 = all_durations.get(p95_idx).unwrap_or(&Duration::ZERO);
    let p99 = all_durations.get(p99_idx).unwrap_or(&Duration::ZERO);
    let avg_duration = total_duration / num_iterations as u32;

    println!("\nAsync Benchmark Results (v{VERSION}):");
    println!("  Iterations: {num_iterations}");
    println!("  Errors: {error_count}");
    println!("  Min time: {min_duration:?}");
    println!("  Max time: {max_duration:?}");
    println!("  Avg time: {avg_duration:?}");
    println!("  95th percentile: {p95:?}");
    println!("  99th percentile: {p99:?}");
    println!("  Total time: {total_duration:?}");

    Ok(BenchmarkResults {
        iterations: num_iterations,
        min_time: min_duration,
        max_time: max_duration,
        avg_time: avg_duration,
        p95: *p95,
        p99: *p99,
        total_time: total_duration,
    })
}

async fn run_sync_benchmark(
    engine: &Engine,
    sample_user_data: &Value,
    num_iterations: usize,
) -> Result<BenchmarkResults, Box<dyn std::error::Error>> {
    let mut total_duration = Duration::new(0, 0);
    let mut min_duration = Duration::new(u64::MAX, 0);
    let mut max_duration = Duration::new(0, 0);
    let mut all_durations = Vec::with_capacity(num_iterations);
    let mut error_count = 0;

    println!("Starting sync-style benchmark with {num_iterations} iterations...");

    for i in 0..num_iterations {
        let mut message = Message::new(&json!({}));
        message.temp_data = sample_user_data.clone();
        message.data = json!({});
        message.metadata = json!({
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "iteration": i
        });

        let start = Instant::now();
        // Use tokio::task::block_in_place to simulate sync behavior
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(engine.process_message(&mut message))
        });

        match result {
            Ok(_) => {
                let duration = start.elapsed();
                all_durations.push(duration);
                total_duration += duration;
                min_duration = min_duration.min(duration);
                max_duration = max_duration.max(duration);

                if message.has_errors() {
                    error_count += 1;
                }
            }
            Err(e) => {
                error_count += 1;
                if error_count <= 5 {
                    println!("Sync error in iteration {i}: {e:?}");
                }
                let duration = start.elapsed();
                all_durations.push(duration);
                total_duration += duration;
                min_duration = min_duration.min(duration);
                max_duration = max_duration.max(duration);
            }
        }

        if (i + 1) % 1000 == 0 {
            println!("Completed {} sync iterations", i + 1);
        }
    }

    if error_count > 0 {
        println!("Total sync errors encountered: {error_count}");
    }

    all_durations.sort();

    let p95_idx = (num_iterations as f64 * 0.95) as usize;
    let p99_idx = (num_iterations as f64 * 0.99) as usize;
    let p95 = all_durations.get(p95_idx).unwrap_or(&Duration::ZERO);
    let p99 = all_durations.get(p99_idx).unwrap_or(&Duration::ZERO);
    let avg_duration = total_duration / num_iterations as u32;

    println!("\nSync Benchmark Results (v{VERSION}):");
    println!("  Iterations: {num_iterations}");
    println!("  Errors: {error_count}");
    println!("  Min time: {min_duration:?}");
    println!("  Max time: {max_duration:?}");
    println!("  Avg time: {avg_duration:?}");
    println!("  95th percentile: {p95:?}");
    println!("  99th percentile: {p99:?}");
    println!("  Total time: {total_duration:?}");

    Ok(BenchmarkResults {
        iterations: num_iterations,
        min_time: min_duration,
        max_time: max_duration,
        avg_time: avg_duration,
        p95: *p95,
        p99: *p99,
        total_time: total_duration,
    })
}

fn log_benchmark_results(
    iterations: usize,
    min_time: Duration,
    max_time: Duration,
    avg_time: Duration,
    p95: Duration,
    p99: Duration,
    total_time: Duration,
    benchmark_type: String,
) -> io::Result<()> {
    let mut benchmark_data = read_benchmark_file().unwrap_or_else(|_| json!({}));

    let benchmark_entry = json!({
        "iterations": iterations,
        "min_time_ns": min_time.as_nanos(),
        "max_time_ns": max_time.as_nanos(),
        "avg_time_ns": avg_time.as_nanos(),
        "p95_ns": p95.as_nanos(),
        "p99_ns": p99.as_nanos(),
        "total_time_ns": total_time.as_nanos(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "benchmark_type": benchmark_type,
    });

    let version_key = format!("{VERSION}_{benchmark_type}");

    if let Some(obj) = benchmark_data.as_object_mut() {
        obj.insert(version_key, benchmark_entry);
    }

    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(BENCHMARK_LOG_FILE)?;

    serde_json::to_writer_pretty(file, &benchmark_data)?;

    Ok(())
}

fn read_benchmark_file() -> io::Result<Value> {
    if !Path::new(BENCHMARK_LOG_FILE).exists() {
        return Ok(json!({}));
    }

    let mut file = File::open(BENCHMARK_LOG_FILE)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    match serde_json::from_str(&contents) {
        Ok(value) => Ok(value),
        Err(e) => {
            eprintln!("Warning: Could not parse benchmark file: {e}");
            Ok(json!({}))
        }
    }
}
