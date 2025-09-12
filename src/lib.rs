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

* **map**: Maps and transforms data between different parts of a message
* **validate**: Validates message data against rules using JSONLogic expressions

## Usage Example

```rust,no_run
use dataflow_rs::{Engine, Workflow};
use dataflow_rs::engine::message::Message;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define a workflow in JSON
    let workflow_json = r#"
    {
        "id": "data_processor",
        "name": "Data Processor",
        "priority": 0,
        "tasks": [
            {
                "id": "transform_data",
                "name": "Transform Data",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "data.result",
                                "logic": { "var": "temp_data.value" }
                            }
                        ]
                    }
                }
            }
        ]
    }
    "#;

    // Parse the workflow
    let workflow = Workflow::from_json(workflow_json)?;

    // Create the workflow engine with the workflow (built-in functions are auto-registered by default)
    let engine = Engine::new(vec![workflow], None);

    // Create a message to process
    let mut message = Message::from_value(&json!({}));

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

```rust,no_run
use dataflow_rs::{Engine, Result, DataflowError};
use dataflow_rs::engine::message::Message;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    // ... setup workflows ...
    let engine = Engine::new(vec![/* workflows */], None);

    let mut message = Message::from_value(&json!({}));

    // Process the message, errors will be collected but not halt execution
    engine.process_message(&mut message).await?;

    // Check if there were any errors during processing
    if message.has_errors() {
        for error in &message.errors {
            println!("Error in workflow: {:?}, task: {:?}: {:?}",
                     error.workflow_id, error.task_id, error.message);
        }
    }

    Ok(())
}
```

## Extending with Custom Functions

You can extend the engine with your own custom function handlers:

```rust,no_run
use dataflow_rs::{Engine, FunctionHandler, Result, Workflow};
use dataflow_rs::engine::{FunctionConfig, message::{Change, Message}, error::DataflowError};
use datalogic_rs::DataLogic;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

struct CustomFunction;

impl FunctionHandler for CustomFunction {
    fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
        datalogic: &DataLogic,
    ) -> Result<(usize, Vec<Change>)> {
        // Implement your custom logic here

        // Extract the custom configuration from config
        let input = match config {
            FunctionConfig::Custom { input, .. } => input,
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
                path: Arc::from("data.custom_field"),
                old_value: Arc::new(Value::Null),
                new_value: Arc::new(json!("custom value")),
            }
        ];

        // Return success code (200) and changes
        Ok((200, changes))
    }
}

fn main() -> Result<()> {
    // Create custom functions
    let mut custom_functions = HashMap::new();
    custom_functions.insert(
        "custom".to_string(),
        Box::new(CustomFunction) as Box<dyn FunctionHandler + Send + Sync>
    );

    // Create engine with workflows and custom functions (using from_sync_handlers for FunctionHandler)
    let engine = Engine::from_sync_handlers(vec![/* workflows */], Some(custom_functions));

    // Now it can be used in workflows...
    Ok(())
}
```
*/

pub mod engine;

// Re-export all public APIs for easier access
pub use engine::error::{DataflowError, ErrorInfo, Result};
pub use engine::functions::{
    AsyncFunctionHandler, FunctionConfig, MapConfig, MapMapping, ValidationConfig, ValidationRule,
};
pub use engine::message::{AuditTrail, Change, Message};
pub use engine::{Engine, FunctionHandler, Task, Workflow};
