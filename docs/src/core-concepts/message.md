# Message

A Message is the data container that flows through workflows, carrying a payload, the context, an audit trail and the errors recorded along the way.

## Overview

A message holds:

- **id** - A UUID v7, unless you set one
- **payload** - The body as the engine received it; not part of the evaluation context
- **context.data** - Main data payload
- **context.metadata** - Message metadata (routing, source info, the engine's stamps)
- **context.temp_data** - Temporary processing data
- **audit_trail** - One entry per task that ran, with the writes it made
- **errors** - Every error recorded during processing

## Message Structure

The fields are private, behind accessors, and the message is pure wire data:
it serializes as `{id, payload, context, audit_trail, errors}`, in that
order, and reads back from what it writes.

```rust
use dataflow_rs::Message;
use dataflow_rs::datavalue::OwnedDataValue;
use serde_json::json;

let message = Message::from_json(&json!({"name": "John"})).with_id("order-7");

assert_eq!(message.id(), "order-7");
assert_eq!(message.payload(), &OwnedDataValue::from(&json!({"name": "John"})));
assert!(message.audit().is_empty());
assert!(message.errors().is_empty());

// The context is always `data`, `metadata` and `temp_data`, in that order.
let wire = serde_json::to_string(&message).unwrap();
assert!(wire.starts_with(r#"{"id":"order-7","payload":{"name":"John"},"context":{"data":{},"metadata":{},"temp_data":{}}"#));

// A message reads back byte for byte.
let read: Message = serde_json::from_str(&wire).unwrap();
assert_eq!(read, message);
```

The accessors are `id()`, `payload()`, `context()` and `context_mut()`,
`audit()`, `errors()`, and `to_json()` for the message as a
`serde_json::Value`.

The context is structured as:

```jsonc
{
    "data": { ... },
    "metadata": { ... },
    "temp_data": { ... }
}
```

Those three roots are the whole context. Reading a message whose context
holds a fourth key, or whose context is not an object, is refused, since
every expression evaluates against this object and a fourth key would be
readable by `{"var": …}`.

## Creating Messages

### From JSON

```rust
use dataflow_rs::Message;
use serde_json::json;

// `from_json` converts the payload. The context starts as three empty
// roots; a `parse_json` task is what lands the payload in `data`.
let message = Message::from_json(&json!({
    "name": "John",
    "email": "john@example.com"
}));
# assert_eq!(serde_json::to_string(message.context()).unwrap(),
#     r#"{"data":{},"metadata":{},"temp_data":{}}"#);
```

### From an Owned Value

`Message::new` takes the payload as an `OwnedDataValue`, or as an
`Arc<OwnedDataValue>`, which is shared rather than copied: messages built
from one `Arc` hold one payload.

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

### Seeding the Context

`from_parts` seeds `data` and `metadata` directly, for a host that has
already shaped them. A seed that is not an object is ignored.

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

### Writing the Context from Rust

Workflows populate the context through the `parse_json`, `map` and other
built-ins, and a host function writes through its `Ctx`, which records the
audit-trail `Change` and lands the write when the call returns `Ok`:

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

Outside a run, `context_mut()` writes by the same path rules, with no audit
entry:

```rust
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::{Message, Path};
use serde_json::json;

# fn main() -> dataflow_rs::Result<()> {
let mut message = Message::from_json(&json!({}));
let path: Path = "data.order.total".parse()?;
message.context_mut().set(&path, OwnedDataValue::from(1200))?;
assert_eq!(message.context().get(&path), Some(&OwnedDataValue::from(1200)));

// A root takes only an object.
let root: Path = "data".parse()?;
assert!(message.context_mut().set(&root, OwnedDataValue::from(1)).is_err());
# Ok(()) }
```

## Context Fields

### data

The main data payload, where your primary data lives and gets transformed.
Workflows populate it via `parse_json` / `map` tasks; a host function reads
it through `ctx.get(&path)` or by evaluating an expression with `ctx.eval`.

```rust
# use dataflow_rs::Message;
# fn _demo(message: &Message) -> dataflow_rs::Result<()> {
// Inspecting a processed message, in a test or a handler of the result.
let total = message.context().get(&"data.order.total".parse()?);
println!("total: {total:?}");
# Ok(()) }
```

### metadata

Information about the message itself (not the data). Commonly used for:

- Routing decisions (workflow conditions)
- Source tracking
- Timestamps
- Message type classification

Several keys under `metadata` belong to the engine; treat them as reserved:

- **`metadata.processed_at`** and **`metadata.engine_version`**: stamped at
  the start of every run, before any workflow. The run reads the clock once;
  `processed_at`, every audit entry and every error record carry that reading
- **`metadata.channel`**: the channel name, stamped only by a run with
  `RunOptions::channel`
- **`metadata.progress`**: rewritten after **every** task that runs, as
  `{"workflow_id": …, "task_id": …, "status_code": …}`. Cross-workflow
  chaining depends on it: a later workflow gates on
  `{"var": "metadata.progress.task_id"}` or on `status_code` to decide whether
  to run. Don't write this path yourself; the next task overwrites it
- the path an [`ErrorContext`](./error-handling.md#branching-on-why-a-task-failed)
  names, when one is configured: removed at run start and appended to after
  each failing task

### temp_data

Temporary storage for intermediate processing results, useful for values
threaded between tasks within the same workflow. A loop's counter and a
fan-out's element bindings live here too.

```json
{"path": "temp_data.calculated_value", "logic": {"*": [{"var": "data.qty"}, 2]}}
```

A later task reads it back with `{"var": "temp_data.calculated_value"}`.

`temp_data` ships on the wire like everything else in the context, so it is
not private scratch space.

## Audit Trail

The engine records one `AuditEntry` per task that ran:

| Field | Description |
|---|---|
| `workflow_id`, `task_id` | Where the entry came from |
| `timestamp` | The run's one clock reading |
| `status` | The task's status: `200` for `Ok`, the status it returned, `500` for an `Err`, `299` for a halt |
| `changes` | The writes the task made, when changes are captured |
| `loop_counter` | The sweep that produced the entry; absent outside a loop |
| `element_index` | The fan-out element; absent outside a fan-out |

Each `Change` carries the dotted `path` written, the `old_value` (`null` when
the path was absent), the `new_value` (`null` for a removal) and `removed`,
which tells a removal from a write of `null`.

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

A task whose function returned `Outcome::Skip` leaves no entry; a task whose
condition was false leaves none either.

### Bounding the trail

Holding every write's old and new value for the whole run costs most in a
[looping workflow](../advanced/loops.md). Two run options bound it:

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message) {
use dataflow_rs::run::{AuditMode, RunOptions};

let opts = RunOptions::default()
    // Keep the entries, with empty `changes` lists.
    .capture_changes(false)
    // Keep only the most recent 10 entries; `Off` keeps none.
    .audit(AuditMode::Last(10));
engine.run_with(&mut message, &opts).await;
# }
```

The writes happen either way; only what is kept changes. `metadata.progress`
does not depend on the audit trail.

## Errors

Every error recorded during processing is on `message.errors()`, as an
`ErrorRecord`: its `code`, its `message`, the `workflow_id` and `task_id` it
came from, its `timestamp`, and `element_index` for a fan-out element.

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

This is the one error list. The `Run` the engine returns says whether the run
stopped early and why; see [Error Handling](./error-handling.md).

## JSONLogic Access

In workflow conditions and mappings, access message fields using JSONLogic:

```jsonc
// Access data fields
{"var": "data.name"}
{"var": "data.user.email"}

// Access metadata
{"var": "metadata.type"}
{"var": "metadata.source"}

// Access temp_data
{"var": "temp_data.intermediate_result"}
```

`payload` is **not** part of that tree. It is a separate field on `Message`, so
`{"var": "payload.foo"}` resolves to nothing. Because expressions run in
templating mode, it fails *silently* rather than erroring: the condition is
never true. Run a `parse_json` (or `parse_xml`) task first to land the
payload under `data`, then read it as `{"var": "data.…"}`. `Compiler::check`
reports a literal `var` rooted at `payload` as a `PAYLOAD_VAR` defect, and a
host function reads the payload through `Ctx::payload`.

Secrets are not in the tree either, on purpose: everything in it is recorded.
You read a signing key with `{"secret": "name"}` from a store the engine holds
outside the message; see [Secrets](../advanced/secrets.md).

## Try It

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"message_demo","name":"Message Demo","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"set_temp","name":"Set Temp Data","function":{"name":"map","input":{"mappings":[{"path":"temp_data.full_name","logic":{"cat":[{"var":"data.input.first_name"}," ",{"var":"data.input.last_name"}]}}]}}},{"id":"use_temp","name":"Use Temp Data","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"temp_data.full_name"},"!"]}}]}}}]}]' data-payload='{"first_name":"John","last_name":"Doe"}'>
</div>

The `set_temp` task stores an intermediate result in `temp_data`, and
`use_temp` reads it back.

## Best Practices

1. **Separate Concerns**
   - Use `data` for business data
   - Use `metadata` for routing and workflow conditions
   - Use `temp_data` for intermediate results

2. **Leave the Engine's Keys Alone**
   - `metadata.progress`, `processed_at`, `engine_version` and `channel` are the engine's

3. **Keep Roots as Objects**
   - A write at `data`, `metadata` or `temp_data` takes only an object

4. **Check the Audit Trail**
   - Use the audit trail for debugging and compliance

## From 3.x

In 3.x a message was built with `Message::from_value`, `Message::new` or
`Message::builder()`, whose `capture_changes`, `audit_mode` and
`routing_bucket` are now `RunOptions`; `context` was a public field, now
behind `context()` and `context_mut()`; and a handler wrote through
`TaskContext::set`, now `Ctx::set`. See
[Migrating from 3.x](../migration/from-3.md).
