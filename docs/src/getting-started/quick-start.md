# Quick Start

Build your first rule in minutes.

## Create a Simple Rule

Rules are defined in JSON and consist of actions (tasks) that process data sequentially.

```rust
use dataflow_rs::{Engine, Workflow};
use dataflow_rs::engine::message::Message;
use dataflow_rs::engine::utils::set_nested_value;
use datavalue::OwnedDataValue;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define a rule that transforms data
    let rule_json = r#"{
        "id": "greeting_rule",
        "name": "Greeting Rule",
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

    // Parse the rule
    let rule = Workflow::from_json(rule_json)?;

    // Create the engine (compiles all logic at startup)
    let engine = Engine::new(vec![rule], None)?;

    // Create a message and seed `data.name`
    let mut message = Message::from_value(&json!({"name": "World"}));
    set_nested_value(
        &mut message.context,
        "data.name",
        OwnedDataValue::from(&json!("World")),
    );

    // Process the message
    engine.process_message(&mut message).await?;

    // Print the result
    println!("Greeting: {:?}", message.data()["greeting"]);

    Ok(())
}
```

In a typical pipeline you'd let a `parse_json` task seed `data` from the
payload rather than calling `set_nested_value` from Rust — see the
playground widget below for that pattern.

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
