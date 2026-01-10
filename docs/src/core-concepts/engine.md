# Engine

The Engine is the central component that orchestrates message processing through workflows.

## Overview

The Engine is responsible for:

- Compiling all JSONLogic expressions at initialization
- Managing workflow execution order
- Processing messages through matching workflows
- Coordinating task execution

## Creating an Engine

```rust
use dataflow_rs::{Engine, Workflow};
use std::collections::HashMap;

// Parse workflows from JSON
let workflow1 = Workflow::from_json(r#"{
    "id": "workflow1",
    "name": "First Workflow",
    "priority": 1,
    "tasks": [...]
}"#)?;

let workflow2 = Workflow::from_json(r#"{
    "id": "workflow2",
    "name": "Second Workflow",
    "priority": 2,
    "tasks": [...]
}"#)?;

// Create engine with workflows
let engine = Engine::new(
    vec![workflow1, workflow2],
    None  // Optional custom functions
);

// Engine is now ready - all logic compiled
println!("Loaded {} workflows", engine.workflows().len());
```

## Processing Messages

```rust
use dataflow_rs::Message;
use serde_json::json;

// Create a message
let mut message = Message::new(&json!({
    "user": "john",
    "action": "login"
}));

// Process through all matching workflows
engine.process_message(&mut message).await?;

// Access results
println!("Processed data: {:?}", message.context["data"]);
println!("Audit trail: {:?}", message.audit_trail);
```

## Workflow Execution Order

Workflows execute in priority order (lowest priority number first):

```rust
// Priority 1 executes first
let high_priority = Workflow::from_json(r#"{
    "id": "high",
    "priority": 1,
    "tasks": [...]
}"#)?;

// Priority 10 executes later
let low_priority = Workflow::from_json(r#"{
    "id": "low",
    "priority": 10,
    "tasks": [...]
}"#)?;
```

## Workflow Conditions

Workflows can have conditions that determine if they should execute:

```json
{
    "id": "user_workflow",
    "name": "User Workflow",
    "condition": { "==": [{"var": "metadata.type"}, "user"] },
    "tasks": [...]
}
```

The workflow only executes if the condition evaluates to true.

## Custom Functions

Register custom functions when creating the engine:

```rust
use dataflow_rs::engine::AsyncFunctionHandler;
use std::collections::HashMap;

let mut custom_functions: HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>> = HashMap::new();
custom_functions.insert("my_function".to_string(), Box::new(MyCustomFunction));

let engine = Engine::new(workflows, Some(custom_functions));
```

## Thread Safety

The Engine is designed for concurrent use:

- Workflows are immutable after creation
- Compiled logic is shared via `Arc`
- Each message is processed independently

```rust
use std::sync::Arc;
use tokio::task;

let engine = Arc::new(Engine::new(workflows, None));

// Process multiple messages concurrently
let handles: Vec<_> = messages.into_iter().map(|mut msg| {
    let engine = Arc::clone(&engine);
    task::spawn(async move {
        engine.process_message(&mut msg).await
    })
}).collect();

// Wait for all to complete
for handle in handles {
    handle.await??;
}
```

## API Reference

### `Engine::new(workflows, custom_functions)`

Creates a new engine with the given workflows and optional custom functions.

- `workflows: Vec<Workflow>` - Workflows to register
- `custom_functions: Option<HashMap<String, Box<dyn AsyncFunctionHandler>>>` - Custom function implementations

### `engine.process_message(&mut message)`

Processes a message through all matching workflows.

- Returns `Result<()>` - Ok if processing succeeded
- Message is modified in place with results and audit trail

### `engine.workflows()`

Returns a reference to the registered workflows.

```rust
let workflow_ids: Vec<&String> = engine.workflows().keys().collect();
```
