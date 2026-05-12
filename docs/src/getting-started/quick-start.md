# Quick Start

Build your first rule in minutes.

## Create a Simple Rule

Rules are defined in JSON and consist of actions (tasks) that process data sequentially.

```rust
use dataflow_rs::prelude::*;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define a rule that loads the payload into `data.input` and then
    // transforms it. Letting `parse_json` seed `data` is the idiomatic
    // pattern — handlers don't have to reach into `message.context`.
    let rule_json = r#"{
        "id": "greeting_rule",
        "name": "Greeting Rule",
        "tasks": [
            {
                "id": "load",
                "name": "Load Payload",
                "function": {
                    "name": "parse_json",
                    "input": { "source": "payload", "target": "input" }
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
                                "logic": { "cat": ["Hello, ", {"var": "data.input.name"}, "!"] }
                            }
                        ]
                    }
                }
            }
        ]
    }"#;

    let rule = Workflow::from_json(rule_json)?;

    // Builder is the recommended construction path. Compiles all
    // JSONLogic up-front; fails loud on bad config.
    let engine = Engine::builder().with_workflow(rule).build()?;

    // Create a message from a serde_json payload. `parse_json` will copy
    // it into `data.input` at workflow start.
    let mut message = Message::from_value(&json!({"name": "World"}));

    // Process the message.
    engine.process_message(&mut message).await?;

    // Print the result.
    println!("Greeting: {:?}", message.data()["greeting"]);

    Ok(())
}
```

## Try It Interactively

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution and rule visualization.

<div class="playground-widget" data-workflows='[{"id":"greeting_rule","name":"Greeting Rule","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"create_greeting","name":"Create Greeting","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"data.input.name"},"!"]}}]}}}]}]' data-payload='{"name":"World"}'>
</div>

## Understanding the Code

1. **Rule Definition** - JSON structure defining actions (tasks) to execute
2. **Engine Creation** - Compiles all JSONLogic expressions at startup
3. **Message Creation** - Input data wrapped in a Message structure
4. **Processing** - Engine evaluates each rule's condition and executes matching actions
5. **Result** - Modified message with transformed data and audit trail

## Add Validation

Extend your rule with data validation:

```json
{
    "id": "validated_rule",
    "name": "Validated Rule",
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
