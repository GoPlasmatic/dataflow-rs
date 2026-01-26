# Quick Start

Build your first data processing workflow in minutes.

## Create a Simple Workflow

Workflows are defined in JSON and consist of tasks that process data sequentially.

```rust
use dataflow_rs::{Engine, Workflow};
use dataflow_rs::engine::message::Message;
use serde_json::json;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define a workflow that transforms data
    let workflow_json = r#"{
        "id": "greeting_workflow",
        "name": "Greeting Workflow",
        "tasks": [
            {
                "id": "create_greeting",
                "name": "Create Greeting",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "data.greeting",
                                "logic": { "cat": ["Hello, ", {"var": "data.name"}, "!"] }
                            }
                        ]
                    }
                }
            }
        ]
    }"#;

    // Parse the workflow
    let workflow = Workflow::from_json(workflow_json)?;

    // Create the engine (compiles all logic at startup)
    let engine = Engine::new(vec![workflow], None);

    // Create a message with payload
    let payload = Arc::new(json!({"name": "World"}));
    let mut message = Message::new(payload);

    // Load payload into data context
    message.context["data"]["name"] = json!("World");

    // Process the message
    engine.process_message(&mut message).await?;

    // Print the result
    println!("Greeting: {}", message.data()["greeting"]);

    Ok(())
}
```

## Try It Interactively

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"greeting_workflow","name":"Greeting Workflow","tasks":[{"id":"create_greeting","name":"Create Greeting","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"data.name"},"!"]}}]}}}]}]' data-message='{"data":{"name":"World"},"metadata":{}}'>
</div>

## Understanding the Code

1. **Workflow Definition** - JSON structure defining tasks to execute
2. **Engine Creation** - Compiles all JSONLogic expressions at startup
3. **Message Creation** - Input data wrapped in a Message structure
4. **Processing** - Engine runs the message through all matching workflows
5. **Result** - Modified message with transformed data and audit trail

## Add Validation

Extend your workflow with data validation:

```json
{
    "id": "validated_workflow",
    "name": "Validated Workflow",
    "tasks": [
        {
            "id": "validate_input",
            "name": "Validate Input",
            "function": {
                "name": "validation",
                "input": {
                    "rules": [
                        {
                            "logic": { "!!": {"var": "data.name"} },
                            "message": "Name is required"
                        }
                    ]
                }
            }
        },
        {
            "id": "create_greeting",
            "name": "Create Greeting",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {
                            "path": "data.greeting",
                            "logic": { "cat": ["Hello, ", {"var": "data.name"}, "!"] }
                        }
                    ]
                }
            }
        }
    ]
}
```

## Next Steps

- [Basic Concepts](./basic-concepts.md) - Understand the core architecture
- [Map Function](../built-in-functions/map.md) - Learn about data transformation
- [Validation](../built-in-functions/validation.md) - Learn about data validation
