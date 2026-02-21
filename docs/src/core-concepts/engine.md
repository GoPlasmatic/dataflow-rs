# Rules Engine

The Engine (also available as `RulesEngine` type alias) is the central component that evaluates rules and orchestrates action execution.

## Overview

The Engine is responsible for:

- Compiling all JSONLogic expressions at initialization
- Managing rule execution order by priority
- Evaluating rule conditions against the full message context
- Processing messages through matching rules
- Coordinating action execution

## Creating an Engine

```rust
use dataflow_rs::{Engine, Workflow};
use std::collections::HashMap;

// Parse rules from JSON
let rule1 = Workflow::from_json(r#"{
    "id": "rule1",
    "name": "First Rule",
    "priority": 1,
    "tasks": [...]
}"#)?;

let rule2 = Workflow::from_json(r#"{
    "id": "rule2",
    "name": "Second Rule",
    "priority": 2,
    "tasks": [...]
}"#)?;

// Create engine with rules
let engine = Engine::new(
    vec![rule1, rule2],
    None  // Optional custom functions
);

// Engine is now ready - all logic compiled
println!("Loaded {} rules", engine.workflows().len());
```

You can also use the `RulesEngine` type alias:

```rust
use dataflow_rs::RulesEngine;

let engine = RulesEngine::new(vec![rule1, rule2], None);
```

## Processing Messages

```rust
use dataflow_rs::engine::message::Message;
use serde_json::json;
use std::sync::Arc;

// Create a message with payload
let payload = Arc::new(json!({
    "user": "john",
    "action": "login"
}));
let mut message = Message::new(payload);

// Process through all matching rules
engine.process_message(&mut message).await?;

// Access results
println!("Processed data: {:?}", message.data());
println!("Audit trail: {:?}", message.audit_trail);
```

## Execution Tracing

For debugging, use `process_message_with_trace` to capture step-by-step execution:

```rust
let (mut message, trace) = engine.process_message_with_trace(&mut message).await?;

println!("Steps executed: {}", trace.executed_count());
println!("Steps skipped: {}", trace.skipped_count());

for step in &trace.steps {
    println!("Rule: {}, Action: {:?}, Result: {:?}",
        step.workflow_id, step.task_id, step.result);
}
```

## Rule Execution Order

Rules execute in priority order (lowest priority number first):

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

## Rule Conditions

Rules have conditions that determine if they should execute. Conditions are evaluated against the **full message context** — `data`, `metadata`, and `temp_data`:

```json
{
    "id": "premium_order",
    "name": "Premium Order Processing",
    "condition": { ">=": [{"var": "data.order.total"}, 1000] },
    "tasks": [...]
}
```

The rule only executes if the condition evaluates to true.

## Custom Functions

Register custom action handlers when creating the engine:

```rust
use dataflow_rs::engine::AsyncFunctionHandler;
use std::collections::HashMap;

let mut custom_functions: HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>> = HashMap::new();
custom_functions.insert("my_function".to_string(), Box::new(MyCustomFunction));

let engine = Engine::new(rules, Some(custom_functions));
```

## Thread Safety

The Engine is designed for concurrent use:

- Rules are immutable after creation
- Compiled logic is shared via `Arc`
- Each message is processed independently

```rust
use std::sync::Arc;
use tokio::task;

let engine = Arc::new(Engine::new(rules, None));

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

Creates a new engine with the given rules and optional custom functions.

- `workflows: Vec<Workflow>` - Rules to register
- `custom_functions: Option<HashMap<String, Box<dyn AsyncFunctionHandler>>>` - Custom action implementations

### `engine.process_message(&mut message)`

Processes a message through all matching rules.

- Returns `Result<()>` - Ok if processing succeeded
- Message is modified in place with results and audit trail

### `engine.process_message_with_trace(&mut message)`

Processes a message and returns an execution trace for debugging.

- Returns `Result<ExecutionTrace>` - Contains all execution steps with message snapshots
- Useful for step-by-step debugging and visualization

### `engine.workflows()`

Returns a reference to the registered rules.

```rust
let rule_ids: Vec<&String> = engine.workflows().keys().collect();
```
