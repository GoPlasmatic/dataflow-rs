/*!
# Dataflow-rs

A lightweight rules engine for building IFTTT-style automation and data processing pipelines in Rust.

## Overview

Dataflow-rs provides a high-performance rules engine that follows the **IF → THEN → THAT** model:

- **IF** — Define conditions using JSONLogic expressions (evaluated against `data`, `metadata`, `temp_data`)
- **THEN** — Execute actions: data transformation, validation, or custom async logic
- **THAT** — Chain multiple actions and rules with priority ordering

Rules are defined declaratively in JSON and compiled once at startup for zero-overhead evaluation at runtime.

## Key Components

| Rules Engine | Workflow Engine | Description |
|---|---|---|
| **RulesEngine** | **Engine** | Central async component that evaluates rules and executes actions |
| **Rule** | **Workflow** | A condition + actions bundle — IF condition THEN execute actions |
| **Action** | **Task** | An individual processing step that performs a function on a message |

* **AsyncFunctionHandler**: A trait implemented by action handlers to define custom async processing logic
* **TaskContext**: Per-call context handed to handlers — typed data accessors, audit-trail-aware setters
* **TaskOutcome**: Return value of a handler — `Success`, `Status(code)`, `Skip`, or `Halt`
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
    let engine = Engine::new(vec![workflow], None)?;

    // Create a message to process
    let mut message = Message::from_value(&json!({}));

    // Process the message through the workflow
    match engine.process_message(&mut message).await {
        Ok(_) => {
            println!("Processed result: {}", message.context["data"]["result"]);
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
    let engine = Engine::new(vec![/* workflows */], None)?;

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

Implement `AsyncFunctionHandler` with a typed `Input` so the engine deserializes
your config once at startup; handlers then receive typed input and a
`TaskContext` that records audit-trail changes automatically.

```rust,no_run
use dataflow_rs::{
    AsyncFunctionHandler, BoxedFunctionHandler, Engine, Result, TaskContext, TaskOutcome, Workflow,
};
use datavalue::OwnedDataValue;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use async_trait::async_trait;

#[derive(Deserialize)]
struct StatsInput {
    /// Path inside `data` whose array of numbers to summarize.
    source: String,
    /// Path inside `data` to write the result to.
    target: String,
}

struct Statistics;

#[async_trait]
impl AsyncFunctionHandler for Statistics {
    type Input = StatsInput;

    async fn execute(
        &self,
        ctx: &mut TaskContext<'_>,
        input: &StatsInput,
    ) -> Result<TaskOutcome> {
        let count = ctx.data()
            .get(input.source.as_str())
            .and_then(|v| v.as_array())
            .map(|arr| arr.len())
            .unwrap_or(0);

        ctx.set(
            &format!("data.{}", input.target),
            OwnedDataValue::from(&json!({ "count": count })),
        );
        Ok(TaskOutcome::Success)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut custom_functions: HashMap<String, BoxedFunctionHandler> = HashMap::new();
    custom_functions.insert("statistics".to_string(), Box::new(Statistics));

    let engine = Engine::new(vec![/* workflows */], Some(custom_functions))?;
    // ...
    Ok(())
}
```
*/

pub mod engine;

// Re-export all public APIs for easier access
pub use engine::error::{DataflowError, ErrorInfo, Result};
pub use engine::functions::{
    AsyncFunctionHandler, BoxedFunctionHandler, EnrichConfig, FilterConfig, FunctionConfig,
    HttpCallConfig, LogConfig, MapConfig, MapMapping, PublishKafkaConfig, ValidationConfig,
    ValidationRule,
};
pub use engine::message::{AuditTrail, Change, Message};
pub use engine::task_context::TaskContext;
pub use engine::task_outcome::TaskOutcome;
pub use engine::trace::{ExecutionStep, ExecutionTrace, StepResult};
pub use engine::{Engine, Task, Workflow, WorkflowStatus};

/// Type alias for `Workflow` — a Rule represents an IF-THEN unit: IF condition THEN execute actions.
pub type Rule = Workflow;

/// Type alias for `Task` — an Action is an individual processing step within a rule.
pub type Action = Task;

/// Type alias for `Engine` — the RulesEngine evaluates rules and executes their actions.
pub type RulesEngine = Engine;
