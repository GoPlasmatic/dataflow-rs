<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  # Dataflow-rs

  **A thread-safe, vertically-scalable async workflow engine for building high-performance data processing pipelines in Rust.**

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)](https://www.rust-lang.org)
  [![Crates.io](https://img.shields.io/crates/v/dataflow-rs.svg)](https://crates.io/crates/dataflow-rs)
</div>

---

Dataflow-rs is a Rust library for creating high-performance, asynchronous data processing pipelines with built-in thread-safety and vertical scalability. It's designed to maximize CPU utilization through intelligent concurrency management, allowing you to build complex workflows that automatically scale with your hardware. Whether you're building REST APIs, processing Kafka streams, or creating sophisticated data transformation pipelines, Dataflow-rs provides enterprise-grade performance out of the box.

## 🚀 Key Features

- **Thread-Safe & Scalable:** Built-in concurrency management with automatic vertical scaling to utilize all available CPU cores.
- **Zero-Cost Workflow Updates:** Arc-Swap architecture allows lock-free reads and atomic workflow updates.
- **Intelligent Resource Pooling:** DataLogic instance pooling eliminates contention and maximizes throughput.
- **Asynchronous by Design:** Built on Tokio for non-blocking, high-performance concurrent processing.
- **Dynamic Workflows:** Use JSONLogic to control workflow execution based on your data.
- **Extensible:** Easily add your own custom processing steps (tasks) to the engine.
- **Built-in Functions:** Comes with thread-safe implementations of HTTP requests, data mapping, and validation.
- **Resilient:** Built-in error handling and retry mechanisms to handle transient failures.
- **Auditing:** Keep track of all the changes that happen to your data as it moves through the pipeline.

## 🏁 Getting Started

Here's a quick example to get you up and running.

### 1. Add to `Cargo.toml`

```toml
[dependencies]
dataflow-rs = "1.0"
tokio = { version = "1.0", features = ["full"] }
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
            "id": "fetch_data",
            "function": {
                "name": "http",
                "input": { "url": "https://jsonplaceholder.typicode.com/users/1" }
            }
        },
        {
            "id": "transform_data",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {
                            "path": "data.user_name",
                            "logic": { "var": "temp_data.body.name" }
                        },
                        {
                            "path": "data.user_email",
                            "logic": { "var": "temp_data.body.email" }
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a new engine (defaults to CPU count for max concurrency)
    let engine = Engine::new();
    
    // Or specify custom concurrency level
    // let engine = Engine::with_concurrency(32);

    // Define and add a workflow
    let workflow_json = r#"{ ... }"#; // Your workflow JSON from above
    let workflow = Workflow::from_json(workflow_json)?;
    engine.add_workflow(&workflow);

    // Process a single message
    let mut message = Message::new(&json!({}));
    engine.process_message(&mut message).await?;

    println!("✅ Processed result: {}", serde_json::to_string_pretty(&message.data)?);

    Ok(())
}
```

### 4. Concurrent Processing (New in v1.0)

Process multiple messages concurrently with automatic resource management:

```rust
use dataflow_rs::{Engine, Workflow};
use dataflow_rs::engine::message::Message;
use serde_json::json;
use std::sync::Arc;
use tokio::task::JoinSet;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create engine with 16 concurrent workers
    let engine = Arc::new(Engine::with_concurrency(16));
    
    // Add your workflow
    let workflow_json = r#"{ ... }"#; // Your workflow JSON
    let workflow = Workflow::from_json(workflow_json)?;
    engine.add_workflow(&workflow);

    // Process messages concurrently
    let mut tasks = JoinSet::new();
    
    for i in 0..1000 {
        let engine_clone = engine.clone();
        tasks.spawn(async move {
            let mut message = Message::new(&json!({"id": i}));
            engine_clone.process_message_concurrent(&mut message).await
        });
    }

    // Wait for all messages to complete
    while let Some(result) = tasks.join_next().await {
        result??;
    }

    println!("✅ Processed 1000 messages concurrently!");
    Ok(())
}
```

## ✨ Core Concepts

- **Engine:** The heart of the library, now thread-safe with configurable concurrency levels.
- **Workflow:** A sequence of tasks that are executed in order, stored using Arc-Swap for lock-free reads.
- **Task:** A single step in a workflow, like making an HTTP request or transforming data.
- **Message:** The data that flows through the engine, with each message getting its own DataLogic instance.
- **Concurrency:** Unified concurrency model where pool size matches max concurrent messages to eliminate contention.

## ⚡ Performance

Dataflow-rs v1.0 introduces significant performance improvements through its unified concurrency model:

- **Improved Scalability:** Performance scales with available CPU cores
- **Zero Contention:** Pool size matches concurrent tasks to eliminate resource contention
- **Lock-Free Reads:** Arc-Swap architecture enables zero-cost workflow reads
- **High Throughput:** Achieve substantial throughput improvements with increased concurrency

Run the included benchmark to test performance on your hardware:
```bash
cargo run --example benchmark
```

## 🛠️ Custom Functions

You can extend the engine with your own custom logic by implementing the `AsyncFunctionHandler` trait. Note that in v1.0, functions receive a DataLogic instance for thread-safe JSONLogic evaluation.

```rust
use dataflow_rs::engine::{AsyncFunctionHandler, error::Result, message::{Change, Message}};
use async_trait::async_trait;
use serde_json::Value;
use datalogic_rs::DataLogic;

pub struct MyCustomFunction;

#[async_trait]
impl AsyncFunctionHandler for MyCustomFunction {
    async fn execute(
        &self, 
        message: &mut Message, 
        input: &Value,
        data_logic: &mut DataLogic  // New in v1.0: DataLogic instance provided
    ) -> Result<(usize, Vec<Change>)> {
        // Your custom async logic here
        // Use data_logic for any JSONLogic evaluation
        println!("Hello from a thread-safe custom function!");
        
        // Example: evaluate JSONLogic
        let logic = serde_json::json!({"var": "data.field"});
        let result = data_logic.apply(&logic, &message.data)?;
        
        Ok((200, vec![]))
    }
}

// Then, register it with the engine:
// engine.register_task_function("my_custom_function", Box::new(MyCustomFunction));
```

## 🤝 Contributing

We welcome contributions! Feel free to fork the repository, make your changes, and submit a pull request. Please make sure to add tests for any new features.

## 🏢 About Plasmatic

Dataflow-rs is developed by the team at [Plasmatic](https://github.com/GoPlasmatic). We're passionate about building open-source tools for data processing.

## 📄 License

This project is licensed under the Apache License, Version 2.0. See the [LICENSE](LICENSE) file for more details.