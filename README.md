# Dataflow-rs

[![Release Crates](https://github.com/codetiger/dataflow-rs/actions/workflows/crate-publish.yml/badge.svg)](https://github.com/codetiger/dataflow-rs/actions/workflows/crate-publish.yml)
[![Crates.io Version](https://img.shields.io/crates/v/dataflow-rs)](https://crates.io/crates/dataflow-rs)
[![License](https://img.shields.io/crates/l/dataflow-rs)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.60+-blue)](https://www.rust-lang.org)

Dataflow-rs is a lightweight, rule-driven workflow engine designed for building powerful data processing pipelines and nanoservices in Rust. Extend it with your custom tasks to create robust, maintainable services.

## Features

- **Rule-Based Workflow Selection:** Dynamically select workflows using JSONLogic expressions.
- **Task Orchestration:** Compose sequences of tasks for complex data processing.
- **Message Transformation:** Seamlessly modify message data via specialized tasks.
- **Audit Trails:** Automatically record changes for debugging and monitoring.
- **Pluggable Architecture:** Easily extend the framework by registering custom tasks.
- **Async Support:** Efficiently handle asynchronous tasks and HTTP requests.

## Table of Contents

- [Overview](#overview)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [Advanced Examples](#advanced-examples)
- [Extending the Framework](#extending-the-framework)
- [Documentation](#documentation)
- [Contributing](#contributing)
- [License](#license)

## Overview

Dataflow-rs empowers developers to build scalable nanoservices and data pipelines with ease. Its core design focuses on flexibility and extensibility, allowing you to integrate your custom business logic into robust workflows.

## Installation

To incorporate Dataflow-rs into your project, add the following to your `Cargo.toml`:

```toml
[dependencies]
dataflow-rs = "0.1.0"
```

## Quick Start

Below is a simple example demonstrating how to set up a workflow that generates a greeting message:

```rust
use dataflow_rs::{Engine, Workflow, FunctionHandler, DataLogic};
use dataflow_rs::engine::message::{Message, Change};
use datalogic_rs::{DataValue, arena::DataArena, FromJson};
use serde_json::json;

struct GreetingTask;

impl FunctionHandler for GreetingTask {
    fn execute<'a>(
        &self,
        message: &mut Message<'a>,
        _input: &DataValue,
        arena: &'a DataArena
    ) -> Result<Vec<Change<'a>>, String> {
        let name = message.payload.get("name").and_then(|v| v.as_str()).unwrap_or("Guest");
        let greeting = format!("Hello, {}!", name);
        let data_object = json!({"greeting": greeting});
        message.data = DataValue::from_json(&data_object, arena);
        let changes = vec![
            Change {
                path: "data.greeting".to_string(),
                old_value: DataValue::null(),
                new_value: DataValue::from_json(&json!(greeting), arena),
            }
        ];
        Ok(changes)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_logic = Box::leak(Box::new(DataLogic::default()));
    let mut engine = Engine::new(data_logic);
    engine.register_function("greet".to_string(), Box::new(GreetingTask));

    let workflow_json = r#"
    {
        "id": "greeting_workflow",
        "name": "Greeting Generator",
        "description": "Generates a greeting based on the payload name",
        "condition": { "==": [true, true] },
        "tasks": [
            {
                "id": "generate_greeting",
                "name": "Generate Greeting",
                "function": "greet",
                "condition": { "==": [true, true] },
                "input": {}
            }
        ]
    }
    "#;

    let mut workflow = Workflow::from_json(workflow_json)?;
    workflow.prepare(data_logic);
    engine.add_workflow(&workflow);

    let mut message = Message {
        id: "msg_001".to_string(),
        data: DataValue::from_json(&json!({}), data_logic.arena()),
        payload: DataValue::from_json(&json!({"name": "Alice"}), data_logic.arena()),
        metadata: DataValue::from_json(&json!({}), data_logic.arena()),
        temp_data: DataValue::from_json(&json!({}), data_logic.arena()),
        audit_trail: Vec::new(),
    };

    engine.process_message(&mut message);
    println!("Message processed: {:?}", message);
    Ok(())
}
```

## Advanced Examples

Dataflow-rs can also integrate with external APIs. For instance, the following example shows how to fetch data from a cat fact API:

```rust
use dataflow_rs::{Engine, Workflow, FunctionHandler};
use dataflow_rs::engine::message::{Message, Change};
use datalogic_rs::{arena::DataArena, DataLogic, DataValue, FromJson};
use reqwest::Client;
use tokio;
use serde_json::{json, Value};

struct CatFactTask {
    client: Client,
}

impl CatFactTask {
    fn new() -> Self {
        Self { client: Client::new() }
    }
}

impl FunctionHandler for CatFactTask {
    fn execute<'a>(
        &self,
        message: &mut Message<'a>,
        _input: &DataValue,
        arena: &'a DataArena
    ) -> Result<Vec<Change<'a>>, String> {
        let runtime = tokio::runtime::Runtime::new().map_err(|e| format!("Runtime error: {}", e))?;
        let url = "https://catfact.ninja/fact";
        let response_data = runtime.block_on(async {
            let response = self.client.get(url)
                .send()
                .await
                .map_err(|e| format!("HTTP request failed: {}", e))?;
            let json = response.json::<Value>()
                .await
                .map_err(|e| format!("JSON parse error: {}", e))?;
            Ok::<Value, String>(json)
        }).map_err(|e| e.to_string())?;

        let mut data_object = json!({});
        data_object["cat_fact"] = response_data.clone();
        message.data = DataValue::from_json(&data_object, arena);
        let changes = vec![
            Change {
                path: "data.cat_fact".to_string(),
                old_value: DataValue::null(),
                new_value: DataValue::from_json(&response_data, arena),
            }
        ];
        Ok(changes)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_logic = Box::leak(Box::new(DataLogic::default()));
    let mut engine = Engine::new(data_logic);
    engine.register_function("cat_fact".to_string(), Box::new(CatFactTask::new()));

    let workflow_json = r#"
    {
        "id": "cat_fact_workflow",
        "name": "Cat Fact Fetcher",
        "description": "Fetches random cat facts and enhances your data",
        "condition": { "==": [true, true] },
        "tasks": [
            {
                "id": "get_cat_fact",
                "name": "Get Cat Fact",
                "function": "cat_fact",
                "condition": { "==": [true, true] },
                "input": {}
            }
        ]
    }
    "#;

    let mut workflow = Workflow::from_json(workflow_json)?;
    workflow.prepare(data_logic);
    engine.add_workflow(&workflow);

    let mut message = Message {
        id: "msg_001".to_string(),
        data: DataValue::from_json(&json!({}), data_logic.arena()),
        payload: DataValue::from_json(&json!({}), data_logic.arena()),
        metadata: DataValue::from_json(&json!({}), data_logic.arena()),
        temp_data: DataValue::from_json(&json!({}), data_logic.arena()),
        audit_trail: Vec::new(),
    };

    engine.process_message(&mut message);
    println!("Message processed: {:?}", message);
    Ok(())
}
```

## Extending the Framework

Dataflow-rs is highly extensible. You can:

- Implement custom tasks by creating structs that implement the `FunctionHandler` trait.
- Build nanoservices by integrating multiple workflows.
- Leverage asynchronous and external API integrations for enriched data processing.

## Documentation

For detailed API documentation and additional examples, please visit:

- [API Documentation](https://docs.rs/dataflow-rs)
- [GitHub Discussions](https://github.com/codetiger/dataflow-rs/discussions)

## Contributing

We welcome contributions! Check out our [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines on how to help improve Dataflow-rs.

## License

This project is licensed under the Apache License 2.0. See the [LICENSE](LICENSE) file for details.
