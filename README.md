<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  # Dataflow-rs

  **A lightweight, async-first workflow engine for building powerful data processing pipelines in Rust.**

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)](https://www.rust-lang.org)
  [![Crates.io](https://img.shields.io/crates/v/dataflow-rs.svg)](https://crates.io/crates/dataflow-rs)
</div>

---

Dataflow-rs is a Rust library for creating high-performance, asynchronous data processing pipelines. It's designed to be flexible and scalable, allowing you to build complex workflows with ease. Whether you're building a simple nanoservice or a sophisticated data transformation pipeline, Dataflow-rs provides the tools you need to get the job done.

## 🚀 Key Features

- **Asynchronous by Design:** Built on top of Tokio for non-blocking, high-performance I/O.
- **Dynamic Workflows:** Use JSONLogic to control workflow execution based on your data.
- **Extensible:** Easily add your own custom processing steps (tasks) to the engine.
- **Built-in Functions:** Comes with a set of common tasks like HTTP requests, data mapping, and validation.
- **Resilient:** Built-in error handling and retry mechanisms to handle transient failures.
- **Auditing:** Keep track of all the changes that happen to your data as it moves through the pipeline.

## 🏁 Getting Started

Here's a quick example to get you up and running.

### 1. Add to `Cargo.toml`

```toml
[dependencies]
dataflow-rs = "0.1.29"
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
    // Create a new engine
    let mut engine = Engine::new();

    // Define and add a workflow
    let workflow_json = r#"{ ... }"#; // Your workflow JSON from above
    let workflow = Workflow::from_json(workflow_json)?;
    engine.add_workflow(&workflow);

    // Create a message and process it
    let mut message = Message::new(&json!({}));
    engine.process_message(&mut message).await?;

    println!("✅ Processed result: {}", serde_json::to_string_pretty(&message.data)?);

    Ok(())
}
```

## ✨ Core Concepts

- **Engine:** The heart of the library, responsible for processing messages.
- **Workflow:** A sequence of tasks that are executed in order.
- **Task:** A single step in a workflow, like making an HTTP request or transforming data.
- **Message:** The data that flows through the engine, which can be modified by tasks.

## 🛠️ Custom Functions

You can extend the engine with your own custom logic by implementing the `AsyncFunctionHandler` trait.

```rust
use dataflow_rs::engine::{AsyncFunctionHandler, error::Result, message::{Change, Message}};
use async_trait::async_trait;
use serde_json::Value;

pub struct MyCustomFunction;

#[async_trait]
impl AsyncFunctionHandler for MyCustomFunction {
    async fn execute(&self, message: &mut Message, input: &Value) -> Result<(usize, Vec<Change>)> {
        // Your custom async logic here
        println!("Hello from a custom function!");
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