use dataflow_rs::{engine::message::Message, Engine, Workflow};
use serde_json::json;
use std::time::{Duration, Instant};

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

        // Update statistics
        total_duration += duration;
        min_duration = min_duration.min(duration);
        max_duration = max_duration.max(duration);

        // Print progress every 100 iterations
        if (i + 1) % 1000 == 0 {
            println!("Completed {} iterations", i + 1);
        }
    }

    // Calculate and print benchmark results
    let avg_duration = total_duration / num_iterations as u32;
    println!("\nBenchmark Results:");
    println!("  Iterations: {}", num_iterations);
    println!("  Min time: {:?}", min_duration);
    println!("  Max time: {:?}", max_duration);
    println!("  Avg time: {:?}", avg_duration);
    println!("  Total time: {:?}", total_duration);

    Ok(())
} 