use dataflow_rs::engine::{Engine, Message, Workflow};
use serde_json::json;
use std::time::Instant;

#[tokio::main]
async fn main() {
    // Create a workflow with multiple sequential mappings
    let workflows_json = json!([
        {
            "id": "perf_test",
            "name": "Performance Test",
            "priority": 1,
            "condition": true,
            "tasks": [
                {
                    "id": "multi_map",
                    "name": "Multiple Sequential Mappings",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {"path": "data.field1", "logic": {"var": "data.source"}},
                                {"path": "data.field2", "logic": {"var": "data.field1"}},
                                {"path": "data.field3", "logic": {"var": "data.field2"}},
                                {"path": "data.field4", "logic": {"var": "data.field3"}},
                                {"path": "data.field5", "logic": {"var": "data.field4"}},
                                {"path": "temp_data.temp1", "logic": {"var": "data.field5"}},
                                {"path": "temp_data.temp2", "logic": {"var": "temp_data.temp1"}},
                                {"path": "data.result", "logic": {"var": "temp_data.temp2"}},
                                {"path": "metadata.meta1", "logic": {"var": "data.result"}},
                                {"path": "metadata.meta2", "logic": {"var": "metadata.meta1"}}
                            ]
                        }
                    }
                }
            ]
        }
    ]);

    let workflows: Vec<Workflow> = workflows_json
        .as_array()
        .unwrap()
        .iter()
        .map(|w| serde_json::from_value(w.clone()).unwrap())
        .collect();

    let engine = Engine::new(workflows, None);

    // Warmup
    println!("Warming up...");
    for _ in 0..1000 {
        let mut message = Message::from_value(&json!({
            "source": "test_value"
        }));
        engine.process_message(&mut message).await.unwrap();
    }

    // Benchmark
    let iterations = 100_000;
    println!(
        "Running {} iterations with 10 sequential mappings each...",
        iterations
    );

    let start = Instant::now();
    for _ in 0..iterations {
        let mut message = Message::from_value(&json!({
            "source": "test_value"
        }));
        engine.process_message(&mut message).await.unwrap();
    }
    let duration = start.elapsed();

    let throughput = iterations as f64 / duration.as_secs_f64();
    let avg_time = duration.as_micros() as f64 / iterations as f64;

    println!("\nPerformance Results:");
    println!("====================");
    println!("Total iterations: {}", iterations);
    println!("Total time: {:?}", duration);
    println!("Throughput: {:.0} messages/second", throughput);
    println!("Average time per message: {:.2} μs", avg_time);
    println!(
        "Total mappings processed: {} (10 per message)",
        iterations * 10
    );
    println!(
        "Mapping throughput: {:.0} mappings/second",
        throughput * 10.0
    );
}
