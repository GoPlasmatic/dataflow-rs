use dataflow_rs::{Engine, Workflow, engine::message::Message};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;

const BENCHMARK_LOG_FILE: &str = "benchmark_results.json";
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("========================================");
    println!("DATAFLOW ENGINE BENCHMARK");
    println!("========================================\n");

    // Define a workflow that:
    // 1. Uses pre-loaded data instead of HTTP fetch
    // 2. Enriches the message with transformed data
    // 3. Demonstrates proper async workflow execution
    let workflow_json = r#"
    {
        "id": "benchmark_workflow",
        "name": "Benchmark Workflow Example",
        "description": "Demonstrates async workflow execution with data transformation",
        "priority": 1,
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

    // Parse the workflow
    let workflow = Workflow::from_json(workflow_json)?;

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

    let iterations = 100000;

    println!("Testing with {} iterations\n", iterations);

    // Test sequential performance with concurrency 1
    println!("--- Sequential Performance (Baseline) ---");
    println!("Concurrency | Avg Time per Message | Total Time | Messages/sec");
    println!("------------|---------------------|------------|-------------");

    // Sequential baseline
    let engine = Arc::new(Engine::with_concurrency(1));
    engine.add_workflow(&workflow);

    let seq_results = run_sequential_benchmark(&*engine, &sample_user_data, iterations).await?;
    let throughput = (iterations as f64) / seq_results.total_time.as_secs_f64();

    println!(
        "{:^11} | {:>19.3}μs | {:>10.2}ms | {:>12.0}",
        1,
        seq_results.avg_time.as_secs_f64() * 1_000_000.0,
        seq_results.total_time.as_secs_f64() * 1000.0,
        throughput
    );

    log_benchmark_results(
        iterations,
        seq_results.min_time,
        seq_results.max_time,
        seq_results.avg_time,
        seq_results.p95,
        seq_results.p99,
        seq_results.total_time,
        format!("seq_x1"),
    )?;

    // Test concurrent performance
    println!("\n--- Concurrent Performance ---");
    println!("Concurrency | Avg Time per Message | Total Time | Messages/sec | Speedup");
    println!("------------|---------------------|------------|--------------|--------");

    // Test configurations with different concurrency levels
    let test_configs = vec![
        1, 16,  // 16 concurrent messages
        32,  // 32 concurrent messages
        64,  // 64 concurrent messages
        128, // 128 concurrent messages
    ];

    let baseline_throughput = throughput;

    for concurrency in test_configs {
        let engine = Arc::new(Engine::with_concurrency(concurrency));
        engine.add_workflow(&workflow);

        let con_results = run_concurrent_benchmark(
            engine.clone(),
            &sample_user_data,
            iterations,
            concurrency, // Use same value for concurrent tasks
        )
        .await?;

        let throughput = (iterations as f64) / con_results.total_time.as_secs_f64();
        let speedup = throughput / baseline_throughput;

        println!(
            "{:^11} | {:>19.3}μs | {:>10.2}ms | {:>12.0} | {:>7.2}x",
            concurrency,
            con_results.avg_time.as_secs_f64() * 1_000_000.0,
            con_results.total_time.as_secs_f64() * 1000.0,
            throughput,
            speedup
        );

        log_benchmark_results(
            iterations,
            con_results.min_time,
            con_results.max_time,
            con_results.avg_time,
            con_results.p95,
            con_results.p99,
            con_results.total_time,
            format!("con_x{}", concurrency),
        )?;
    }

    println!("\n========================================");
    println!("Benchmark results saved to '{}'", BENCHMARK_LOG_FILE);
    println!("========================================");

    Ok(())
}

#[derive(Debug)]
struct BenchmarkResults {
    min_time: Duration,
    max_time: Duration,
    avg_time: Duration,
    p95: Duration,
    p99: Duration,
    total_time: Duration,
}

async fn run_sequential_benchmark(
    engine: &Engine,
    sample_user_data: &Value,
    num_iterations: usize,
) -> Result<BenchmarkResults, Box<dyn std::error::Error>> {
    let mut total_duration = Duration::new(0, 0);
    let mut min_duration = Duration::new(u64::MAX, 0);
    let mut max_duration = Duration::new(0, 0);
    let mut all_durations = Vec::with_capacity(num_iterations);
    let mut error_count = 0;

    // Sequential processing - one message at a time
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

    Ok(BenchmarkResults {
        min_time: min_duration,
        max_time: max_duration,
        avg_time: avg_duration,
        p95: *p95,
        p99: *p99,
        total_time: total_duration,
    })
}

async fn run_concurrent_benchmark(
    engine: Arc<Engine>,
    sample_user_data: &Value,
    num_iterations: usize,
    concurrent_tasks: usize,
) -> Result<BenchmarkResults, Box<dyn std::error::Error>> {
    let start_time = Instant::now();
    let mut all_durations = Vec::with_capacity(num_iterations);
    let mut error_count = 0;

    // Concurrent processing using JoinSet
    let mut tasks = JoinSet::new();

    for i in 0..num_iterations {
        let engine_clone = engine.clone();
        let data_clone = sample_user_data.clone();

        // Spawn concurrent tasks
        tasks.spawn(async move {
            let mut message = Message::new(&json!({}));
            message.temp_data = data_clone;
            message.data = json!({});
            message.metadata = json!({
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "iteration": i
            });

            let msg_start = Instant::now();
            let result = engine_clone.process_message(&mut message).await;
            let duration = msg_start.elapsed();

            (duration, result.is_ok(), message.has_errors())
        });

        // Limit concurrent tasks
        while tasks.len() >= concurrent_tasks {
            // Wait for at least one task to complete
            if let Some(Ok((duration, ok, has_errors))) = tasks.join_next().await {
                all_durations.push(duration);
                if !ok || has_errors {
                    error_count += 1;
                }
            }
        }
    }

    // Wait for remaining tasks
    while let Some(Ok((duration, ok, has_errors))) = tasks.join_next().await {
        all_durations.push(duration);
        if !ok || has_errors {
            error_count += 1;
        }
    }

    let total_time = start_time.elapsed();

    if error_count > 0 {
        println!("Total errors encountered: {error_count}");
    }

    // Calculate statistics
    all_durations.sort();
    let min_duration = *all_durations.first().unwrap_or(&Duration::ZERO);
    let max_duration = *all_durations.last().unwrap_or(&Duration::ZERO);
    let sum: Duration = all_durations.iter().sum();
    let avg_duration = sum / all_durations.len() as u32;

    let p95_idx = (all_durations.len() as f64 * 0.95) as usize;
    let p99_idx = (all_durations.len() as f64 * 0.99) as usize;
    let p95 = *all_durations.get(p95_idx).unwrap_or(&Duration::ZERO);
    let p99 = *all_durations.get(p99_idx).unwrap_or(&Duration::ZERO);

    Ok(BenchmarkResults {
        min_time: min_duration,
        max_time: max_duration,
        avg_time: avg_duration,
        p95,
        p99,
        total_time,
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

    let version_key = format!("{}_{}", VERSION, benchmark_type);

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
