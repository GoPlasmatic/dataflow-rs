# Message

A `Message` is the data container that flows through the engine, carrying the input payload, the mutable evaluation context, an audit trail of changes, and any recorded errors.

## Overview

Every `Message` contains:

- **`id`** — Unique message identifier (defaults to a generated UUID v7 unless set explicitly).
- **`payload`** — The raw input value passed into the engine (read-only and separate from the JSONLogic evaluation context).
- **`context.data`** — Primary business data read and transformed by workflows.
- **`context.metadata`** — Message and execution metadata (routing headers, source info, and engine stamps).
- **`context.temp_data`** — Temporary scratch data shared between tasks during processing.
- **`audit_trail`** — Ordered list of `AuditEntry` records, one per executed task, detailing the changes made.
- **`errors`** — Ordered list of `ErrorRecord` entries recorded during execution.

## Message Structure and Serialization

`Message` fields are accessed via methods and serialize cleanly to and from JSON as `{id, payload, context, audit_trail, errors}` in that exact key order:

```rust
use dataflow_rs::Message;
use dataflow_rs::datavalue::OwnedDataValue;
use serde_json::json;

let message = Message::from_json(&json!({"name": "John"})).with_id("order-7");

assert_eq!(message.id(), "order-7");
assert_eq!(message.payload(), &OwnedDataValue::from(&json!({"name": "John"})));
assert!(message.audit().is_empty());
assert!(message.errors().is_empty());

// The context always serializes as `data`, `metadata`, and `temp_data` in order.
let wire = serde_json::to_string(&message).unwrap();
assert!(wire.starts_with(r#"{"id":"order-7","payload":{"name":"John"},"context":{"data":{},"metadata":{},"temp_data":{}}"#));

// Messages round-trip through JSON byte-for-byte.
let read: Message = serde_json::from_str(&wire).unwrap();
assert_eq!(read, message);
```

Main accessor methods on `Message`:
- `id()`
- `payload()`
- `context()` and `context_mut()`
- `audit()`
- `errors()`
- `to_json()` (converts the message to a `serde_json::Value`)

The evaluation `Context` always consists of three object roots:

```jsonc
{
    "data": { ... },
    "metadata": { ... },
    "temp_data": { ... }
}
```

Deserializing a `Message` whose `context` is not an object or contains any top-level key other than `data`, `metadata`, and `temp_data` returns an error.

## Creating Messages

### From JSON (`Message::from_json`)

```rust
use dataflow_rs::Message;
use serde_json::json;

// `from_json` sets the message `payload`. The three context roots (`data`,
// `metadata`, `temp_data`) start empty; use a `parse_json` task to copy the
// payload into `data`.
let message = Message::from_json(&json!({
    "name": "John",
    "email": "john@example.com"
}));
# assert_eq!(serde_json::to_string(message.context()).unwrap(),
#     r#"{"data":{},"metadata":{},"temp_data":{}}"#);
```

### From an `OwnedDataValue` (`Message::new`)

`Message::new` accepts an `OwnedDataValue` or an `Arc<OwnedDataValue>`. Passing an `Arc<OwnedDataValue>` shares the payload across multiple messages without cloning:

```rust
use dataflow_rs::Message;
use dataflow_rs::datavalue::OwnedDataValue;
use serde_json::json;
use std::sync::Arc;

let payload = Arc::new(OwnedDataValue::from(&json!({"name": "John"})));
let first = Message::new(Arc::clone(&payload));
let second = Message::new(Arc::clone(&payload));
assert!(std::ptr::eq(first.payload(), second.payload()));
```

### Pre-Populating `data` and `metadata` (`Message::from_parts`)

If your application has already parsed the input and wants to populate `data` and `metadata` directly before running workflows, use `Message::from_parts` (any non-object argument for `data` or `metadata` is ignored and replaced with `{}`):

```rust
use dataflow_rs::Message;
use dataflow_rs::datavalue::OwnedDataValue;
use serde_json::json;

let message = Message::from_parts(
    OwnedDataValue::Null,
    OwnedDataValue::from(&json!({"order": {"total": 1200}})),
    OwnedDataValue::from(&json!({"source": "api"})),
)
.with_id("correlation-123");

assert_eq!(message.context().data(), &OwnedDataValue::from(&json!({"order": {"total": 1200}})));
assert_eq!(message.context().metadata(), &OwnedDataValue::from(&json!({"source": "api"})));
```

### Modifying the Context from Rust

During a workflow run, built-in tasks like `parse_json` and `map` write to the context automatically, while custom host functions write through `Ctx::set` / `Ctx::emit`, which record audit-trail changes and commit writes when the call returns `Ok`:

```rust
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::{Ctx, Function, Outcome, Result};

struct Stamp;

impl Function for Stamp {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        ctx.set(&"metadata.source".parse()?, OwnedDataValue::from("api"))?;
        ctx.set(&"metadata.type".parse()?, OwnedDataValue::from("user"))?;
        Ok(Outcome::Ok)
    }
}
```

Outside of a run, you can read or modify the context directly via `message.context()` and `message.context_mut()` (direct mutations outside a run do not create audit entries):

```rust
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::{Message, Path};
use serde_json::json;

# fn main() -> dataflow_rs::Result<()> {
let mut message = Message::from_json(&json!({}));
let path: Path = "data.order.total".parse()?;
message.context_mut().set(&path, OwnedDataValue::from(1200))?;
assert_eq!(message.context().get(&path), Some(&OwnedDataValue::from(1200)));

// Top-level roots (`data`, `metadata`, `temp_data`) must remain JSON objects.
let root: Path = "data".parse()?;
assert!(message.context_mut().set(&root, OwnedDataValue::from(1)).is_err());
# Ok(()) }
```

## Context Roots

### `data`

The main business data container where your domain fields live and get transformed. Workflows typically populate `data` using `parse_json` and `map`, while custom host functions read it via `ctx.get(&path)` or `ctx.eval(&expr)`:

```rust
# use dataflow_rs::Message;
# fn _demo(message: &Message) -> dataflow_rs::Result<()> {
// Read a field from `data` after a run completes.
let total = message.context().get(&"data.order.total".parse()?);
println!("total: {total:?}");
# Ok(()) }
```

### `metadata`

Holds metadata about the message and the current run (such as headers, source identifiers, and routing attributes).

Several paths under `metadata` are managed automatically by the engine—treat them as reserved:

- **`metadata.processed_at`** & **`metadata.engine_version`** — Stamped at the start of every run before any workflow executes. The engine reads the clock once at run start and uses that timestamp for `processed_at`, audit entries, and error records.
- **`metadata.channel`** — Stamped at run start when `RunOptions::channel` is set.
- **`metadata.progress`** — Updated after **every** executed task to `{"workflow_id": ..., "task_id": ..., "status_code": ...}`. Subsequent workflows and tasks can inspect `{"var": "metadata.progress.task_id"}` or `{"var": "metadata.progress.status_code"}` to branch on earlier steps. Avoid writing to `metadata.progress` yourself, as the engine overwrites it after each task.
- **Configured `ErrorContext` path** — If [`ErrorContext`](./error-handling.md#branching-on-task-failures) is enabled on `EngineConfig`, the engine clears that path at run start and appends `{workflow_id, task_id, code, status}` whenever a task records an error.

### `temp_data`

Scratch space for intermediate values passed between tasks within a workflow. Loop counters (`loop`) and fan-out element bindings (`for_each`) also live under `temp_data`.

```json
{"path": "temp_data.calculated_value", "logic": {"*": [{"var": "data.qty"}, 2]}}
```

Subsequent tasks can read that value with `{"var": "temp_data.calculated_value"}`.

> **Note:** `temp_data` is serialized as part of `Message` just like `data` and `metadata`. Do not store sensitive secrets in `temp_data`—use the engine's [Secrets](../advanced/secrets.md) store instead.

## Audit Trail

The engine records one `AuditEntry` in `message.audit()` for each executed task:

| Field | Description |
|---|---|
| `workflow_id`, `task_id` | The workflow and task that produced the entry |
| `timestamp` | RFC 3339 timestamp of the run |
| `status` | HTTP-style status code (`200` for `Ok`, custom status code, `500` for `Err`, `299` for `Halt`) |
| `changes` | List of context writes (`Change`) committed by the task (when `capture_changes` is enabled) |
| `loop_counter` | Loop sweep index (present only inside a looping workflow) |
| `element_index` | Fan-out element index (present only inside a `for_each` task) |

Each `Change` contains the dotted `path`, `old_value` (`null` if the field did not exist), `new_value` (`null` if removed), and `removed` (`true` when the field was deleted rather than set to `null`).

```rust
# fn _demo(message: &dataflow_rs::Message) {
for entry in message.audit() {
    println!("{}/{}: status {}", entry.workflow_id, entry.task_id, entry.status);
    for change in &entry.changes {
        if change.removed {
            println!("  removed {}", change.path);
        } else {
            println!("  {} = {:?}", change.path, change.new_value);
        }
    }
}
# }
```

Tasks skipped because their condition evaluated to `false` or because their function returned `Outcome::Skip` do not add an audit entry.

### Controlling Audit Trail Size

Capturing `old_value` and `new_value` snapshots for every write has a memory cost, especially in [looping workflows](../advanced/loops.md). You can bound or disable audit capture per run via `RunOptions`:

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message) {
use dataflow_rs::run::{AuditMode, RunOptions};

let opts = RunOptions::default()
    // Keep audit entries (workflow_id, task_id, status), but omit `changes` diffs.
    .capture_changes(false)
    // Retain only the 10 most recent audit entries (`AuditMode::Off` retains none).
    .audit(AuditMode::Last(10));
engine.run_with(&mut message, &opts).await;
# }
```

Disabling change capture or audit entries does not affect context updates or `metadata.progress`.

## Errors

Every error recorded during a run is appended to `message.errors()` as an `ErrorRecord` containing `code`, `message`, `workflow_id`, `task_id`, `timestamp`, `path` (for mapping errors), and `element_index` (for `for_each` errors):

```rust
# fn _demo(message: &dataflow_rs::Message) {
for error in message.errors() {
    println!(
        "{} in {}/{}: {}",
        error.code,
        error.workflow_id.as_deref().unwrap_or("-"),
        error.task_id.as_deref().unwrap_or("-"),
        error.message
    );
}
# }
```

To check whether an error stopped the run early, inspect `Run::status()`. See [Error Handling](./error-handling.md) for details.

## Accessing Fields in JSONLogic

In workflow conditions, task conditions, and mappings, read context fields using `{"var": "..."}`:

```jsonc
// Read from `data`
{"var": "data.name"}
{"var": "data.user.email"}

// Read from `metadata`
{"var": "metadata.type"}
{"var": "metadata.source"}

// Read from `temp_data`
{"var": "temp_data.intermediate_result"}
```

- **`payload` is not in the JSONLogic context:** `{"var": "payload.foo"}` always evaluates to `null` (and `Compiler::check` flags it as a `PAYLOAD_VAR` defect). Copy the payload into `data` first using `parse_json` or `parse_xml` (custom host functions can also read the raw payload via `Ctx::payload`).
- **Secrets are read via `{"secret": "name"}`:** Secrets live in an isolated store on `ExprEngine` so they are never serialized with the message. See [Secrets](../advanced/secrets.md).

## Try It

> Open the [Full Debugger UI](/dataflow-rs/debugger/) for step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"message_demo","name":"Message Demo","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"set_temp","name":"Set Temp Data","function":{"name":"map","input":{"mappings":[{"path":"temp_data.full_name","logic":{"cat":[{"var":"data.input.first_name"}," ",{"var":"data.input.last_name"}]}}]}}},{"id":"use_temp","name":"Use Temp Data","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"temp_data.full_name"},"!"]}}]}}}]}]' data-payload='{"first_name":"John","last_name":"Doe"}'>
</div>

Here, `set_temp` writes an intermediate string to `temp_data.full_name`, and `use_temp` reads it to build `data.greeting`.

## Best Practices

1. **Separate Concerns Across Roots**
   - Use `data` for domain and business fields.
   - Use `metadata` for routing headers, environment info, and workflow conditions.
   - Use `temp_data` for temporary intermediate calculations.
2. **Do Not Overwrite Reserved `metadata` Keys**
   - Leave `metadata.progress`, `metadata.processed_at`, `metadata.engine_version`, and `metadata.channel` to the engine.
3. **Keep Context Roots as Objects**
   - Writes directly to `data`, `metadata`, or `temp_data` must always be JSON objects.

## Migrating from 3.x

In 3.x, messages were constructed with `Message::from_value`, `Message::new`, or `Message::builder()` (where `capture_changes`, `audit_mode`, and `routing_bucket` lived on the message), `message.context` was a public field, and custom handlers wrote via `TaskContext::set`.

In 4.0:
- Use `Message::from_json`, `Message::new`, or `Message::from_parts`.
- Configure `capture_changes`, `audit`, and `routing_bucket` per run on `RunOptions`.
- Access the context via `message.context()` and `message.context_mut()`, and write inside host functions via `Ctx::set` or `Ctx::emit`.

See [Migrating from 3.x](../migration/from-3.md) for the complete guide.
