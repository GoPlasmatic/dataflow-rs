<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  # Dataflow-rs

  **A high-performance workflow engine for building data processing pipelines in Rust with zero-overhead JSONLogic evaluation.**

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)](https://www.rust-lang.org)
  [![Crates.io](https://img.shields.io/crates/v/dataflow-rs.svg)](https://crates.io/crates/dataflow-rs)
</div>

---

Dataflow-rs is a Rust library for creating high-performance data processing pipelines with pre-compiled JSONLogic and zero runtime overhead. It features a modular architecture that separates compilation from execution, ensuring predictable low-latency performance. Whether you're building REST APIs, processing Kafka streams, or creating sophisticated data transformation pipelines, Dataflow-rs provides enterprise-grade performance with minimal complexity.

## 🚀 Key Features

- **Zero Runtime Compilation:** All JSONLogic expressions pre-compiled at startup for optimal performance.
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

## ✨ Core Concepts

- **Engine:** High-performance engine with pre-compiled logic and immutable workflows.
- **LogicCompiler:** Compiles all JSONLogic expressions at initialization for zero runtime overhead.
- **InternalExecutor:** Executes built-in functions using pre-compiled logic from the cache.
- **Workflow:** A sequence of tasks executed in order, with conditions accessing only metadata.
- **Task:** A single processing step with optional JSONLogic conditions.
- **Message:** The data structure flowing through workflows with audit trail support.

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
- **Cache-Friendly:** Compiled logic stored contiguously in memory
- **Direct Instantiation:** DataLogic instances created directly without locking
- **Predictable Latency:** No runtime allocations for logic evaluation
- **Modular Design:** Clear separation of compilation and execution phases

Run the included benchmarks to test performance on your hardware:
```bash
cargo run --example benchmark           # Performance benchmark
cargo run --example custom_function     # Custom function implementation
cargo run --example complete_workflow   # Complete workflow example
```

## 🛠️ Custom Functions

You can extend the engine with your own custom logic by implementing the `AsyncFunctionHandler` trait:

```rust
use async_trait::async_trait;
use dataflow_rs::engine::{AsyncFunctionHandler, FunctionConfig, error::Result, message::{Change, Message}};
use datalogic_rs::DataLogic;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

pub struct MyCustomFunction;

#[async_trait]
impl AsyncFunctionHandler for MyCustomFunction {
    async fn execute(
        &self, 
        message: &mut Message, 
        config: &FunctionConfig,
        datalogic: Arc<DataLogic>,
    ) -> Result<(usize, Vec<Change>)> {
        // Your custom logic here (can be async or sync)
        println!("Hello from a custom async function!");
        
        // Modify message data
        message.data["processed"] = json!(true);
        
        // Return status code and changes for audit trail
        Ok((200, vec![Change {
            path: Arc::from("data.processed"),
            old_value: Arc::new(json!(null)),
            new_value: Arc::new(json!(true)),
        }]))
    }
}

// Register when creating the engine:
let mut custom_functions = HashMap::new();
custom_functions.insert(
    "my_custom_function".to_string(),
    Box::new(MyCustomFunction) as Box<dyn AsyncFunctionHandler + Send + Sync>
);

let engine = Engine::new(
    workflows,
    Some(custom_functions),  // Custom async functions
);
```

## 🤝 Contributing

We welcome contributions! Feel free to fork the repository, make your changes, and submit a pull request. Please make sure to add tests for any new features.

## 🏢 About Plasmatic

Dataflow-rs is developed by the team at [Plasmatic](https://github.com/GoPlasmatic). We're passionate about building open-source tools for data processing.

## 📄 License

This project is licensed under the Apache License, Version 2.0. See the [LICENSE](LICENSE) file for more details.