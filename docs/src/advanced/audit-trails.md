# Audit Trails

A run leaves two records of what it did to a message: the **audit trail**,
one entry per task that ran, kept on the message; and, when asked for, a
**trace**, the initial context plus every change, from which the context
after any step can be rebuilt. This page covers both, and the knobs that
bound them.

## The audit trail

Every task that ran, and did not `Skip`, appends one `AuditEntry` to the
message:

| Field | Holds |
|---|---|
| `workflow_id`, `task_id` | Where the entry came from |
| `timestamp` | The run's one clock reading: every entry of a run carries the same instant |
| `status` | The task's status code: `200` for `Outcome::Ok`, the code of `Status(n)`, `299` for `Halt`, `500` for an `Err` |
| `changes` | The writes the task made, when changes are captured |
| `loop_counter` | The sweep's counter, inside a workflow's [`loop`](./loops.md); absent otherwise |
| `element_index` | The element, inside a [`for_each`](./for-each.md); absent otherwise |

Each `Change` is one write or removal:

| Field | Holds |
|---|---|
| `path` | The dotted path written, `data.full_name` |
| `old_value` | What the path held before; `null` when it was absent |
| `new_value` | The value written; `null` for a removal |
| `removed` | The key was removed rather than written. Absent in JSON when `false` |

`removed` marks a removal: a `map` mapping with `unset`, `on_null: "unset"`
meeting a `null` result, or a host function's `ctx.remove`. `new_value` is
then `null`, which on its own could not tell "now absent" from "now holds
`null`", so read the flag. A `Change` also carries `replaced`, which only a
trace sets (see below); on an audit entry it is always `false` and absent
from the JSON.

A host function that returns `Err` still records an entry, with `status`
`500` and an empty `changes` list: its writes were dropped, so there is
nothing to record. The error itself goes to `message.errors()`.

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

### JSON representation

The trail is written as `audit_trail` on the message, as 3.x wrote it:

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

### What gets tracked

- **`map`** records one `Change` per mapping that wrote: a write, a removal,
  or, for `mode: "append"` and `"extend"`, one per element appended at
  `path.<index>`.
- **A host function** records one `Change` per `ctx.set`, `ctx.emit` or
  `ctx.remove`, in call order, once the call returns `Ok`. A second write to
  one path leaves two changes and one value.
- **`validation`** writes nothing, so its entry carries an empty `changes`
  list; its `status` is `200` when every rule passed and `400` when one
  failed. The rules that failed land on `message.errors()`.
- **The engine's own writes are not changes.** `metadata.progress` after
  every task, a loop's counter, element and scratch, and a fan-out's result
  array land outside any entry. A trace records them.

Only `Outcome::Skip` suppresses an entry entirely.

## Bounding the trail

Two options on the run decide what the message keeps. Both used to sit on
the message in 3.x; in 4.0 they are `RunOptions`, so one message can be run
either way.

**`capture_changes`** (default `true`) decides whether each write's `Change`
is recorded. Capturing copies the old and the new value of every write, which
is the largest cost in a mapping-heavy workload and grows with every sweep
of a long loop. With it off, every entry's `changes` is empty; the entries,
statuses and error records are unchanged.

**`audit`** (default `AuditMode::Full`) decides which entries are kept:
`Full` keeps every one, `Last(n)` the most recent `n`, and `Off` none. Under
`Off` and `Last(0)` no changes are recorded either, since no entry would
hold them. Control flow, error records and `metadata.progress` are the same
in every mode; only the trail is.

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

// Every entry, without the values.
let mut message = Message::from_json(&json!({}));
engine.run_with(&mut message, &RunOptions::default().capture_changes(false)).await;
assert_eq!(message.audit().len(), 3);
assert!(message.audit().iter().all(|e| e.changes.is_empty()));

// The last two entries only.
let mut message = Message::from_json(&json!({}));
engine.run_with(&mut message, &RunOptions::default().audit(AuditMode::Last(2))).await;
let kept: Vec<&str> = message.audit().iter().map(|e| &*e.task_id).collect();
assert_eq!(kept, ["b", "c"]);

// No trail at all; the context is still written.
let mut message = Message::from_json(&json!({}));
engine.run_with(&mut message, &RunOptions::default().audit(AuditMode::Off)).await;
assert!(message.audit().is_empty());
assert_eq!(message.to_json()["context"]["data"], json!({"a": 1, "b": 2, "c": 3}));
# });
```

## Traces

A trace is asked for per run with `RunOptions::trace(TraceOptions)` and
comes back on the `Run` (`run.trace()`, or `run.take_trace()` to own it).
It holds:

- **`initial`**: the context after the run's stamps (`metadata.processed_at`,
  `metadata.engine_version`, `metadata.channel` under a channel) and before
  any workflow;
- **`steps`**: one `TraceStep` per recorded call, per step a condition
  skipped, per workflow skipped or failed, and per write the engine made as
  a loop's sweep is entered. Each carries its `result` (`executed`,
  `skipped` or `failed`), the call's `status`, its `changes` in order (the
  engine's writes made for it, the call's own, the `metadata.progress`
  write, the error context's), the `errors` it added, its `loop_counter`
  and `element_index`, and, when `TraceOptions::timings` asked, how long the
  call ran;
- **`truncated`**: whether `TraceOptions::max_bytes` was crossed, after
  which every step keeps everything but its changes.

Nothing is snapshotted. The context after any step is `Trace::contexts()`,
which replays the steps' changes from `initial` in order; the last one is
the message's context, on every status, the run that stopped included. For
a `map` step, `Trace::mapping_contexts(step)` replays mapping by mapping.

A trace records its own changes whatever `capture_changes` says, which
governs the audit trail alone. Three differences from the audit entry's
changes follow:

- a trace's `Change` at a context root says `replaced` when a host function
  wrote the root whole, where `map`'s root mapping merges keys into it; a
  replay needs to know which, and an audit entry never carries the flag;
- a host function's `Err` drops its writes, so its `executed` or `failed`
  step has none; a built-in's step keeps the writes it made before failing,
  so a replay still reaches the message's context;
- `TraceOptions::redact` masks the values at named paths in `initial` and in
  every change, as each is captured; the message keeps the real values.

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

// The replay from `initial` ends at the message's context.
let contexts: Vec<_> = trace.contexts().collect();
assert_eq!(serde_json::to_value(contexts[0].data()).unwrap(), json!({"a": 1}));
assert_eq!(contexts.last().unwrap().as_value(), message.context().as_value());
# });
```

A trace serializes as `{initial, steps, truncated}` and reads back from what
it writes, so a stored trace replays to the same contexts later, in another
process or in the [UI debugger](../ui/overview.md).

## Observers

For counters, histograms and spans kept by the host as a run goes, an
`Observer` is cheaper than a trace: it is called inline with each event it
asked for (`RunStarted`, `WorkflowStarted`, `TaskFinished`,
`WorkflowFinished`, `RunFinished`), copies out what it keeps, and returns.
Attach one to every run with `EngineConfig::observer` or to one run with
`RunOptions::observer`. Its `interest()` decides which events it gets, and
no clock is read unless it asks for `Interest::TIMINGS`.

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

## Use cases

### Debugging

Find where a value was set:

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

### Change detection

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

### Compliance

Persist each entry as it is: `workflow_id`, `task_id`, `timestamp`, `status`
and `changes` serialize as the JSON above, and an `AuditEntry` reads back
from what it wrote.

## Practical notes

1. **Write through `Ctx`.** A host function's `set`, `emit` and `remove`
   record the changes; there is no other way to write, so nothing bypasses
   capture.
2. **Turn capture off in bulk pipelines** that never read `changes`, with
   `RunOptions::default().capture_changes(false)`. The entries still record
   each task's status. See [Memory in long loops](./loops.md#memory-in-long-loops).
3. **Bound the trail** with `AuditMode::Last(n)` or `AuditMode::Off` when
   nothing reads it, for instance because the run is traced or routes on
   `metadata.progress`.
4. **Trace when you need the context after a step.** The audit trail says
   what each task wrote; a trace says what the whole context was, and
   replays on every status.
