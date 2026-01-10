# Quick Start

Build your first data processing workflow in minutes.

## Create a Simple Workflow

Workflows are defined in JSON and consist of tasks that process data sequentially.

```rust
use dataflow_rs::{Engine, Workflow, Message};
use serde_json::json;

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

    // Create a message to process
    let mut message = Message::new(&json!({
        "name": "World"
    }));

    // Process the message
    engine.process_message(&mut message).await?;

    // Print the result
    println!("Result: {}", serde_json::to_string_pretty(&message.context)?);

    Ok(())
}
```

## Try It Interactively

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
                            "condition": { "!!": {"var": "data.name"} },
                            "error_message": "Name is required"
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
