use dataflow_rs::{Engine, Message, ThreadedEngine, Workflow};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

const ITERATIONS: usize = 100_000;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════╗");
    println!("║      DATAFLOW ENGINE BENCHMARK          ║");
    println!("╚══════════════════════════════════════════╝");
    println!();
    println!("Configuration:");
    println!("  • Total iterations: {}", ITERATIONS);
    println!("  • CPU cores available: {}", num_cpus::get());
    println!();

    // Define a CPU-intensive workflow with multiple transformations
    let workflow_json = r#"
    {
        "id": "benchmark_workflow",
        "name": "Benchmark Workflow",
        "description": "CPU-intensive workflow for performance testing",
        "priority": 1,
        "tasks": [
            {
                "id": "transform_data",
                "name": "Transform Data",
                "description": "Complex data transformations",
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

    // Parse the workflow
    let workflow = Workflow::from_json(workflow_json)?;

    // Sample data for benchmarking
    let sample_data = json!({
        "id": 12345,
        "name": "John Doe",
        "email": "john.doe@example.com",
        "age": 25,
        "department": "Engineering"
    });

    // Run single-threaded benchmark
    println!("═══════════════════════════════════════════");
    println!("📊 SINGLE-THREADED ENGINE");
    println!("═══════════════════════════════════════════");
    let single_throughput = run_single_threaded_benchmark(workflow.clone(), &sample_data)?;

    // Collect all results for comparison
    let mut all_results = Vec::new();
    let mut async_results = Vec::new();

    // Run multi-threaded benchmarks with SYNC interface
    {
        println!();
        println!("═══════════════════════════════════════════");
        println!("📊 MULTI-THREADED ENGINE - SYNC INTERFACE");
        println!("═══════════════════════════════════════════");

        // Test with different worker and client configurations
        let worker_counts = vec![1, 2, 4, 8];
        let client_counts = vec![1, 2, 4, 8, 16];

        for &workers in &worker_counts {
            println!("\n🔧 Testing {} worker thread(s):", workers);
            println!("─────────────────────────────────────");

            for &clients in &client_counts {
                let throughput =
                    run_parallel_benchmark(workflow.clone(), &sample_data, workers, clients)?;

                all_results.push((workers, clients, throughput));

                // Show progress
                let speedup = throughput / single_throughput;
                let efficiency = (speedup / workers as f64) * 100.0;

                println!(
                    "  {:2} client(s): {:>8.0} msg/s (speedup: {:.2}x, efficiency: {:.1}%)",
                    clients, throughput, speedup, efficiency
                );
            }
        }

        // Display comprehensive results table
        println!();
        println!("═══════════════════════════════════════════════════════════════════════");
        println!("📊 PERFORMANCE MATRIX (throughput in msg/s)");
        println!("═══════════════════════════════════════════════════════════════════════");
        println!("        │ Concurrent Clients                                           ");
        println!("Workers │    1    │    2    │    4    │    8    │   16    │ Best");
        println!("────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────");

        for &workers in &worker_counts {
            print!("{:^7} │", workers);
            let mut row_best = 0.0;

            for &clients in &client_counts {
                let throughput = all_results
                    .iter()
                    .find(|(w, c, _)| *w == workers && *c == clients)
                    .map(|(_, _, t)| *t)
                    .unwrap_or(0.0);

                print!(" {:>7.0} │", throughput);
                if throughput > row_best {
                    row_best = throughput;
                }
            }

            let speedup = row_best / single_throughput;
            print!(" {:>7.0}", row_best);
            println!(" ({:.2}x)", speedup);
        }

        println!("═══════════════════════════════════════════════════════════════════════");

        // Key insights and analysis
        println!();
        println!("📈 KEY INSIGHTS:");
        println!("────────────────");
        println!(
            "  • Single-threaded baseline: {:.0} msg/s",
            single_throughput
        );

        // Find best configuration
        let best = all_results
            .iter()
            .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
            .unwrap();
        println!(
            "  • Best configuration: {} workers, {} clients = {:.0} msg/s",
            best.0, best.1, best.2
        );
        println!(
            "  • Maximum speedup achieved: {:.2}x",
            best.2 / single_throughput
        );

        // Analyze scaling patterns
        let single_client_1w = all_results
            .iter()
            .find(|(w, c, _)| *w == 1 && *c == 1)
            .map(|(_, _, t)| *t)
            .unwrap_or(0.0);

        let single_client_4w = all_results
            .iter()
            .find(|(w, c, _)| *w == 4 && *c == 1)
            .map(|(_, _, t)| *t)
            .unwrap_or(0.0);

        let multi_client_4w = all_results
            .iter()
            .find(|(w, c, _)| *w == 4 && *c == 16)
            .map(|(_, _, t)| *t)
            .unwrap_or(0.0);

        println!();
        println!("💡 SCALING ANALYSIS:");
        println!("────────────────────");

        if single_client_4w <= single_client_1w * 1.1 {
            println!("  ⚠️  Single client cannot saturate multiple workers");
            println!("      1 worker, 1 client:  {:.0} msg/s", single_client_1w);
            println!(
                "      4 workers, 1 client: {:.0} msg/s (no improvement)",
                single_client_4w
            );
        }

        if multi_client_4w > single_client_4w * 1.5 {
            println!("  ✅ Multiple clients enable better parallelism");
            println!("      4 workers, 1 client:   {:.0} msg/s", single_client_4w);
            println!(
                "      4 workers, 16 clients: {:.0} msg/s ({:.1}x better)",
                multi_client_4w,
                multi_client_4w / single_client_4w
            );
        }

        // Explain why threaded performance is lower
        if best.2 < single_throughput {
            println!();
            println!("⚠️  WHY IS SYNC THREADING SLOWER?");
            println!("──────────────────────────────");
            println!("  For CPU-bound tasks with ~5-10μs execution time:");
            println!("  • Synchronization overhead (~2-3μs per message)");
            println!("  • Context switching cost (~1-10μs)");
            println!("  • Channel bridging overhead in sync interface");
            println!("  • Cache misses from thread switching");
        }
    }

    // Run multi-threaded benchmarks with ASYNC interface
    {
        println!();
        println!("═══════════════════════════════════════════");
        println!("📊 MULTI-THREADED ENGINE - ASYNC INTERFACE");
        println!("═══════════════════════════════════════════");

        let worker_counts = vec![1, 2, 4, 8];
        let client_counts = vec![1, 2, 4, 8, 16, 32];

        for &workers in &worker_counts {
            println!("\n🔧 Testing {} worker thread(s):", workers);
            println!("─────────────────────────────────────");

            for &clients in &client_counts {
                let throughput =
                    run_async_benchmark(workflow.clone(), &sample_data, workers, clients).await?;

                async_results.push((workers, clients, throughput));

                let speedup = throughput / single_throughput;
                let efficiency = (speedup / workers as f64) * 100.0;

                println!(
                    "  {:2} client(s): {:>8.0} msg/s (speedup: {:.2}x, efficiency: {:.1}%)",
                    clients, throughput, speedup, efficiency
                );
            }
        }

        // Display async results table
        println!();
        println!("═══════════════════════════════════════════════════════════════════════════════");
        println!("📊 ASYNC PERFORMANCE MATRIX (throughput in msg/s)");
        println!("═══════════════════════════════════════════════════════════════════════════════");
        println!(
            "        │ Concurrent Tasks                                                      "
        );
        println!("Workers │    1    │    2    │    4    │    8    │   16    │   32    │ Best");
        println!("────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────┼─────────");

        for &workers in &worker_counts {
            print!("{:^7} │", workers);
            let mut row_best = 0.0;

            for &clients in &client_counts {
                let throughput = async_results
                    .iter()
                    .find(|(w, c, _)| *w == workers && *c == clients)
                    .map(|(_, _, t)| *t)
                    .unwrap_or(0.0);

                print!(" {:>7.0} │", throughput);
                if throughput > row_best {
                    row_best = throughput;
                }
            }

            let speedup = row_best / single_throughput;
            print!(" {:>7.0}", row_best);
            println!(" ({:.2}x)", speedup);
        }

        println!("═══════════════════════════════════════════════════════════════════════════════");

        // Compare best results from both interfaces
        let best_sync = all_results
            .iter()
            .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
            .unwrap();

        let best_async = async_results
            .iter()
            .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
            .unwrap();

        println!();
        println!("═══════════════════════════════════════════");
        println!("📊 PERFORMANCE COMPARISON SUMMARY");
        println!("═══════════════════════════════════════════");
        println!(
            "  • Single-threaded baseline: {:.0} msg/s",
            single_throughput
        );
        println!(
            "  • Best SYNC ({} workers, {} clients): {:.0} msg/s ({:.2}x)",
            best_sync.0,
            best_sync.1,
            best_sync.2,
            best_sync.2 / single_throughput
        );
        println!(
            "  • Best ASYNC ({} workers, {} clients): {:.0} msg/s ({:.2}x)",
            best_async.0,
            best_async.1,
            best_async.2,
            best_async.2 / single_throughput
        );
        println!();

        if best_async.2 > best_sync.2 {
            let improvement = (best_async.2 / best_sync.2 - 1.0) * 100.0;
            println!(
                "  🎯 Async interface is {:.1}% faster than sync",
                improvement
            );
            println!();
            println!("💡 WHY ASYNC IS FASTER:");
            println!("───────────────────────");
            println!("  • No channel bridging overhead");
            println!("  • Direct oneshot channel communication");
            println!("  • Better integration with Tokio runtime");
            println!("  • Natural async/await flow without blocking");
        } else {
            println!("  ⚠️  Sync interface performed similarly or better");
            println!("      CPU-bound workload dominates any overhead differences");
        }
    }

    Ok(())
}

fn run_single_threaded_benchmark(
    workflow: Workflow,
    sample_data: &serde_json::Value,
) -> Result<f64, Box<dyn std::error::Error>> {
    let mut engine = Engine::new(vec![workflow], None, None);

    // Warm-up
    print!("Warming up...");
    use std::io::Write;
    std::io::stdout().flush()?;

    for _ in 0..1000 {
        let mut message = Message::new(&json!({}));
        message.temp_data = sample_data.clone();
        let _ = engine.process_message(&mut message);
    }
    println!(" done");

    // Benchmark run
    print!("Running benchmark...");
    std::io::stdout().flush()?;

    let benchmark_start = Instant::now();
    let mut success_count = 0;

    for i in 0..ITERATIONS {
        let mut message = Message::new(&json!({}));
        message.temp_data = sample_data.clone();
        message.metadata = json!({ "iteration": i });

        if engine.process_message(&mut message).is_ok() {
            success_count += 1;
        }

        // Progress indicator
        if (i + 1) % 10000 == 0 {
            print!(".");
            std::io::stdout().flush()?;
        }
    }

    let total_time = benchmark_start.elapsed();
    println!(" done");

    let throughput = ITERATIONS as f64 / total_time.as_secs_f64();

    println!();
    println!("Results:");
    println!("  • Total time:    {:.3} seconds", total_time.as_secs_f64());
    println!("  • Successful:    {} / {}", success_count, ITERATIONS);
    println!("  • Throughput:    {:.0} messages/second", throughput);
    println!(
        "  • Avg latency:   {:.1} μs/message",
        total_time.as_micros() as f64 / ITERATIONS as f64
    );

    Ok(throughput)
}

fn run_parallel_benchmark(
    workflow: Workflow,
    sample_data: &serde_json::Value,
    worker_threads: usize,
    client_threads: usize,
) -> Result<f64, Box<dyn std::error::Error>> {
    let engine = Arc::new(ThreadedEngine::new(
        vec![workflow],
        None,
        None,
        worker_threads,
    ));

    // Warm-up
    for _ in 0..100 {
        let message = Message::new(&json!({}));
        let _ = engine.process_message_sync(message);
    }

    let messages_per_client = ITERATIONS / client_threads;
    let success_count = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(client_threads + 1));

    // Spawn client threads to submit work concurrently
    let mut handles = Vec::new();

    for client_id in 0..client_threads {
        let engine = Arc::clone(&engine);
        let sample_data = sample_data.clone();
        let success = Arc::clone(&success_count);
        let barrier = Arc::clone(&barrier);

        let handle = thread::spawn(move || {
            // Wait for all clients to be ready
            barrier.wait();

            // Submit messages
            for i in 0..messages_per_client {
                let mut message = Message::new(&json!({}));
                message.temp_data = sample_data.clone();
                message.metadata = json!({
                    "iteration": client_id * messages_per_client + i
                });

                if engine.process_message_sync(message).is_ok() {
                    success.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        handles.push(handle);
    }

    // Start timing when all clients are ready
    barrier.wait();
    let start = Instant::now();

    // Wait for all clients to complete
    for handle in handles {
        handle.join().unwrap();
    }

    let elapsed = start.elapsed();
    let total_processed = success_count.load(Ordering::Relaxed);
    let throughput = total_processed as f64 / elapsed.as_secs_f64();

    Ok(throughput)
}

async fn run_async_benchmark(
    workflow: Workflow,
    sample_data: &serde_json::Value,
    worker_threads: usize,
    client_tasks: usize,
) -> Result<f64, Box<dyn std::error::Error>> {
    let engine = Arc::new(ThreadedEngine::new(
        vec![workflow],
        None,
        None,
        worker_threads,
    ));

    // Warm-up
    for _ in 0..100 {
        let message = Message::new(&json!({}));
        let _ = engine.process_message(message).await;
    }

    let messages_per_client = ITERATIONS / client_tasks;
    let success_count = Arc::new(AtomicUsize::new(0));

    let start = Instant::now();
    let mut handles = Vec::new();

    for client_id in 0..client_tasks {
        let engine = Arc::clone(&engine);
        let sample_data = sample_data.clone();
        let success = Arc::clone(&success_count);

        let handle = tokio::spawn(async move {
            for i in 0..messages_per_client {
                let mut message = Message::new(&json!({}));
                message.temp_data = sample_data.clone();
                message.metadata = json!({
                    "iteration": client_id * messages_per_client + i
                });

                if engine.process_message(message).await.is_ok() {
                    success.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap();
    }

    let elapsed = start.elapsed();
    let actual_messages = success_count.load(Ordering::Relaxed);
    let throughput = actual_messages as f64 / elapsed.as_secs_f64();

    Ok(throughput)
}
