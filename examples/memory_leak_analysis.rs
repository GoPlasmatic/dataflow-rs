use dataflow_rs::{Engine, Workflow, engine::message::Message};
use serde_json::json;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

// Custom allocator to track memory usage
struct TrackingAllocator;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static DEALLOCATED: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe {
            let ret = System.alloc(layout);
            if !ret.is_null() {
                ALLOCATED.fetch_add(layout.size(), Ordering::SeqCst);
            }
            ret
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe {
            System.dealloc(ptr, layout);
            DEALLOCATED.fetch_add(layout.size(), Ordering::SeqCst);
        }
    }
}

#[global_allocator]
static GLOBAL: TrackingAllocator = TrackingAllocator;

fn get_current_memory() -> isize {
    let allocated = ALLOCATED.load(Ordering::SeqCst);
    let deallocated = DEALLOCATED.load(Ordering::SeqCst);
    (allocated as isize) - (deallocated as isize)
}

fn format_bytes(bytes: isize) -> String {
    let abs_bytes = bytes.abs() as f64;
    if abs_bytes < 1024.0 {
        format!("{} B", bytes)
    } else if abs_bytes < 1024.0 * 1024.0 {
        format!("{:.2} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

const WARMUP_ITERATIONS: usize = 10_000;
const TEST_ITERATIONS: usize = 100_000;
const SAMPLE_INTERVAL: usize = 10_000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("========================================");
    println!("MEMORY LEAK ANALYSIS");
    println!("========================================\n");
    println!("Warmup iterations: {}", WARMUP_ITERATIONS);
    println!("Test iterations: {}", TEST_ITERATIONS);
    println!("Sample interval: every {} iterations\n", SAMPLE_INTERVAL);

    // Define a workflow with memory-intensive operations
    let workflow_json = r#"
    {
        "id": "memory_test_workflow",
        "name": "Memory Test Workflow",
        "description": "Workflow for memory leak detection",
        "priority": 1,
        "tasks": [
            {
                "id": "transform_data",
                "name": "Transform Data",
                "description": "Map and transform data fields",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "data.processed.id", 
                                "logic": { "var": "temp_data.id" }
                            },
                            {
                                "path": "data.processed.name", 
                                "logic": { "var": "temp_data.name" }
                            },
                            {
                                "path": "data.processed.email", 
                                "logic": { "var": "temp_data.email" }
                            },
                            {
                                "path": "data.processed.large_text",
                                "logic": { "var": "temp_data.large_text_field" }
                            },
                            {
                                "path": "data.processed.description",
                                "logic": { "var": "temp_data.description" }
                            },
                            {
                                "path": "data.processed.computed",
                                "logic": { 
                                    "cat": [
                                        { "var": "temp_data.name" },
                                        " - ",
                                        { "var": "temp_data.department" },
                                        " - ",
                                        { "var": "temp_data.id" },
                                        " - ",
                                        { "var": "temp_data.description" }
                                    ]
                                }
                            },
                            {
                                "path": "data.processed.tags",
                                "logic": {
                                    "map": [
                                        { "var": "temp_data.tags" },
                                        {
                                            "cat": [
                                                "processed_tag_",
                                                { "var": "" },
                                                "_with_suffix"
                                            ]
                                        }
                                    ]
                                }
                            },
                            {
                                "path": "data.processed.nested_objects",
                                "logic": { "var": "temp_data.nested_objects" }
                            },
                            {
                                "path": "data.processed.binary_data",
                                "logic": { "var": "temp_data.binary_data" }
                            },
                            {
                                "path": "data.processed.floats",
                                "logic": { "var": "temp_data.floats" }
                            },
                            {
                                "path": "data.processed.additional_fields",
                                "logic": { "var": "temp_data.additional_fields" }
                            },
                            {
                                "path": "data.processed.metadata",
                                "logic": { "var": "temp_data.metadata" }
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
                                "logic": { "!!" : { "var": "data.processed.id" } },
                                "message": "ID is required"
                            },
                            {
                                "path": "data",
                                "logic": { "!!" : { "var": "data.processed.email" } },
                                "message": "Email is required"
                            },
                            {
                                "path": "data",
                                "logic": { "!!" : { "var": "data.processed.tags" } },
                                "message": "Tags are required"
                            },
                            {
                                "path": "data",
                                "logic": { "!!" : { "var": "data.processed.nested_objects" } },
                                "message": "Nested objects are required"
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

    // Create the engine
    let mut engine = Engine::new(vec![workflow], None, None);

    // Create large sample data to make memory leaks more visible
    let large_text = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(100);
    let large_array: Vec<String> = (0..1000)
        .map(|i| format!("item_{}_with_some_longer_text_content", i))
        .collect();
    let nested_objects: Vec<serde_json::Value> = (0..100).map(|i| {
        json!({
            "id": i,
            "name": format!("Object {}", i),
            "description": format!("This is a description for object {} with some additional text", i),
            "attributes": {
                "color": format!("color_{}", i),
                "size": format!("size_{}", i),
                "weight": i * 100,
                "metadata": {
                    "created": "2024-01-01T00:00:00Z",
                    "modified": "2024-01-01T00:00:00Z",
                    "version": i
                }
            },
            "tags": vec![format!("tag_{}", i), format!("category_{}", i % 10), format!("type_{}", i % 5)]
        })
    }).collect();

    let sample_data = json!({
        "id": 12345,
        "name": "John Doe",
        "email": "john.doe@example.com",
        "age": 25,
        "department": "Engineering",
        "description": large_text.clone(),
        "large_text_field": large_text,
        "tags": large_array,
        "nested_objects": nested_objects,
        "metadata": {
            "created": "2024-01-01T00:00:00Z",
            "updated": "2024-01-01T00:00:00Z",
            "complex_data": {
                "level1": {
                    "level2": {
                        "level3": {
                            "data": (0..100).map(|i| format!("nested_value_{}", i)).collect::<Vec<_>>()
                        }
                    }
                }
            }
        },
        "binary_data": (0..10000).map(|i| i % 256).collect::<Vec<u32>>(),
        "floats": (0..1000).map(|i| i as f64 * 3.14159).collect::<Vec<f64>>(),
        "additional_fields": (0..50).map(|i| {
            (format!("field_{}", i), format!("value_{}_with_additional_content", i))
        }).collect::<std::collections::HashMap<_, _>>()
    });

    // Memory tracking
    let initial_memory = get_current_memory();
    println!("Initial memory usage: {}", format_bytes(initial_memory));

    // Warmup phase
    println!("\n📊 WARMUP PHASE");
    println!("──────────────────────────────────────────");
    let warmup_start = Instant::now();
    let warmup_initial_memory = get_current_memory();

    for i in 0..WARMUP_ITERATIONS {
        let mut message = Message::new(&json!({}));
        message.temp_data = sample_data.clone();
        message.metadata = json!({
            "iteration": i,
            "phase": "warmup"
        });

        let _ = engine.process_message(&mut message);

        if (i + 1) % 1000 == 0 {
            print!(".");
            use std::io::Write;
            std::io::stdout().flush()?;
        }
    }

    let warmup_duration = warmup_start.elapsed();
    let warmup_final_memory = get_current_memory();
    let warmup_memory_growth = warmup_final_memory - warmup_initial_memory;

    println!("\n");
    println!(
        "Warmup complete in: {:.2} seconds",
        warmup_duration.as_secs_f64()
    );
    println!("Memory after warmup: {}", format_bytes(warmup_final_memory));
    println!(
        "Memory growth during warmup: {}",
        format_bytes(warmup_memory_growth)
    );

    // Give time for any deferred deallocations
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Test phase - looking for memory leaks
    println!("\n📊 MEMORY LEAK TEST PHASE");
    println!("──────────────────────────────────────────");
    let test_start = Instant::now();
    let test_initial_memory = get_current_memory();
    let mut memory_samples: Vec<(usize, isize)> = Vec::new();
    let mut max_memory = test_initial_memory;
    let mut min_memory = test_initial_memory;

    println!("Starting memory: {}", format_bytes(test_initial_memory));
    println!("\nRunning iterations...");

    for i in 0..TEST_ITERATIONS {
        let mut message = Message::new(&json!({}));
        message.temp_data = sample_data.clone();
        message.metadata = json!({
            "iteration": i,
            "phase": "test",
            "timestamp": chrono::Utc::now().to_rfc3339()
        });

        match engine.process_message(&mut message) {
            Ok(_) => {}
            Err(e) => {
                eprintln!("Error at iteration {}: {:?}", i, e);
            }
        }

        // Sample memory usage periodically
        if (i + 1) % SAMPLE_INTERVAL == 0 {
            let current_memory = get_current_memory();
            memory_samples.push((i + 1, current_memory));
            max_memory = max_memory.max(current_memory);
            min_memory = min_memory.min(current_memory);

            let memory_diff = current_memory - test_initial_memory;
            println!(
                "Iteration {:6}: Memory = {} (Δ = {})",
                i + 1,
                format_bytes(current_memory),
                format_bytes(memory_diff)
            );
        }
    }

    let test_duration = test_start.elapsed();
    let test_final_memory = get_current_memory();
    let test_memory_growth = test_final_memory - test_initial_memory;

    println!("\n========================================");
    println!("📈 ANALYSIS RESULTS");
    println!("========================================\n");

    println!("Test Duration: {:.2} seconds", test_duration.as_secs_f64());
    println!(
        "Iterations/second: {:.0}",
        TEST_ITERATIONS as f64 / test_duration.as_secs_f64()
    );
    println!();

    println!("MEMORY STATISTICS:");
    println!("──────────────────────────────────────────");
    println!(
        "Initial memory (after warmup): {}",
        format_bytes(test_initial_memory)
    );
    println!("Final memory: {}", format_bytes(test_final_memory));
    println!("Memory growth: {}", format_bytes(test_memory_growth));
    println!("Peak memory: {}", format_bytes(max_memory));
    println!("Minimum memory: {}", format_bytes(min_memory));
    println!("Memory range: {}", format_bytes(max_memory - min_memory));
    println!();

    // Calculate memory growth rate
    let growth_per_iteration = test_memory_growth as f64 / TEST_ITERATIONS as f64;
    println!(
        "Average growth per iteration: {:.2} bytes",
        growth_per_iteration
    );

    // Analyze trend
    if memory_samples.len() >= 2 {
        let first_half_avg = memory_samples[..memory_samples.len() / 2]
            .iter()
            .map(|(_, m)| *m)
            .sum::<isize>() as f64
            / (memory_samples.len() / 2) as f64;

        let second_half_avg = memory_samples[memory_samples.len() / 2..]
            .iter()
            .map(|(_, m)| *m)
            .sum::<isize>() as f64
            / (memory_samples.len() - memory_samples.len() / 2) as f64;

        let trend = second_half_avg - first_half_avg;

        println!("\nMEMORY TREND ANALYSIS:");
        println!("──────────────────────────────────────────");
        println!(
            "First half average: {}",
            format_bytes(first_half_avg as isize)
        );
        println!(
            "Second half average: {}",
            format_bytes(second_half_avg as isize)
        );
        println!("Trend: {}", format_bytes(trend as isize));

        // Verdict
        println!("\n🔍 VERDICT:");
        println!("──────────────────────────────────────────");

        let threshold_bytes = 1024 * 100; // 100 KB threshold
        if test_memory_growth.abs() < threshold_bytes {
            println!("✅ PASS: Memory usage is stable (growth < 100 KB)");
            println!("   No significant memory leak detected.");
        } else if growth_per_iteration < 10.0 {
            println!("⚠️  WARNING: Small memory growth detected");
            println!(
                "   Growth rate: {:.2} bytes/iteration",
                growth_per_iteration
            );
            println!("   This may be acceptable for your use case.");
        } else {
            println!("❌ FAIL: Potential memory leak detected!");
            println!(
                "   Growth rate: {:.2} bytes/iteration",
                growth_per_iteration
            );
            println!("   Total growth: {}", format_bytes(test_memory_growth));
            println!("   This indicates a possible memory leak that should be investigated.");
        }

        if trend.abs() as isize > threshold_bytes {
            println!("\n⚠️  Increasing memory trend detected between first and second half!");
        }
    }

    println!("\n========================================");
    println!("Analysis complete!");
    println!("========================================");

    Ok(())
}
