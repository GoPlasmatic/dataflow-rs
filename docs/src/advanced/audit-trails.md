# Audit Trails

Dataflow-rs automatically tracks all data modifications for debugging, monitoring, and compliance.

## Overview

Every change to message data is recorded in the audit trail:

- **What changed** - Path and values (old and new)
- **When it changed** - Timestamp
- **Which task** - Workflow and task identifiers

## Audit Trail Structure

```rust
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

## Accessing the Audit Trail

After processing, the audit trail is available on the message:

```rust
engine.process_message(&mut message).await?;

for entry in &message.audit_trail {
    println!("Workflow: {}, Task: {}", entry.workflow_id, entry.task_id);
    println!("Timestamp: {}", entry.timestamp);

    for change in &entry.changes {
        println!("  Path: {}", change.path);
        println!("  Old: {}", change.old_value);
        println!("  New: {}", change.new_value);
    }
}
```

## JSON Representation

In the playground output, the audit trail appears as:

```json
{
    "audit_trail": [
        {
            "task_id": "transform_data",
            "workflow_id": "my_workflow",
            "timestamp": "2024-01-01T12:00:00Z",
            "changes": [
                {
                    "path": "data.full_name",
                    "old_value": null,
                    "new_value": "John Doe"
                },
                {
                    "path": "data.greeting",
                    "old_value": null,
                    "new_value": "Hello, John Doe!"
                }
            ]
        }
    ]
}
```

## What Gets Tracked

### Map Function

Every mapping that modifies data creates a change entry:

```json
{
    "mappings": [
        {"path": "data.name", "logic": "John"}
    ]
}
```

Creates:
```json
{
    "path": "data.name",
    "old_value": null,
    "new_value": "John"
}
```

### Custom Functions

Custom functions should track changes for proper auditing:

```rust
let changes = vec![Change {
    path: Arc::from("data.processed"),
    old_value: Arc::new(old_value),
    new_value: Arc::new(new_value),
}];

Ok((200, changes))
```

### Validation Function

Validation is read-only, so it produces no audit trail entries.

## Try It

> **Want more features?** Try the [Full Debugger UI](/debugger/) with step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"audit_demo","name":"Audit Demo","tasks":[{"id":"step1","name":"Step 1","function":{"name":"map","input":{"mappings":[{"path":"data.full_name","logic":{"cat":[{"var":"data.first_name"}," ",{"var":"data.last_name"}]}}]}}},{"id":"step2","name":"Step 2","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"data.full_name"},"!"]}}]}}},{"id":"step3","name":"Step 3","function":{"name":"map","input":{"mappings":[{"path":"data.processed","logic":true},{"path":"temp_data.step_count","logic":3}]}}}]}]' data-message='{"data":{"first_name":"John","last_name":"Doe"},"metadata":{}}'>
</div>

Notice the audit trail shows each step's changes.

## Use Cases

### Debugging

Trace exactly how data was transformed:

```rust
// Find where a value was set
for entry in &message.audit_trail {
    for change in &entry.changes {
        if change.path.as_ref() == "data.total" {
            println!("data.total set by {}/{}",
                entry.workflow_id, entry.task_id);
            println!("Changed from {} to {}",
                change.old_value, change.new_value);
        }
    }
}
```

### Compliance

Log all changes for regulatory compliance:

```rust
for entry in &message.audit_trail {
    log_to_audit_system(
        entry.timestamp,
        entry.workflow_id.clone(),
        entry.task_id.clone(),
        &entry.changes
    );
}
```

### Change Detection

Detect if specific fields were modified:

```rust
fn was_field_modified(message: &Message, field: &str) -> bool {
    message.audit_trail.iter()
        .flat_map(|e| e.changes.iter())
        .any(|c| c.path.as_ref() == field)
}

if was_field_modified(&message, "data.price") {
    // Price was changed during processing
}
```

### Rollback (Conceptual)

The audit trail can be used to implement rollback:

```rust
fn get_original_value(message: &Message, field: &str) -> Option<&Value> {
    message.audit_trail.iter()
        .flat_map(|e| e.changes.iter())
        .find(|c| c.path.as_ref() == field)
        .map(|c| c.old_value.as_ref())
}
```

## Best Practices

1. **Track All Changes** - Custom functions should record all modifications
2. **Use Arc** - Use `Arc<str>` and `Arc<Value>` for efficient sharing
3. **Timestamp Accuracy** - Timestamps are UTC for consistency
4. **Check Audit Trail** - Review audit trail during development
5. **Log for Production** - Persist audit trails for production debugging
