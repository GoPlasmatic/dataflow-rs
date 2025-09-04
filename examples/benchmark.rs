use dataflow_rs::{Engine, Workflow, engine::message::Message};
use serde_json::json;
use std::time::Instant;

const ITERATIONS: usize = 1_000_000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("========================================");
    println!("DATAFLOW ENGINE BENCHMARK");
    println!("========================================\n");
    println!(
        "Running {} iterations on single-threaded engine\n",
        ITERATIONS
    );

    // Define a simple workflow with data transformation
    let workflow_json = r#"
    {
        "id": "benchmark_workflow",
        "name": "Benchmark Workflow",
        "description": "Simple workflow for performance testing",
        "priority": 1,
        "tasks": [
            {
                "id": "transform_data",
                "name": "Transform Data",
                "description": "Map data fields",
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
                            }
                        ]
                    }
                }
            },
            {
                "id": "validate_data",
                "name": "Validate Data",
                "description": "Validate transformed data",
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

    // Create the engine with built-in functions
    let mut engine = Engine::new(vec![workflow], None, None);

    // Sample data for benchmarking
    let sample_data = json!({
        "id": 12345,
        "name": "John Doe",
        "email": "john.doe@example.com",
        "age": 25,
        "department": "Engineering"
    });

    // Warm-up run
    println!("Warming up...");
    for _ in 0..1000 {
        let mut message = Message::new(&json!({}));
        message.temp_data = sample_data.clone();
        let _ = engine.process_message(&mut message);
    }

    // Benchmark run
    println!("Starting benchmark...\n");

    let mut all_durations = Vec::with_capacity(ITERATIONS);
    let mut success_count = 0;
    let mut error_count = 0;

    let benchmark_start = Instant::now();

    for i in 0..ITERATIONS {
        let mut message = Message::new(&json!({}));
        message.temp_data = sample_data.clone();
        message.metadata = json!({
            "iteration": i,
            "timestamp": chrono::Utc::now().to_rfc3339()
        });

        let iteration_start = Instant::now();
        match engine.process_message(&mut message) {
            Ok(_) => {
                success_count += 1;
                if message.has_errors() {
                    error_count += 1;
                }
            }
            Err(_) => {
                error_count += 1;
            }
        }
        let iteration_duration = iteration_start.elapsed();
        all_durations.push(iteration_duration);

        // Progress indicator every 10k iterations
        if (i + 1) % 10000 == 0 {
            print!(".");
            use std::io::Write;
            std::io::stdout().flush()?;
        }
    }

    let total_time = benchmark_start.elapsed();
    println!("\n\nBenchmark Complete!");
    println!("==========================================\n");

    // Calculate statistics
    all_durations.sort_unstable();
    let p50 = all_durations[ITERATIONS * 50 / 100];
    let p90 = all_durations[ITERATIONS * 90 / 100];
    let p95 = all_durations[ITERATIONS * 95 / 100];
    let p99 = all_durations[ITERATIONS * 99 / 100];
    let throughput = ITERATIONS as f64 / total_time.as_secs_f64();

    // Display results
    println!("📊 PERFORMANCE METRICS");
    println!("──────────────────────────────────────────");
    println!("Total iterations:    {:>10}", ITERATIONS);
    println!("Successful:          {:>10}", success_count);
    println!("Errors:              {:>10}", error_count);
    println!(
        "Total time:          {:>10.3} seconds",
        total_time.as_secs_f64()
    );
    println!();

    println!("Messages/second:     {:>10.0}", throughput);
    println!();

    println!("📉 LATENCY PERCENTILES");
    println!("──────────────────────────────────────────");
    println!("P50:                 {:>10.3} μs", p50.as_micros() as f64);
    println!("P90:                 {:>10.3} μs", p90.as_micros() as f64);
    println!("P95:                 {:>10.3} μs", p95.as_micros() as f64);
    println!("P99:                 {:>10.3} μs", p99.as_micros() as f64);
    println!();

    Ok(())
}
