# Message

A Message is the data container that flows through workflows, carrying data, metadata, and an audit trail.

## Overview

The Message structure contains:

- **context.data** - Main data payload
- **context.metadata** - Message metadata (routing, source info)
- **context.temp_data** - Temporary processing data
- **audit_trail** - Record of all changes
- **errors** - Collected errors during processing

## Message Structure

```rust
pub struct Message {
    pub id: Uuid,
    pub payload: Arc<Value>,
    pub context: Value,       // Contains data, metadata, temp_data
    pub audit_trail: Vec<AuditTrail>,
    pub errors: Vec<ErrorInfo>,
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

let mut message = Message::new(&json!({
    "name": "John",
    "email": "john@example.com"
}));
```

### With Metadata

```rust
let mut message = Message::new(&json!({
    "name": "John"
}));

message.context["metadata"] = json!({
    "source": "api",
    "type": "user",
    "timestamp": "2024-01-01T00:00:00Z"
});
```

### From Full Context

```rust
let mut message = Message::from_value(&json!({
    "data": {
        "name": "John"
    },
    "metadata": {
        "source": "api"
    },
    "temp_data": {}
}));
```

## Context Fields

### data

The main data payload. This is where your primary data lives and is transformed.

```rust
// Set data
message.context["data"]["name"] = json!("John");

// Read data
let name = &message.context["data"]["name"];
```

### metadata

Information about the message itself (not the data). Commonly used for:

- Routing decisions (workflow conditions)
- Source tracking
- Timestamps
- Message type classification

```rust
message.context["metadata"] = json!({
    "type": "user",
    "source": "webhook",
    "received_at": "2024-01-01T00:00:00Z"
});
```

### temp_data

Temporary storage for intermediate processing results. Cleared between processing runs if needed.

```rust
// Store intermediate result
message.context["temp_data"]["calculated_value"] = json!(42);

// Use in later task
// {"var": "temp_data.calculated_value"}
```

## Audit Trail

Every modification to message data is recorded:

```rust
#[derive(Debug)]
pub struct AuditTrail {
    pub task_id: String,
    pub workflow_id: String,
    pub timestamp: DateTime<Utc>,
    pub changes: Vec<Change>,
}

pub struct Change {
    pub path: Arc<str>,
    pub old_value: Arc<Value>,
    pub new_value: Arc<Value>,
}
```

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

In workflow conditions and mappings, access message fields using JSONLogic:

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

> **Want more features?** Try the [Full Debugger UI](/debugger/) with step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"message_demo","name":"Message Demo","tasks":[{"id":"set_temp","name":"Set Temp Data","function":{"name":"map","input":{"mappings":[{"path":"temp_data.full_name","logic":{"cat":[{"var":"data.first_name"}," ",{"var":"data.last_name"}]}}]}}},{"id":"use_temp","name":"Use Temp Data","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"temp_data.full_name"},"!"]}}]}}}]}]' data-message='{"data":{"first_name":"John","last_name":"Doe"},"metadata":{"source":"playground"}}'>
</div>

Notice how `temp_data` is used to store an intermediate result.

## Best Practices

1. **Separate Concerns**
   - Use `data` for business data
   - Use `metadata` for routing and tracking
   - Use `temp_data` for intermediate results

2. **Don't Modify metadata in Tasks**
   - Metadata should remain stable for routing decisions

3. **Clean temp_data**
   - Use `temp_data` for values only needed during processing

4. **Check Audit Trail**
   - Use the audit trail for debugging and compliance
