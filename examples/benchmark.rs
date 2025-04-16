use dataflow_rs::{engine::message::Message, Engine, Workflow};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::path::Path;
use std::time::{Duration, Instant};

const BENCHMARK_LOG_FILE: &str = "benchmark_results.json";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create the workflow engine (built-in functions are auto-registered)
    let mut engine = Engine::new();

    // Define a workflow that:
    // 1. Uses pre-loaded data instead of HTTP fetch
    // 2. Enriches the message with transformed data
    // 3. Validates the enriched data
    let workflow_json = r#"
    {
        "id": "benchmark_workflow",
        "name": "Benchmark Workflow Example",
        "description": "Demonstrates enrich -> validate flow without HTTP call",
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
                            }
                        ]
                    }
                }
            },
            {
                "id": "validate_user_data",
                "name": "Validate User Data",
                "description": "Ensure the user data meets our requirements",
                "function": {
                    "name": "validate",
                    "input": {
                        "rule": {
                            "and": [
                                { "!": { "var": "data.user.id" } },
                                { "!": { "var": "data.user.name" } },
                                { "!": { "var": "data.user.email" } },
                                {
                                    "in": [
                                        "@",
                                        { "var": "data.user.email" }
                                    ]
                                }
                            ]
                        }
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

    // Benchmark parameters
    let num_iterations = 10000;
    let mut total_duration = Duration::new(0, 0);
    let mut min_duration = Duration::new(u64::MAX, 0);
    let mut max_duration = Duration::new(0, 0);
    // Store all durations for percentile calculations
    let mut all_durations = Vec::with_capacity(num_iterations);

    println!("Starting benchmark with {} iterations...", num_iterations);

    // Run the benchmark
    for i in 0..num_iterations {
        // Create a new message with pre-loaded temp_data
        let mut message = Message::new(&json!({}));
        message.temp_data = sample_user_data.clone();

        // Time the processing
        let start = Instant::now();
        let _ = engine.process_message(&mut message);
        let duration = start.elapsed();

        // Store duration for percentile calculations
        all_durations.push(duration);

        // Update statistics
        total_duration += duration;
        min_duration = min_duration.min(duration);
        max_duration = max_duration.max(duration);

        // Print progress every 1000 iterations
        if (i + 1) % 1000 == 0 {
            println!("Completed {} iterations", i + 1);
        }
    }

    // Sort durations for percentile calculations
    all_durations.sort();

    // Calculate percentiles
    let p95_idx = (num_iterations as f64 * 0.95) as usize;
    let p99_idx = (num_iterations as f64 * 0.99) as usize;
    let p95 = all_durations.get(p95_idx).unwrap_or(&Duration::ZERO);
    let p99 = all_durations.get(p99_idx).unwrap_or(&Duration::ZERO);

    // Calculate and print benchmark results
    let avg_duration = total_duration / num_iterations as u32;
    
    println!("\nBenchmark Results (v{}):", VERSION);
    println!("  Iterations: {}", num_iterations);
    println!("  Min time: {:?}", min_duration);
    println!("  Max time: {:?}", max_duration);
    println!("  Avg time: {:?}", avg_duration);
    println!("  95th percentile: {:?}", p95);
    println!("  99th percentile: {:?}", p99);
    println!("  Total time: {:?}", total_duration);

    // Log results to file
    log_benchmark_results(
        num_iterations,
        min_duration,
        max_duration,
        avg_duration,
        *p95,
        *p99,
        total_duration,
    )?;

    println!("\nBenchmark results saved to '{}'", BENCHMARK_LOG_FILE);

    Ok(())
}

fn log_benchmark_results(
    iterations: usize,
    min_time: Duration,
    max_time: Duration,
    avg_time: Duration,
    p95: Duration,
    p99: Duration,
    total_time: Duration,
) -> io::Result<()> {
    // Read existing benchmark data or create new if file doesn't exist
    let mut benchmark_data = read_benchmark_file().unwrap_or_else(|_| json!({}));
    
    // Create a new benchmark entry
    let benchmark_entry = json!({
        "iterations": iterations,
        "min_time_ns": min_time.as_nanos(),
        "max_time_ns": max_time.as_nanos(),
        "avg_time_ns": avg_time.as_nanos(),
        "p95_ns": p95.as_nanos(),
        "p99_ns": p99.as_nanos(),
        "total_time_ns": total_time.as_nanos(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    
    // Add the new entry under the current version
    let version_key = VERSION.to_string();
    
    if let Some(obj) = benchmark_data.as_object_mut() {
        // If this version already exists, treat it as an array of benchmark runs
        match obj.get_mut(&version_key) {
            Some(Value::Array(arr)) => {
                // Version exists and is an array, add new entry
                arr.push(benchmark_entry);
            }
            Some(existing) => {
                // Version exists but is not an array, convert to array with old and new value
                let old_value = existing.clone();
                let mut new_arr = Vec::new();
                new_arr.push(old_value);
                new_arr.push(benchmark_entry);
                obj.insert(version_key, Value::Array(new_arr));
            }
            None => {
                // First entry for this version, start with a single entry (not array)
                obj.insert(version_key, benchmark_entry);
            }
        }
    }
    
    // Write the updated data back to the file
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
            eprintln!("Warning: Could not parse benchmark file: {}", e);
            Ok(json!({}))
        }
    }
} 