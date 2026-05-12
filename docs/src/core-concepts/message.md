# Message

A Message is the data container that flows through rules, carrying data, metadata, and an audit trail.

## Overview

The Message structure contains:

- **context.data** - Main data payload
- **context.metadata** - Message metadata (routing, source info)
- **context.temp_data** - Temporary processing data
- **audit_trail** - Record of all changes
- **errors** - Collected errors during processing

## Message Structure

```rust
use datavalue::OwnedDataValue;
use std::sync::Arc;

pub struct Message {
    pub id: String,                       // UUID v7 string by default
    pub payload: Arc<OwnedDataValue>,
    pub context: OwnedDataValue,          // Always an Object {data, metadata, temp_data}
    pub audit_trail: Vec<AuditTrail>,
    pub errors: Vec<ErrorInfo>,
    pub capture_changes: bool,            // In-memory only; not serialized
}
```

The context is structured as:

```json
{
    "data": { ... },
    "metadata": { ... },
    "temp_data": { ... }
}
```

## Creating Messages

### Basic Creation

```rust
use dataflow_rs::Message;
use serde_json::json;

// `from_value` bridges from serde_json::Value — handiest when you already
// have JSON literals. The payload lands at `message.payload`; the context
// starts empty with the canonical {data, metadata, temp_data} shape.
let mut message = Message::from_value(&json!({
    "name": "John",
    "email": "john@example.com"
}));
```

### Native Construction (Zero-Conversion)

```rust
use datavalue::OwnedDataValue;
use std::sync::Arc;

let payload = Arc::new(OwnedDataValue::from(&json!({
    "name": "John"
})));
let mut message = Message::new(payload);
```

### Populating the Context

`message.context` is an `OwnedDataValue` tree — it doesn't support
`serde_json`-style `IndexMut` assignment. Use the path helpers from
`dataflow_rs::engine::utils`:

```rust
use dataflow_rs::engine::utils::set_nested_value;
use datavalue::OwnedDataValue;
use serde_json::json;

set_nested_value(
    &mut message.context,
    "metadata.source",
    OwnedDataValue::from(&json!("api")),
);
set_nested_value(
    &mut message.context,
    "metadata.type",
    OwnedDataValue::from(&json!("user")),
);
```

In practice you rarely need to mutate the context directly from Rust: the
`parse_json` / `map` / `validation` built-ins are how your workflows
populate it.

## Context Fields

### data

The main data payload. This is where your primary data lives and is transformed.

```rust
use dataflow_rs::engine::utils::{get_nested_value, set_nested_value};
use datavalue::OwnedDataValue;
use serde_json::json;

// Write a value at data.name
set_nested_value(
    &mut message.context,
    "data.name",
    OwnedDataValue::from(&json!("John")),
);

// Read it back (returns Option<&OwnedDataValue>)
let name = get_nested_value(&message.context, "data.name");

// Or, via the convenience accessor — returns &OwnedDataValue (Null if missing)
let name = &message.data()["name"];
```

### metadata

Information about the message itself (not the data). Commonly used for:

- Routing decisions (rule conditions)
- Source tracking
- Timestamps
- Message type classification

```rust
set_nested_value(
    &mut message.context,
    "metadata",
    OwnedDataValue::from(&json!({
        "type": "user",
        "source": "webhook",
        "received_at": "2024-01-01T00:00:00Z"
    })),
);
```

The engine also stamps `metadata.processed_at` and
`metadata.engine_version` automatically on every `process_message` call.

### temp_data

Temporary storage for intermediate processing results. Cleared between processing runs if needed.

```rust
// Store intermediate result
set_nested_value(
    &mut message.context,
    "temp_data.calculated_value",
    OwnedDataValue::from(&json!(42)),
);

// Use in a later task
// {"var": "temp_data.calculated_value"}
```

## Audit Trail

Every modification to message data is recorded:

```rust
pub struct AuditTrail {
    pub workflow_id: Arc<str>,
    pub task_id: Arc<str>,
    pub timestamp: DateTime<Utc>,
    pub changes: Vec<Change>,
    pub status: usize,
}

pub struct Change {
    pub path: Arc<str>,
    pub old_value: Arc<OwnedDataValue>,
    pub new_value: Arc<OwnedDataValue>,
}
```

Set `message.capture_changes = false` (or call `.without_change_capture()`
at construction) to skip per-write `Change` capture — audit-trail entries
are still recorded, but their `changes: []` is empty. The wire shape is
unchanged either way.

### Accessing Audit Trail

```rust
// After processing
for entry in &message.audit_trail {
    println!("Workflow: {}, Task: {}", entry.workflow_id, entry.task_id);
    for change in &entry.changes {
        println!("  {} -> {} at {}", change.old_value, change.new_value, change.path);
    }
}
```

## Error Handling

Errors are collected in `message.errors`:

```rust
for error in &message.errors {
    println!("Error in {}/{}: {}",
        error.workflow_id.as_deref().unwrap_or("unknown"),
        error.task_id.as_deref().unwrap_or("unknown"),
        error.message
    );
}
```

## JSONLogic Access

In rule conditions and mappings, access message fields using JSONLogic:

```json
// Access data fields
{"var": "data.name"}
{"var": "data.user.email"}

// Access metadata
{"var": "metadata.type"}
{"var": "metadata.source"}

// Access temp_data
{"var": "temp_data.intermediate_result"}
```

## Try It

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"message_demo","name":"Message Demo","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"set_temp","name":"Set Temp Data","function":{"name":"map","input":{"mappings":[{"path":"temp_data.full_name","logic":{"cat":[{"var":"data.input.first_name"}," ",{"var":"data.input.last_name"}]}}]}}},{"id":"use_temp","name":"Use Temp Data","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"temp_data.full_name"},"!"]}}]}}}]}]' data-payload='{"first_name":"John","last_name":"Doe"}'>
</div>

Notice how `temp_data` is used to store an intermediate result.

## Best Practices

1. **Separate Concerns**
   - Use `data` for business data
   - Use `metadata` for routing and rule conditions
   - Use `temp_data` for intermediate results

2. **Don't Modify metadata in Tasks**
   - Metadata should remain stable for routing decisions

3. **Clean temp_data**
   - Use `temp_data` for values only needed during processing

4. **Check Audit Trail**
   - Use the audit trail for debugging and compliance
