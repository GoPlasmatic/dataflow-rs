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
    // Read via accessors: id(), payload(), payload_arc(), audit_trail(),
    // errors(), capture_changes(). Mutate `errors` via add_error(...).
    // `context` is the only public field — it's the legitimate read
    // surface for tests (e.g. `message.context["data"]["x"]`); inside a
    // handler, prefer `TaskContext::set` so audit-trail changes are
    // recorded automatically.
    pub context: OwnedDataValue,          // Always an Object {data, metadata, temp_data}
    // ... encapsulated fields ...
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
// have JSON literals. The payload lands on the message; the context
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

### Builder

For the richer cases — caller-supplied id (correlation), capture-off
fast path — use `Message::builder()`:

```rust
let mut message = Message::builder()
    .id("correlation-123")
    .payload_json(&json!({"name": "John"}))
    .capture_changes(false) // skip per-write Change capture
    .build();
```

### Populating the Context

In practice you don't mutate `message.context` directly from Rust — the
`parse_json` / `map` / `validation` built-ins are how your workflows
populate it. Inside a custom `AsyncFunctionHandler`, use
[`TaskContext::set`](../advanced/custom-functions.md) which records
audit-trail changes automatically:

```rust,ignore
ctx.set("metadata.source", OwnedDataValue::from(&json!("api")));
ctx.set("metadata.type",   OwnedDataValue::from(&json!("user")));
```

## Context Fields

### data

The main data payload. This is where your primary data lives and is transformed.
Workflows populate it via `parse_json` / `map` tasks; handlers read it
through `ctx.data()`. The example below shows the read accessors:

```rust,ignore
// Inside an AsyncFunctionHandler — TaskContext::data() returns
// &OwnedDataValue (Null if missing, matching serde_json::Value index
// semantics).
let name = ctx.data().get("name");

// Outside a handler (e.g. inspecting a processed message in tests):
let name = &message.data()["name"];
```

### metadata

Information about the message itself (not the data). Commonly used for:

- Routing decisions (rule conditions)
- Source tracking
- Timestamps
- Message type classification

From a handler, `ctx.set("metadata.X", v)` is the canonical write
path. The engine also stamps `metadata.processed_at` and
`metadata.engine_version` automatically on every `process_message` call.

### temp_data

Temporary storage for intermediate processing results — useful for values
threaded between tasks within the same workflow. From a handler:

```rust,ignore
ctx.set("temp_data.calculated_value", OwnedDataValue::from(&json!(42)));

// Later tasks read it via JSONLogic:
//   {"var": "temp_data.calculated_value"}
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
    pub old_value: OwnedDataValue,  // owned (not Arc) — one fewer heap alloc per Change
    pub new_value: OwnedDataValue,
}
```

To skip per-write `Change` capture (bulk-pipeline fast path), build the
message with `capture_changes(false)`:

```rust
let m = Message::builder()
    .payload_json(&json!({}))
    .capture_changes(false)
    .build();
```

Audit-trail entries are still recorded — just with empty `changes` lists.
The wire shape is unchanged either way.

### Accessing Audit Trail

```rust
// After processing — audit_trail() returns &[AuditTrail].
for entry in message.audit_trail() {
    println!("Workflow: {}, Task: {}", entry.workflow_id, entry.task_id);
    for change in &entry.changes {
        println!("  {} -> {} at {}", change.old_value, change.new_value, change.path);
    }
}
```

## Error Handling

Errors are collected in `message.errors()` (the always-on channel, even
when `Engine::process_message` returns `Result::Err`):

```rust
for error in message.errors() {
    println!("Error in {}/{}: {}",
        error.workflow_id.as_deref().unwrap_or("unknown"),
        error.task_id.as_deref().unwrap_or("unknown"),
        error.message
    );
}
```

See [Error Handling](./error-handling.md) for the unified-channel
contract in detail.

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
