<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  # Dataflow-rs

  **A high-performance workflow engine for building data processing pipelines in Rust with zero-overhead JSONLogic evaluation.**

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)](https://www.rust-lang.org)
  [![Crates.io](https://img.shields.io/crates/v/dataflow-rs.svg)](https://crates.io/crates/dataflow-rs)
</div>

---

Dataflow-rs is a Rust library for creating high-performance data processing pipelines with pre-compiled JSONLogic and zero runtime overhead. It features a modular architecture that separates compilation from execution, ensuring predictable low-latency performance. With built-in multi-threading support through ThreadedEngine and high-performance parallel processing via RayonEngine, it provides excellent vertical scaling capabilities. Whether you're building REST APIs, processing Kafka streams, or creating sophisticated data transformation pipelines, Dataflow-rs provides enterprise-grade performance with minimal complexity.

## 🚀 Key Features

- **Zero Runtime Compilation:** All JSONLogic expressions pre-compiled at startup for optimal performance.
- **Multi-Threading Support:** Built-in ThreadedEngine with configurable thread pools for vertical scaling.
- **Parallel Processing:** RayonEngine leverages work-stealing for CPU-intensive workloads.
- **Modular Architecture:** Clear separation between compilation (LogicCompiler) and execution (InternalExecutor).
- **Direct DataLogic Instantiation:** Each engine has its own DataLogic instance for zero contention.
- **Immutable Workflows:** Workflows compiled once at initialization for predictable performance.
- **Dynamic Workflows:** Use JSONLogic to control workflow execution based on your data.
- **Extensible:** Easily add your own custom processing steps (tasks) to the engine.
- **Built-in Functions:** Comes with thread-safe implementations of data mapping and validation.
- **Resilient:** Built-in error handling and retry mechanisms to handle transient failures.
- **Auditing:** Keep track of all the changes that happen to your data as it moves through the pipeline.

## 🏁 Getting Started

Here's a quick example to get you up and running.

### 1. Add to `Cargo.toml`

```toml
[dependencies]
dataflow-rs = "1.0.8"
serde_json = "1.0"
```

### 2. Create a Workflow

Workflows are defined in JSON and consist of a series of tasks.

```json
{
    "id": "data_processor",
    "name": "Data Processor",
    "tasks": [
        {
            "id": "transform_data",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {
                            "path": "data.user_name",
                            "logic": { "var": "temp_data.name" }
                        },
                        {
                            "path": "data.user_email",
                            "logic": { "var": "temp_data.email" }
                        }
                    ]
                }
            }
        }
    ]
}
```

### 3. Run the Engine

```rust
use dataflow_rs::{Engine, Workflow};
use dataflow_rs::engine::message::Message;
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define workflows
    let workflow_json = r#"{ ... }"#; // Your workflow JSON from above
    let workflow = Workflow::from_json(workflow_json)?;
    
    // Create engine with workflows (immutable after creation)
    let mut engine = Engine::new(
        vec![workflow],  // Workflows to compile and cache
        None,           // Custom functions (optional)
        None,           // Retry config (optional)
    );

    // Process a single message
    let mut message = Message::new(&json!({}));
    engine.process_message(&mut message)?;

    println!("✅ Processed result: {}", serde_json::to_string_pretty(&message.data)?);

    Ok(())
}
```

### 4. Multi-Threading with ThreadedEngine

For high-performance multi-threaded processing, use the built-in ThreadedEngine:

```rust
use dataflow_rs::{ThreadedEngine, Workflow};
use dataflow_rs::engine::message::Message;
use serde_json::json;
use std::sync::Arc;
use std::thread;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define your workflows
    let workflow_json = r#"{ ... }"#; // Your workflow JSON
    let workflow = Workflow::from_json(workflow_json)?;
    
    // Create ThreadedEngine with 4 worker threads
    let engine = Arc::new(ThreadedEngine::new(
        vec![workflow],  // Workflows
        None,           // Custom functions (optional)
        None,           // Retry config (optional)
        4,              // Number of worker threads
    ));

    // Process messages concurrently from multiple client threads
    let mut handles = Vec::new();
    
    for i in 0..1000 {
        let engine = Arc::clone(&engine);
        let handle = thread::spawn(move || {
            let message = Message::new(&json!({"id": i}));
            engine.process_message_sync(message)
        });
        handles.push(handle);
    }

    // Wait for all messages to complete
    for handle in handles {
        handle.join().unwrap()?;
    }

    println!("✅ Processed 1000 messages with ThreadedEngine!");
    Ok(())
}
```

The ThreadedEngine provides:
- **Configurable thread pool** for vertical scaling
- **Work queue distribution** for efficient load balancing
- **Graceful shutdown** support
- **Both sync and async APIs** for flexible integration
- **Health monitoring** and worker restart capabilities

### 5. High-Performance Parallel Processing with RayonEngine

For CPU-intensive workloads requiring maximum parallelism, use RayonEngine:

```rust
use dataflow_rs::{RayonEngine, Workflow, Message};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define your workflows
    let workflow_json = r#"{ ... }"#; // Your workflow JSON
    let workflow = Workflow::from_json(workflow_json)?;
    
    // Create RayonEngine (uses all CPU cores by default)
    let engine = RayonEngine::new(
        vec![workflow],  // Workflows
        None,           // Custom functions (optional)
        None,           // Retry config (optional)
    );

    // Process batch of messages in parallel
    let messages: Vec<Message> = (0..10000)
        .map(|i| Message::new(&json!({"id": i})))
        .collect();
    
    let results = engine.process_batch(messages);
    
    // Handle results
    for result in results {
        match result {
            Ok(message) => println!("Processed: {:?}", message.data),
            Err(e) => eprintln!("Error: {:?}", e),
        }
    }

    println!("✅ Processed 10000 messages with RayonEngine!");
    Ok(())
}
```

The RayonEngine provides:
- **Work-stealing parallelism** for automatic load balancing
- **Thread-local engines** for zero contention
- **Batch processing** for maximum throughput
- **Stream processing** with parallel iterators
- **CPU-optimized** for compute-intensive workloads

## ✨ Core Concepts

- **Engine:** High-performance engine with pre-compiled logic and immutable workflows.
- **ThreadedEngine:** Multi-threaded variant with configurable thread pool for vertical scaling.
- **RayonEngine:** Parallel processing engine using Rayon's work-stealing for CPU-bound workloads.
- **LogicCompiler:** Compiles all JSONLogic expressions at initialization for zero runtime overhead.
- **InternalExecutor:** Executes built-in functions using pre-compiled logic from the cache.
- **Workflow:** A sequence of tasks executed in order, with conditions accessing only metadata.
- **Task:** A single processing step with optional JSONLogic conditions.
- **Message:** The data structure flowing through workflows with audit trail support.

## 🔧 Choosing Between Engine Types

**Use `Engine` when:**
- Processing messages sequentially in a single thread
- Embedding in async runtimes (tokio, async-std)
- Maximum performance for individual message processing
- Simple integration without thread management

**Use `ThreadedEngine` when:**
- Need to process multiple messages concurrently
- Want vertical scaling on multi-core systems
- Have mixed I/O and CPU-bound workloads
- Need a built-in thread pool with work distribution

**Use `RayonEngine` when:**
- Processing large batches of messages in parallel
- CPU-intensive workloads with minimal I/O
- Need automatic work-stealing for load balancing
- Want maximum CPU utilization across all cores

## 🏗️ Architecture

The v3.0 architecture focuses on simplicity and performance through clear separation of concerns:

### Compilation Phase (Startup)
1. **LogicCompiler** compiles all JSONLogic expressions from workflows and tasks
2. Creates an indexed cache of compiled logic for O(1) runtime access
3. Validates all logic expressions early, failing fast on errors
4. Stores compiled logic in contiguous memory for cache efficiency

### Execution Phase (Runtime)
1. **Engine** orchestrates message processing through immutable workflows
2. **InternalExecutor** evaluates conditions and executes built-in functions
3. Uses compiled logic from cache - zero compilation overhead at runtime
4. Direct DataLogic instantiation eliminates any locking or contention

### Key Design Decisions
- **Immutable Workflows:** All workflows defined at engine creation, cannot be modified
- **Pre-compilation:** All expensive parsing/compilation done once at startup
- **Direct Instantiation:** Each engine owns its DataLogic instance directly
- **Modular Design:** Clear boundaries between compilation, execution, and orchestration

## ⚡ Performance

Dataflow-rs achieves optimal performance through architectural improvements:

- **Pre-Compilation:** All JSONLogic compiled at startup, zero runtime overhead
- **Multi-Threading:** Built-in ThreadedEngine for vertical scaling with configurable worker threads
- **Parallel Processing:** RayonEngine with work-stealing for maximum CPU utilization
- **Cache-Friendly:** Compiled logic stored contiguously in memory
- **Direct Instantiation:** DataLogic instances created directly without locking
- **Predictable Latency:** No runtime allocations for logic evaluation
- **Modular Design:** Clear separation of compilation and execution phases

Run the included benchmarks to test performance on your hardware:
```bash
cargo run --example benchmark           # Comprehensive comparison
cargo run --example threaded_benchmark  # ThreadedEngine performance
cargo run --example rayon_benchmark     # RayonEngine performance
```

The benchmarks compare single-threaded Engine vs multi-threaded ThreadedEngine vs parallel RayonEngine with various configurations, providing detailed performance metrics and scaling analysis.

## 🛠️ Custom Functions

You can extend the engine with your own custom logic by implementing the `FunctionHandler` trait:

```rust
use dataflow_rs::engine::{FunctionHandler, FunctionConfig, error::Result, message::{Change, Message}};
use datalogic_rs::DataLogic;
use serde_json::{json, Value};
use std::collections::HashMap;

pub struct MyCustomFunction;

impl FunctionHandler for MyCustomFunction {
    fn execute(
        &self, 
        message: &mut Message, 
        config: &FunctionConfig,
        datalogic: &DataLogic,
    ) -> Result<(usize, Vec<Change>)> {
        // Your custom logic here
        println!("Hello from a custom function!");
        
        // Modify message data
        message.data["processed"] = json!(true);
        
        // Return status code and changes for audit trail
        Ok((200, vec![Change {
            path: "data.processed".to_string(),
            old_value: json!(null),
            new_value: json!(true),
            operation: "set".to_string(),
        }]))
    }
}

// Register when creating the engine:
let mut custom_functions = HashMap::new();
custom_functions.insert(
    "my_custom_function".to_string(),
    Box::new(MyCustomFunction) as Box<dyn FunctionHandler + Send + Sync>
);

let mut engine = Engine::new(
    workflows,
    Some(custom_functions),  // Custom functions
    None,  // Use default retry config
);
```

## 🤝 Contributing

We welcome contributions! Feel free to fork the repository, make your changes, and submit a pull request. Please make sure to add tests for any new features.

## 🏢 About Plasmatic

Dataflow-rs is developed by the team at [Plasmatic](https://github.com/GoPlasmatic). We're passionate about building open-source tools for data processing.

## 📄 License

This project is licensed under the Apache License, Version 2.0. See the [LICENSE](LICENSE) file for more details.