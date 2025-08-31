/*!
# Dataflow-rs

A lightweight, rule-driven workflow engine for building powerful data processing pipelines and nanoservices in Rust.

## Overview

Dataflow-rs provides a flexible and extensible framework for processing data through a series of tasks organized in workflows.
The engine automatically routes messages through appropriate workflows based on configurable rules, and each workflow can
contain multiple tasks that transform, validate, or enrich the data.

## Key Components

* **Engine**: The central component that processes messages through workflows
* **Workflow**: A collection of tasks with conditions that determine when they should be applied
* **Task**: An individual processing unit that performs a specific function on a message
* **AsyncFunctionHandler**: A trait implemented by task handlers to define custom async processing logic
* **Message**: The data structure that flows through the engine, containing payload, metadata, and processing results

## Built-in Functions

The engine comes with several pre-registered functions:

* **http**: Fetches data from external HTTP APIs
* **map**: Maps and transforms data between different parts of a message
* **validate**: Validates message data against rules using JSONLogic expressions

## Async Support

The engine fully supports asynchronous operation with Tokio, providing improved scalability and
performance for IO-bound operations like HTTP requests:

```rust
use dataflow_rs::{Engine, Workflow};
use dataflow_rs::engine::message::Message;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create the async workflow engine
    let mut engine = Engine::new();

    // Define and add a workflow
    let workflow_json = r#"{
        "id": "data_processor",
        "name": "Data Processor",
        "priority": 0,
        "tasks": [
            {
                "id": "fetch_data",
                "name": "Fetch Data",
                "function": {
                    "name": "http",
                    "input": { "url": "https://api.example.com/data" }
                }
            }
        ]
    }"#;

    let workflow = Workflow::from_json(workflow_json)?;
    engine.add_workflow(&workflow);

    // Create and process a message
    let mut message = Message::new(&json!({}));

    // Process the message asynchronously
    engine.process_message(&mut message).await?;

    println!("Processed result: {}", message.data["result"]);
    Ok(())
}
```

## Usage Example

```rust
use dataflow_rs::{Engine, Workflow};
use dataflow_rs::engine::message::Message;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create the workflow engine (built-in functions are auto-registered)
    let mut engine = Engine::new();

    // Define a workflow in JSON
    let workflow_json = r#"
    {
        "id": "data_processor",
        "name": "Data Processor",
        "priority": 0,
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
    match engine.process_message(&mut message).await {
        Ok(_) => {
            println!("Processed result: {}", message.data["result"]);
        }
        Err(e) => {
            println!("Error in workflow: {:?}", e);
        }
    }

    Ok(())
}
```

## Error Handling

The library provides a comprehensive error handling system:

```rust
use dataflow_rs::{Engine, Result, DataflowError};
use dataflow_rs::engine::message::Message;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    let mut engine = Engine::new();
    // ... setup workflows ...

    let mut message = Message::new(&json!({}));

    // Process the message, errors will be collected but not halt execution
    engine.process_message(&mut message).await?;

    // Check if there were any errors during processing
    if message.has_errors() {
        for error in &message.errors {
            println!("Error in workflow: {:?}, task: {:?}: {:?}",
                     error.workflow_id, error.task_id, error.error_message);
        }
    }

    Ok(())
}
```

## Extending with Custom Functions

You can extend the engine with your own custom function handlers:

```rust
use dataflow_rs::{Engine, AsyncFunctionHandler, Result, Workflow};
use dataflow_rs::engine::{FunctionConfig, message::{Change, Message}, error::DataflowError};
use datalogic_rs::DataLogic;
use serde_json::{json, Value};
use async_trait::async_trait;

struct CustomFunction;

#[async_trait]
impl AsyncFunctionHandler for CustomFunction {
    async fn execute(&self, message: &mut Message, config: &FunctionConfig, _data_logic: &mut DataLogic) -> Result<(usize, Vec<Change>)> {
        // Implement your custom logic here

        // Extract the raw input from config
        let input = match config {
            FunctionConfig::Raw(input) => input,
            _ => return Err(DataflowError::Validation("Invalid configuration type".to_string())),
        };

        // Validate input
        let required_field = input.get("field")
            .ok_or_else(|| DataflowError::Validation("Missing required field".to_string()))?
            .as_str()
            .ok_or_else(|| DataflowError::Validation("Field must be a string".to_string()))?;

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

#[tokio::main]
async fn main() -> Result<()> {
    let mut engine = Engine::new();

    // Register your custom function
    engine.register_task_function("custom".to_string(), Box::new(CustomFunction));

    // Now it can be used in workflows...
    Ok(())
}
```
*/

pub mod engine;

// Re-export all public APIs for easier access
pub use async_trait::async_trait;
pub use engine::RetryConfig;
pub use engine::error::{DataflowError, ErrorInfo, Result};
pub use engine::{AsyncFunctionHandler, Engine, Task, Workflow};
