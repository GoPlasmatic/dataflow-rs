<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  # Dataflow-rs

  **A high-performance workflow engine for building data processing pipelines in Rust with zero-overhead JSONLogic evaluation.**

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![Rust](https://img.shields.io/badge/rust-1.85+-orange.svg)](https://www.rust-lang.org)
  [![Crates.io](https://img.shields.io/crates/v/dataflow-rs.svg)](https://crates.io/crates/dataflow-rs)
</div>

---

Dataflow-rs is a Rust library for creating high-performance data processing pipelines with pre-compiled JSONLogic and zero runtime overhead. It features an async-first architecture that separates compilation from execution, ensuring predictable low-latency performance. Whether you're building REST APIs, processing Kafka streams, or creating sophisticated data transformation pipelines, Dataflow-rs provides enterprise-grade performance with minimal complexity.

## 🚀 Key Features

- **Async-First Architecture:** Native async/await support with Tokio for high-throughput processing.
- **Zero Runtime Compilation:** All JSONLogic expressions pre-compiled at startup for optimal performance.
- **Execution Tracing:** Step-by-step debugging with message snapshots after each task.
- **Built-in Functions:** Parse (JSON/XML), Map, Validate, and Publish (JSON/XML) for complete data pipelines.
- **Dynamic Workflows:** Use JSONLogic to control workflow execution based on your data.
- **Extensible:** Easily add your own custom async processing steps (tasks) to the engine.
- **WebAssembly Support:** Run workflows in the browser with `@goplasmatic/dataflow-wasm`.
- **React UI Components:** Visualize and debug workflows with `@goplasmatic/dataflow-ui`.
- **Auditing:** Keep track of all the changes that happen to your data as it moves through the pipeline.

## 🏁 Getting Started

Here's a quick example to get you up and running.

### 1. Add to `Cargo.toml`

```toml
[dependencies]
dataflow-rs = "2.0"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
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
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define workflows
    let workflow_json = r#"{ ... }"#; // Your workflow JSON from above
    let workflow = Workflow::from_json(workflow_json)?;

    // Create engine with workflows (compiled once at creation)
    let engine = Engine::new(vec![workflow], None);

    // Process a single message
    let payload = Arc::new(json!({"name": "Alice", "email": "alice@example.com"}));
    let mut message = Message::new(payload);
    engine.process_message(&mut message).await?;

    println!("Processed: {}", serde_json::to_string_pretty(message.data())?);
    Ok(())
}
```

## ✨ Core Concepts

- **Engine:** Async-first engine with pre-compiled logic and immutable workflows.
- **Workflow:** A sequence of tasks executed in order, with JSONLogic conditions.
- **Task:** A single async processing step with optional conditions.
- **Message:** The data structure flowing through workflows with `data`, `metadata`, `temp_data`, `payload`, and audit trail.
- **ExecutionTrace:** Step-by-step debugging with message snapshots after each task execution.

## 🏗️ Architecture

The v2.0 architecture uses an async-first design with pre-compiled JSONLogic for optimal performance:

### Compilation Phase (Startup)
1. All JSONLogic expressions compiled once when the Engine is created
2. Compiled logic cached with Arc for zero-copy sharing
3. Validates all expressions early, failing fast on errors

### Execution Phase (Runtime)
1. **Engine** orchestrates async message processing through workflows
2. Built-in functions execute with pre-compiled logic (zero compilation overhead)
3. `process_message()` for normal execution, `process_message_with_trace()` for debugging
4. Each task can be async, enabling I/O operations without blocking

### Key Design Decisions
- **Async-First:** Native async/await with Tokio for high-throughput processing
- **Immutable Workflows:** All workflows defined at engine creation
- **Pre-compilation:** All parsing/compilation done once at startup
- **Execution Tracing:** Optional step-by-step debugging with message snapshots

## ⚡ Performance

Dataflow-rs achieves optimal performance through architectural improvements:

- **Pre-Compilation:** All JSONLogic compiled at startup, zero runtime overhead
- **Arc-Wrapped Logic:** Zero-copy sharing of compiled expressions
- **Context Arc Caching:** 50% improvement via cached Arc context
- **Async I/O:** Non-blocking operations for external services
- **Predictable Latency:** No runtime allocations for logic evaluation

Run the included examples to test performance on your hardware:
```bash
cargo run --example benchmark           # Performance benchmark
cargo run --example custom_function     # Custom async function implementation
cargo run --example complete_workflow   # Parse → Transform → Validate pipeline
```

## 🛠️ Custom Functions

You can extend the engine with your own custom logic by implementing the `AsyncFunctionHandler` trait:

```rust
use async_trait::async_trait;
use dataflow_rs::engine::{
    AsyncFunctionHandler, FunctionConfig,
    error::Result, message::{Change, Message}
};
use datalogic_rs::DataLogic;
use serde_json::json;
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
        // Your custom async logic here
        let old_value = message.data().get("processed").cloned().unwrap_or(json!(null));

        // Modify message data
        if let Some(data) = message.data_mut().as_object_mut() {
            data.insert("processed".to_string(), json!(true));
        }
        message.invalidate_context_cache();

        // Return status code and changes for audit trail
        Ok((200, vec![Change {
            path: Arc::from("data.processed"),
            old_value: Arc::new(old_value),
            new_value: Arc::new(json!(true)),
        }]))
    }
}

// Register when creating the engine:
let mut custom_functions: HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>> = HashMap::new();
custom_functions.insert("my_custom".to_string(), Box::new(MyCustomFunction));

let engine = Engine::new(workflows, Some(custom_functions));
```

## 📦 Built-in Functions

| Function | Purpose | Modifies Data |
|----------|---------|---------------|
| `parse_json` | Parse JSON from payload into data context | Yes |
| `parse_xml` | Parse XML string into JSON data structure | Yes |
| `map` | Data transformation using JSONLogic | Yes |
| `validation` | Rule-based data validation | No (read-only) |
| `publish_json` | Serialize data to JSON string | Yes |
| `publish_xml` | Serialize data to XML string | Yes |

## 🌐 Related Packages

| Package | Description |
|---------|-------------|
| [@goplasmatic/dataflow-wasm](https://www.npmjs.com/package/@goplasmatic/dataflow-wasm) | WebAssembly bindings for browser execution |
| [@goplasmatic/dataflow-ui](https://www.npmjs.com/package/@goplasmatic/dataflow-ui) | React components for workflow visualization |

## 🤝 Contributing

We welcome contributions! Feel free to fork the repository, make your changes, and submit a pull request. Please make sure to add tests for any new features.

## 🏢 About Plasmatic

Dataflow-rs is developed by the team at [Plasmatic](https://github.com/GoPlasmatic). We're passionate about building open-source tools for data processing.

## 📄 License

This project is licensed under the Apache License, Version 2.0. See the [LICENSE](LICENSE) file for more details.