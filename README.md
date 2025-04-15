# Dataflow-rs

[![Release Crates](https://github.com/codetiger/dataflow-rs/actions/workflows/crate-publish.yml/badge.svg)](https://github.com/codetiger/dataflow-rs/actions/workflows/crate-publish.yml)
[![Crates.io Version](https://img.shields.io/crates/v/dataflow-rs)](https://crates.io/crates/dataflow-rs)
[![License](https://img.shields.io/crates/l/dataflow-rs)](LICENSE)

Dataflow-rs is a lightweight, rule-driven workflow engine designed for building powerful data processing pipelines and nanoservices in Rust. Extend it with your custom tasks to create robust, maintainable services.

## Features

- **Rule-Based Workflow Selection:** Dynamically select workflows using JSONLogic expressions.
- **Task Orchestration:** Compose sequences of tasks for complex data processing.
- **Message Transformation:** Seamlessly modify message data via specialized tasks.
- **Comprehensive Error Handling:** Detailed error types and recovery mechanisms.
- **Retry Capabilities:** Configurable retry policies for transient failures.
- **Audit Trails:** Automatically record changes for debugging and monitoring.
- **Pluggable Architecture:** Easily extend the framework by registering custom tasks.
- **Thread-Safety:** Properly handles concurrent execution with thread-safe patterns.

## Table of Contents

- [Overview](#overview)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [Advanced Examples](#advanced-examples)
- [Error Handling](#error-handling)
- [Extending the Framework](#extending-the-framework)
- [Documentation](#documentation)
- [Contributing](#contributing)
- [License](#license)

## Overview

Dataflow-rs empowers developers to build scalable nanoservices and data pipelines with ease. Its core design focuses on flexibility, extensibility, and resilience, allowing you to integrate your custom business logic into robust workflows with proper error handling.

## Installation

To incorporate Dataflow-rs into your project, add the following to your `Cargo.toml`:

```toml
[dependencies]
dataflow-rs = "0.1.4"
```

## Quick Start

Below is a simple example demonstrating how to set up a workflow that processes data:

```rust
use dataflow_rs::{Engine, Workflow, Result};
use dataflow_rs::engine::message::Message;
use serde_json::json;

fn main() -> Result<()> {
    // Create the workflow engine (built-in functions are auto-registered)
    let mut engine = Engine::new();

    // Define a workflow in JSON
    let workflow_json = r#"
    {
        "id": "data_processor",
        "name": "Data Processor",
        "tasks": [
            {
                "id": "fetch_data",
                "name": "Fetch Data",
                "function": {
                    "name": "http",
                    "input": { "url": "https://api.example.com/data" }
                }
            },
            {
                "id": "transform_data",
                "name": "Transform Data",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "data.result",
                                "logic": { "var": "temp_data.body.value" }
                            }
                        ]
                    }
                }
            }
        ]
    }
    "#;

    // Parse and add the workflow to the engine
    let workflow = Workflow::from_json(workflow_json)?;
    engine.add_workflow(&workflow);

    // Create a message to process
    let mut message = Message::new(&json!({}));

    // Process the message through the workflow
    engine.process_message(&mut message)?;

    println!("Processed result: {}", message.data["result"]);

    Ok(())
}
```

## Advanced Examples

### Custom Function Handler

Extend the engine with your own custom function handlers:

```rust
use dataflow_rs::{Engine, FunctionHandler, Result, Workflow};
use dataflow_rs::engine::message::{Change, Message};
use dataflow_rs::engine::error::DataflowError;
use serde_json::{json, Value};

struct CustomFunction;

impl FunctionHandler for CustomFunction {
    fn execute(&self, message: &mut Message, input: &Value) -> Result<(usize, Vec<Change>)> {
        // Validate input
        let required_field = input.get("field")
            .ok_or_else(|| DataflowError::Validation("Missing required field".to_string()))?
            .as_str()
            .ok_or_else(|| DataflowError::Validation("Field must be a string".to_string()))?;

        // Implement your custom logic here
        println!("Processing with field: {}", required_field);

        // Record changes for audit trail
        let changes = vec![
            Change {
                path: "data.custom_field".to_string(),
                old_value: Value::Null,
                new_value: json!("custom value"),
            }
        ];

        // Return success code (200) and changes
        Ok((200, changes))
    }
}

fn main() -> Result<()> {
    let mut engine = Engine::new();

    // Register your custom function
    engine.register_task_function("custom".to_string(), Box::new(CustomFunction));

    // Use it in a workflow
    let workflow_json = r#"
    {
        "id": "custom_workflow",
        "name": "Custom Workflow",
        "tasks": [
            {
                "id": "custom_task",
                "name": "Custom Task",
                "function": {
                    "name": "custom",
                    "input": { "field": "example_value" }
                }
            }
        ]
    }
    "#;

    let workflow = Workflow::from_json(workflow_json)?;
    engine.add_workflow(&workflow);
    
    let mut message = Message::new(&json!({}));
    engine.process_message(&mut message)?;
    
    Ok(())
}
```

## Error Handling

Dataflow-rs provides comprehensive error handling with dedicated error types:

```rust
use dataflow_rs::{Engine, Result, DataflowError};
use dataflow_rs::engine::message::Message;
use serde_json::json;

fn main() -> Result<()> {
    let mut engine = Engine::new();
    // ... setup workflows ...
    
    let mut message = Message::new(&json!({}));
    
    // Configure message to continue processing despite errors
    message.set_continue_on_error(true);
    
    // Process the message, errors will be collected but not halt execution
    engine.process_message(&mut message)?;
    
    // Check if there were any errors during processing
    if message.has_errors() {
        for error in &message.errors {
            println!("Error in workflow: {:?}, task: {:?}: {:?}", 
                     error.workflow_id, error.task_id, error.error);
        }
    }
    
    Ok(())
}
```

### Retry Configuration

Configure retry behavior for transient failures:

```rust
use dataflow_rs::{Engine, RetryConfig};

fn main() {
    // Create an engine with custom retry configuration
    let engine = Engine::new()
        .with_retry_config(RetryConfig {
            max_retries: 5,
            retry_delay_ms: 500,
            use_backoff: true,
        });
    
    // Now any transient failures will be retried according to this configuration
}
```

## Extending the Framework

Dataflow-rs is highly extensible. You can:

- Implement custom tasks by creating structs that implement the `FunctionHandler` trait.
- Create your own error types by extending from the base `DataflowError`.
- Build nanoservices by integrating multiple workflows.
- Leverage the built-in HTTP, validation, and mapping functions.

## Built-in Functions

The engine comes with several pre-registered functions:

- **http**: Fetches data from external HTTP APIs
- **map**: Maps and transforms data between different parts of a message
- **validate**: Validates message data against rules using JSONLogic expressions

## Documentation

For detailed API documentation and additional examples, please visit:

- [API Documentation](https://docs.rs/dataflow-rs)
- [GitHub Discussions](https://github.com/codetiger/dataflow-rs/discussions)

## Contributing

We welcome contributions! Check out our [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines on how to help improve Dataflow-rs.

## License

This project is licensed under the Apache License 2.0. See the [LICENSE](LICENSE) file for details.
