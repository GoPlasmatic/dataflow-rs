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
* **FunctionHandler**: A trait implemented by task handlers to define custom processing logic
* **Message**: The data structure that flows through the engine, containing payload, metadata, and processing results

## Built-in Functions

The engine comes with several pre-registered functions:

* **http**: Fetches data from external HTTP APIs
* **map**: Maps and transforms data between different parts of a message
* **validate**: Validates message data against rules using JSONLogic expressions

## Usage Example

```rust
use dataflow_rs::{Engine, Workflow};
use dataflow_rs::engine::message::Message;
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    engine.process_message(&mut message);

    println!("Processed result: {}", message.data["result"]);

    Ok(())
}
```

## Extending with Custom Functions

You can extend the engine with your own custom function handlers:

```rust
use dataflow_rs::{Engine, FunctionHandler, Workflow};
use dataflow_rs::engine::message::{Change, Message};
use serde_json::{json, Value};

struct CustomFunction;

impl FunctionHandler for CustomFunction {
    fn execute(&self, message: &mut Message, input: &Value) -> Result<(usize, Vec<Change>), String> {
        // Implement your custom logic here

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

fn main() {
    let mut engine = Engine::new();

    // Register your custom function
    engine.register_task_function("custom".to_string(), Box::new(CustomFunction));

    // Now it can be used in workflows...
}
```
*/

pub mod engine;

// Re-export all public APIs for easier access
pub use engine::{Engine, FunctionHandler, Task, Workflow};
