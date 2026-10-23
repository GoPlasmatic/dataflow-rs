# Audit Trails and Execution Traces

Dataflow-rs provides two ways to inspect what a run did to a message:

- The **audit trail**: one `AuditEntry` per executed task, stored directly on the
  `Message`.
- An **execution trace** (optional): a record of the initial context and every
  step-by-step change, allowing you to reconstruct the exact message context at
  any point in the run.

This guide covers both mechanisms and how to configure their memory footprint.

## The Audit Trail

Every task that executes (and does not return `Outcome::Skip`) appends an
`AuditEntry` to `message.audit()`:

| Field | Description |
|---|---|
| `workflow_id`, `task_id` | Identifies the workflow and task that produced the entry. |
| `timestamp` | The run's timestamp (all entries in a single run share the same clock reading). |
| `status` | The task's status code: `200` for `Outcome::Ok`, `n` for `Status(n)`, `299` for `Halt`, or `500` for `Err`. |
| `changes` | The list of context writes made by the task (when change capture is enabled). |
| `loop_counter` | The current iteration counter when inside a workflow [`loop`](./loops.md); omitted otherwise. |
| `element_index` | The current array index when inside a task [`for_each`](./for-each.md); omitted otherwise. |

Each `Change` in `entry.changes` represents a single write or key removal:

| Field | Description |
|---|---|
| `path` | The dotted context path that was modified (e.g., `data.full_name`). |
| `old_value` | The previous value at `path`, or `null` if the key did not exist. |
| `new_value` | The new value written at `path`, or `null` if the key was removed. |
| `removed` | `true` if the key was removed rather than set to `null` (omitted in JSON when `false`). |

Because `new_value` is `null` both when a field is set to `null` and when a
field is removed (via `map`'s `"unset": true`, `on_null: "unset"`, or
`ctx.remove` in a host function), check `change.removed` to distinguish a
removal from a `null` assignment. (`Change` also has a `replaced` field used
exclusively by execution traces; on audit entries it is always `false` and
omitted in JSON.)

If a custom host function returns `Err`, the engine discards its buffered
context writes and still records an `AuditEntry` with `status: 500` and an empty
`changes` list, while appending the error details to `message.errors()`.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::{Engine, Message};
use serde_json::json;

let engine = Engine::builder()
    .workflow_json(&json!({"id": "names", "tasks": [
        {"id": "full_name", "function": {"name": "map", "input": {"mappings": [
            {"path": "data.full_name",
             "logic": {"cat": [{"var": "data.first"}, " ", {"var": "data.last"}]}},
            {"path": "data.first", "unset": true}
        ]}}}
    ]}))
    .build()
    .unwrap();

let data = OwnedDataValue::from(&json!({"first": "Ada", "last": "Lovelace"}));
let mut message = Message::from_parts(OwnedDataValue::Null, data, OwnedDataValue::Null);
engine.run(&mut message).await;

for entry in message.audit() {
    println!("{}/{} -> {}", entry.workflow_id, entry.task_id, entry.status);
    for change in &entry.changes {
        println!("  {}: {} -> {}", change.path, change.old_value, change.new_value);
    }
}

let entry = &message.audit()[0];
assert_eq!((&*entry.task_id, entry.status), ("full_name", 200));
assert_eq!(entry.changes.len(), 2);
assert_eq!(&*entry.changes[0].path, "data.full_name");
assert_eq!(entry.changes[0].new_value, OwnedDataValue::from(&json!("Ada Lovelace")));
assert!(entry.changes[1].removed);
assert_eq!(entry.changes[1].old_value, OwnedDataValue::from(&json!("Ada")));
# });
```

### JSON Format

When a `Message` is serialized to JSON, the audit trail is written under the
`audit_trail` key:

```json
{
    "audit_trail": [
        {
            "workflow_id": "names",
            "task_id": "full_name",
            "timestamp": "2026-01-01T12:00:00Z",
            "changes": [
                {"path": "data.full_name", "old_value": null, "new_value": "Ada Lovelace"},
                {"path": "data.first", "old_value": "Ada", "new_value": null, "removed": true}
            ],
            "status": 200
        }
    ]
}
```

### What Gets Recorded in `changes`

- **`map`**: Records one `Change` per mapping that modified the context (a
  write, a removal, or—for `mode: "append"` and `"extend"`—one `Change` per
  appended element at `path.<index>`).
- **Custom host functions**: Records one `Change` per `ctx.set`, `ctx.emit`, or
  `ctx.remove` call in execution order once the call returns `Ok`.
- **`validation`**: Modifies no context fields, so its `changes` list is empty.
  Its `status` is `200` when all rules pass and `400` when any rule fails (failed
  rules are recorded in `message.errors()`).
- **Engine bookkeeping writes**: Internal context updates—such as
  `metadata.progress`, loop counters/elements/scratch paths, and `for_each`
  result arrays—are not added to task audit entries, though they are captured in
  execution traces.

Only `Outcome::Skip` suppresses the `AuditEntry` altogether.

## Bounding Audit Trail Memory

Two options on `RunOptions` control how much audit data a run retains on the
message:

- **`capture_changes(bool)`** (default `true`): Controls whether each write's
  old and new values are cloned into `entry.changes`. Cloning values on every
  write is the main allocation cost in mapping-heavy pipelines and long loops.
  Setting `capture_changes(false)` leaves `entry.changes` empty while preserving
  the audit entries, status codes, and error records.
- **`audit(AuditMode)`** (default `AuditMode::Full`): Controls how many audit
  entries are kept:
  - `AuditMode::Full`: Keeps all audit entries.
  - `AuditMode::Last(n)`: Keeps only the most recent `n` entries in a ring
    buffer.
  - `AuditMode::Off`: Keeps no audit entries (and automatically disables change
    capture, just like `Last(0)`).

Control flow, error records, and `metadata.progress` behave identically across
all audit modes.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::run::AuditMode;
use dataflow_rs::{Engine, Message, RunOptions};
use serde_json::json;

let engine = Engine::builder()
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "a", "function": {"name": "map", "input": {"mappings": [{"path": "data.a", "logic": 1}]}}},
        {"id": "b", "function": {"name": "map", "input": {"mappings": [{"path": "data.b", "logic": 2}]}}},
        {"id": "c", "function": {"name": "map", "input": {"mappings": [{"path": "data.c", "logic": 3}]}}}
    ]}))
    .build()
    .unwrap();

// Keep all audit entries, but omit the old/new value snapshots:
let mut message = Message::from_json(&json!({}));
engine.run_with(&mut message, &RunOptions::default().capture_changes(false)).await;
assert_eq!(message.audit().len(), 3);
assert!(message.audit().iter().all(|e| e.changes.is_empty()));

// Keep only the last 2 audit entries:
let mut message = Message::from_json(&json!({}));
engine.run_with(&mut message, &RunOptions::default().audit(AuditMode::Last(2))).await;
let kept: Vec<&str> = message.audit().iter().map(|e| &*e.task_id).collect();
assert_eq!(kept, ["b", "c"]);

// Disable the audit trail completely; context writes still take effect:
let mut message = Message::from_json(&json!({}));
engine.run_with(&mut message, &RunOptions::default().audit(AuditMode::Off)).await;
assert!(message.audit().is_empty());
assert_eq!(message.to_json()["context"]["data"], json!({"a": 1, "b": 2, "c": 3}));
# });
```

## Execution Traces

To record a full execution trace for a run, pass
`RunOptions::trace(TraceOptions)` to `engine.run_with` and retrieve the trace
from the returned `Run` via `run.trace()` (or `run.take_trace()`). A `Trace`
contains:

- **`initial`**: The message context after initial run metadata stamps
  (`metadata.processed_at`, `metadata.engine_version`, and `metadata.channel`)
  have been written, before any workflow executes.
- **`steps`**: A sequence of `TraceStep` records covering every executed task,
  skipped task, skipped or failed workflow, and loop iteration setup write. Each
  step records its `result` (`executed`, `skipped`, or `failed`), `status`,
  ordered `changes` (including engine bookkeeping writes such as
  `metadata.progress` and error-context writes), added `errors`, `loop_counter`,
  `element_index`, and optional execution duration (when
  `TraceOptions::timings(true)` is enabled).
- **`truncated`**: Indicates whether the trace exceeded
  `TraceOptions::max_bytes`, after which subsequent steps record their metadata
  without change payloads.

Rather than snapshotting the entire context after every step, `Trace` stores
only the incremental `changes`. Calling `trace.contexts()` replays those changes
from `initial` to yield the exact context after each step—and for `map` tasks,
`trace.mapping_contexts(step)` replays changes mapping by mapping. For
interactive step-by-step navigation over large traces, `observe::Replay`
maintains a single working context and seeks on demand (`context_at`,
`mapping_context_at`). If your application accepts serialized traces from
external callers, call `trace.validate()` first to verify its paths, root
writes, and mapping boundaries.

Note three differences between trace changes and audit trail changes:

- **Independent of `capture_changes`**: Enabling tracing captures step changes
  in the `Trace` even when `RunOptions::capture_changes(false)` is set for the
  message's audit trail.
- **Root replacement tracking**: A trace `Change` at a context root sets
  `replaced: true` when a host function replaces the root object as a whole,
  distinguishing it from a `map` root mapping that merges keys into the root.
- **Path redaction**: `TraceOptions::redact` masks values at specified paths in
  both `initial` and step `changes` as they are recorded, while leaving the real
  values intact on the `Message`.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::observe::{StepResult, TraceOptions};
use dataflow_rs::{Engine, Message, RunOptions};
use serde_json::json;

let engine = Engine::builder()
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "first", "function": {"name": "map", "input": {"mappings": [{"path": "data.a", "logic": 1}]}}},
        {"id": "skipped", "condition": false,
         "function": {"name": "map", "input": {"mappings": [{"path": "data.never", "logic": 1}]}}},
        {"id": "second", "function": {"name": "map", "input": {"mappings": [{"path": "data.b", "logic": 2}]}}}
    ]}))
    .build()
    .unwrap();

let mut message = Message::from_json(&json!({}));
let run = engine
    .run_with(&mut message, &RunOptions::default().trace(TraceOptions::default()))
    .await;
let trace = run.trace().unwrap();

let steps: Vec<(&str, StepResult)> = trace
    .steps
    .iter()
    .map(|s| (s.task_id.as_deref().unwrap(), s.result))
    .collect();
assert_eq!(steps, [
    ("first", StepResult::Executed),
    ("skipped", StepResult::Skipped),
    ("second", StepResult::Executed),
]);

// Replaying changes from `initial` reproduces the final message context:
let contexts: Vec<_> = trace.contexts().collect();
assert_eq!(serde_json::to_value(contexts[0].data()).unwrap(), json!({"a": 1}));
assert_eq!(contexts.last().unwrap().as_value(), message.context().as_value());
# });
```

A `Trace` serializes cleanly to `{initial, steps, truncated}` and deserializes
back, so you can store traces and replay them later in another service or load
them into the [UI debugger](../ui/overview.md).

## Lightweight Metrics with `Observer`

When you want to collect metrics, counters, histograms, or tracing spans during
execution without the overhead of building a full `Trace`, implement `Observer`.
The engine invokes your observer inline for each event it subscribes to
(`RunStarted`, `WorkflowStarted`, `TaskFinished`, `WorkflowFinished`,
`RunFinished`).

Attach an observer globally via `EngineConfig::observer` or for a single run via
`RunOptions::observer`. Override `Observer::interest()` to select which events
you want to receive; the engine only reads the system clock for task timings if
your observer requests `Interest::TIMINGS`.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::observe::{Event, Observer};
use dataflow_rs::run::EngineConfig;
use dataflow_rs::{Engine, Message};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
struct Calls(AtomicU64);

impl Observer for Calls {
    fn on(&self, event: &Event<'_>) {
        if let Event::TaskFinished { .. } = event {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}

let calls = Arc::new(Calls::default());
let engine = Engine::builder()
    .config(EngineConfig::default().observer(calls.clone()))
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.a", "logic": 1}]}}}
    ]}))
    .build()
    .unwrap();
engine.run(&mut Message::from_json(&json!({}))).await;
assert_eq!(calls.0.load(Ordering::Relaxed), 1);
# });
```

## Common Use Cases

### Debugging Where a Field Changed

```rust
# fn _demo(message: dataflow_rs::Message) {
for entry in message.audit() {
    for change in &entry.changes {
        if &*change.path == "data.total" {
            println!("data.total set by {}/{}", entry.workflow_id, entry.task_id);
            println!("changed from {} to {}", change.old_value, change.new_value);
        }
    }
}
# }
```

### Detecting Whether a Field Was Modified

```rust
use dataflow_rs::Message;

fn was_field_modified(message: &Message, field: &str) -> bool {
    message
        .audit()
        .iter()
        .flat_map(|e| e.changes.iter())
        .any(|c| &*c.path == field)
}
# let _ = was_field_modified;
```

### Compliance Logging

Each `AuditEntry` serializes and deserializes directly with `serde_json`, making
it straightforward to persist audit records to an external log or database.

## Best Practices

1. **Perform all writes through `Ctx`:** Host functions write via `ctx.set`,
   `ctx.emit`, and `ctx.remove`, ensuring every context modification is captured
   consistently.
2. **Disable `capture_changes` in high-throughput pipelines:** If downstream
   code only needs task status codes and not old/new value snapshots, run with
   `RunOptions::default().capture_changes(false)`. See
   [Memory in Long Loops](./loops.md#memory-in-long-loops).
3. **Bound or disable the audit trail when unused:** Use `AuditMode::Last(n)` or
   `AuditMode::Off` when you don't inspect `message.audit()`.
4. **Use `Trace` for step-by-step state inspection:** The audit trail records
   what each task wrote; a `Trace` lets you reconstruct the complete message
   context after any step (including skipped steps and engine bookkeeping).
