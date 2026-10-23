# Loops

By default, a workflow executes its task list once. Adding a `loop` configuration
repeats the workflow's tasks as a bounded loop: you can iterate over an array,
run a fixed number of times, step through a counter range, or repeat until the
workflow's `condition` evaluates to `false`.

```json
{
    "id": "per_item",
    "condition": {"<": [{"var": "temp_data.i"}, {"var": "temp_data.n"}]},
    "loop": { "counter": "i", "init": 0, "increment": 1, "max": 10000 },
    "tasks": [ ]
}
```

## Configuration Fields

| Field | Required | Default | Description |
|---|---|---|---|
| `max` | **Yes** | None | Upper bound on the loop counter. Iterations run while `counter < max` (half-open range). |
| `counter` | No | None | `temp_data` path where the engine writes the current counter value (e.g., `"i"` writes to `temp_data.i`). Supports nested dot-paths. |
| `init` | No | `0` | Initial counter value for the first iteration. |
| `increment` | No | `1` | Amount added to the counter after each iteration. Must be `>= 1`. |
| `setup` | No | `[]` | Steps executed once before the first iteration, using the standard task/group grammar. See [Iterating Over an Array](#iterating-over-an-array). |
| `over` | No | None | JSONLogic expression evaluated once after `setup`. Must produce an array; the loop stops when it reaches the end of the array. |
| `as` | No | None | `temp_data` path where the engine writes the current element of `over` on each iteration. Requires `over`. |
| `scratch` | No | None | `temp_data` path reset to `{}` at the start of every iteration for per-iteration temporary state. |

`max` is required so that every loop has a guaranteed upper bound. Setting
`init: 0, max: n` produces counter values `0..n-1`, matching zero-based array
indices. The default values for `init` and `increment` are exposed as constants
at `def::defaults::INIT` and `def::defaults::INCREMENT`.

## Execution Order per Iteration

Each pass through the loop is called a *sweep* (or iteration). On every sweep,
the engine performs the following steps in order:

```text
1. Write the counter to `temp_data.<counter>` (if `counter` is configured)
2. Check `counter < max`                    -> exit loop if false
3. Check the run's deadline and cancel token -> stop the run if expired/cancelled
4. Re-evaluate the workflow `condition`      -> exit loop if false
5. Execute the workflow's `tasks` in sequence
6. Add `increment` to the counter
```

Because the counter is written to `temp_data` *before* the workflow condition is
evaluated, the condition can safely reference or index by the counter on the very
first iteration.

A loop finishes when:
- The counter reaches `max`
- An `over` array has no more elements
- The workflow `condition` evaluates to `false`
- A task halts the workflow (`terminal: true`, `Outcome::Halt`, `filter` halt,
  or a `"halt"` outcome policy)
- An unhandled task failure stops the workflow

Reaching `max` is treated as normal completion (`WorkflowExit::Completed`), not
an error.

**The engine manages the counter.** The counter value is tracked internally by
the engine and written into `temp_data` at the start of each iteration. If a
task overwrites `temp_data.<counter>` during an iteration, the engine replaces
it with the true next counter value on the following iteration. When the loop
finishes, `temp_data.<counter>` holds the first out-of-bounds counter value.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::run::WorkflowExit;
use dataflow_rs::{Engine, Message};
use serde_json::json;

let engine = Engine::builder()
    .workflow_json(&json!({"id": "w", "loop": {"counter": "i", "max": 3}, "tasks": [
        {"id": "seen", "function": {"name": "map", "input": {"mappings": [
            {"path": "data.seen", "logic": {"var": "temp_data.i"}, "mode": "append"}]}}},
        {"id": "clobber", "function": {"name": "map", "input": {"mappings": [
            {"path": "temp_data.i", "logic": 100}]}}}
    ]}))
    .build()
    .unwrap();

let mut message = Message::from_json(&json!({}));
let run = engine.run(&mut message).await;

let w = run.workflows().next().unwrap();
assert_eq!((w.sweeps, w.exit), (3, Some(WorkflowExit::Completed)));
let context = message.to_json()["context"].clone();
assert_eq!(context["data"]["seen"], json!([0, 1, 2]));
assert_eq!(context["temp_data"]["i"], json!(3));
// Each iteration's audit entries are stamped with the loop counter:
let counters: Vec<Option<i64>> = message.audit().iter().map(|e| e.loop_counter).collect();
assert_eq!(counters, [Some(0), Some(0), Some(1), Some(1), Some(2), Some(2)]);
# });
```

## Iterating Over an Array

A common workflow pattern is processing a batch of items: run one-time setup
steps, then execute a sequence of tasks (including custom host functions) for
each item in an array.

Use `over` to specify the array expression, `as` to bind the current element in
`temp_data`, `setup` for steps that run once before the loop starts, and
`scratch` to give each iteration a clean `{}` object for temporary per-item
variables:

```json
{
  "id": "batch",
  "loop": {
    "setup": [
      { "id": "read",
        "function": { "name": "map", "input": { "mappings": [
          { "path": "temp_data.batch", "logic": { "var": "data.claim.items" } } ] } } }
    ],
    "over": { "var": "temp_data.batch" },
    "as": "item",
    "counter": "i",
    "max": 64,
    "scratch": "it"
  },
  "tasks": [
    { "id": "flag", "condition": { ">": [{ "var": "temp_data.item.qty" }, 10] },
      "function": { "name": "map", "input": { "mappings": [
        { "path": "temp_data.it.large", "logic": true } ] } } },
    { "id": "collect",
      "function": { "name": "map", "input": { "mappings": [
        { "path": "data.processed", "mode": "append",
          "logic": { "id": { "var": "temp_data.item.id" },
                     "large": { "!!": [{ "var": "temp_data.it.large" }] } } } ] } } }
  ]
}
```

When `over` is configured, the engine executes the workflow in this order:

```text
1. Evaluate the workflow `condition` once -> if false, skip both `setup` and the loop
2. Run `setup` steps once                 -> if `terminal` or halted here, exit the workflow
3. Evaluate `over` once                   -> must produce an array; otherwise the workflow fails
Then, on each iteration:
4. Write the counter to `temp_data.<counter>`
5. Exit the loop if `counter >= max` or `counter >= over.len()`
6. Check the run's deadline and cancellation token
7. Write `over[counter]` to `temp_data.<as>`
8. Reset `temp_data.<scratch>` to `{}`
9. Re-evaluate the workflow `condition`   -> exit loop if false
10. Execute `tasks` in sequence
11. Add `increment` to the counter
```

With `over` and `scratch`, you don't need manual array-length checks, index
lookups, or cleanup mappings between iterations: `temp_data.it` is reset to `{}`
before every item, so `temp_data.it.large` never leaks from one item to the
next.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
# use dataflow_rs::datavalue::OwnedDataValue;
# use dataflow_rs::{Engine, Message};
# use serde_json::json;
# let engine = Engine::builder()
#     .workflow_json(&json!({"id": "batch", "loop": {
#         "setup": [{"id": "read", "function": {"name": "map", "input": {"mappings": [
#             {"path": "temp_data.batch", "logic": {"var": "data.claim.items"}}]}}}],
#         "over": {"var": "temp_data.batch"}, "as": "item", "counter": "i", "max": 64, "scratch": "it"},
#       "tasks": [
#         {"id": "flag", "condition": {">": [{"var": "temp_data.item.qty"}, 10]},
#          "function": {"name": "map", "input": {"mappings": [{"path": "temp_data.it.large", "logic": true}]}}},
#         {"id": "collect", "function": {"name": "map", "input": {"mappings": [
#             {"path": "data.processed", "mode": "append",
#              "logic": {"id": {"var": "temp_data.item.id"}, "large": {"!!": [{"var": "temp_data.it.large"}]}}}]}}}
#       ]}))
#     .build()
#     .unwrap();
let data = OwnedDataValue::from(&json!({"claim": {"items": [
    {"id": "b", "qty": 20}, {"id": "a", "qty": 5}
]}}));
let mut message = Message::from_parts(OwnedDataValue::Null, data, OwnedDataValue::Null);
let run = engine.run(&mut message).await;
assert_eq!(run.workflows().next().unwrap().sweeps, 2);
assert_eq!(
    message.to_json()["context"]["data"]["processed"],
    json!([{"id": "b", "large": true}, {"id": "a", "large": false}])
);
// Setup steps execute outside the loop and have `loop_counter: None`.
assert_eq!(message.audit()[0].loop_counter, None);
assert_eq!(&*message.audit()[0].task_id, "read");
# });
```

### Important Details for Array Loops

- **The counter is the array index:** `init` sets the starting index (must be
  `>= 0` when `over` is used) and `increment` sets the step size. With the
  defaults (`init: 0`, `increment: 1`), the loop visits each element in order up
  to `max`.
- **`over` must evaluate to an array:** If `over` evaluates to `null` or any
  non-array value, the workflow fails before the first iteration and records a
  `LOGIC_ERROR` under the workflow's ID. (An empty array `[]` is valid and runs
  zero iterations.) How the failure is handled depends on the workflow's
  `on_failure` policy; see [Error Handling in Loops](#error-handling-in-loops).
- **`setup` runs outside the loop:** Audit entries and trace steps from `setup`
  have `loop_counter: None`, `CallInfo::loop_counter` is `None`, and
  `WorkflowRecord::sweeps` counts only loop iterations. `setup` shares the step
  ID namespace with `tasks` and supports nested task groups.
- **A failure in `setup` aborts the workflow:** If a setup step fails, the loop
  never starts. Under workflow `on_failure: "continue"`, the failure is recorded
  and the engine moves on to the next workflow; under `"fail"` (the default),
  the run stops.
- **`scratch` is reset before the workflow condition is re-evaluated:** Each
  iteration starts with `temp_data.<scratch>` set to `{}`. After the loop
  terminates, `counter`, `as`, and `scratch` retain their final values in
  `temp_data`.
- **`setup` and `scratch` work without `over`:** While `as` requires `over`, you
  can use `setup` and `scratch` on any counter or condition loop.
- **Engine loop bindings are not recorded as task audit changes:** Writes to
  `counter`, `as`, and `scratch` happen between iterations outside any task.
  Execution traces record them as dedicated engine steps at the start of each
  iteration.

### Indexing an Array by Counter Manually

If you prefer not to use `over`, you can index into an array manually using the
loop counter and the `val` operator:

```json
[
  {
    "id": "setup", "priority": 0,
    "tasks": [{
      "id": "count",
      "function": { "name": "map", "input": { "mappings": [
        { "path": "temp_data.n",
          "logic": {"reduce": [{"var": "data.items"},
                               {"+": [{"var": "accumulator"}, 1]}, 0]} },
        { "path": "data.processed", "logic": [] }
      ]}}
    }]
  },
  {
    "id": "per_item", "priority": 1,
    "condition": {"<": [{"var": "temp_data.i"}, {"var": "temp_data.n"}]},
    "loop": { "counter": "i", "max": 10000 },
    "tasks": [
      {
        "id": "pick",
        "function": { "name": "map", "input": { "mappings": [
          { "path": "temp_data.item",
            "logic": {"val": [["data", "items", {"var": "temp_data.i"}]]} }
        ]}}
      },
      {
        "id": "collect",
        "function": { "name": "map", "input": { "mappings": [
          { "path": "data.processed", "mode": "append",
            "logic": {"var": "temp_data.item.id"} }
        ]}}
      }
    ]
  }
]
```

Keep in mind that **`temp_data` persists across iterations** unless you use
`scratch` or clear fields explicitly. If a task writes to a `temp_data` field
conditionally (for example, `{"if": [cond, value, null]}`, where `null` is
skipped by `map`), the previous iteration's value will remain in that field on
iterations where the condition is false. Either place per-iteration fields under
`scratch`, add `"on_null": "unset"` to the mapping, or unset the field at the
end of the iteration with `{"path": "temp_data.slot", "unset": true}`.

## Running a Fixed Number of Times

To run a workflow's tasks a fixed number of times, omit `condition` and set
`max`:

```json
{ "id": "three_times",
  "loop": { "max": 3 },
  "tasks": [ ] }
```

`counter` is optional—the engine still tracks the iteration count internally and
stamps `loop_counter` on every audit entry.

## Repeating Until a Condition Becomes False

You can drive a loop with the workflow's `condition` (such as polling or
following pagination cursors) while using `max` as a safety limit:

```json
{ "id": "paginate",
  "condition": {"!!": [{"var": "temp_data.next_cursor"}]},
  "loop": { "max": 1000 },
  "tasks": [ ] }
```

If evaluating the workflow `condition` produces an error, it is treated as
`false` and ends the loop.

## Breaking Out of a Loop Early

The workflow `condition` is checked only *between* iterations. To exit a loop
partway through an iteration, use any workflow-halting mechanism:
- A task or group with `terminal: true`
- A `filter` task with `on_reject: "halt"`
- A task with `on_rejection: "halt"` or `on_failure: "halt"`
- A custom host function returning `Ok(Outcome::Halt)`

Each of these exits the entire loop (not just the current iteration).

```json
{
  "id": "stop_on_error",
  "function": { "name": "filter", "input": {
    "condition": {"!": [{"var": "temp_data.item.invalid"}]},
    "on_reject": "halt"
  }}
}
```

To skip only a single task while continuing the current iteration, use `filter`
with `on_reject: "skip"` or a task `condition`.

## Error Handling in Loops

When a task failure propagates to the workflow level (because the task returned
a `5xx` status or `Err` under its default `on_failure: "fail"` policy), the
workflow's own `on_failure` policy decides what happens next:

- **`on_failure: "continue"`**: The engine records the error and **advances to
  the next iteration** of the loop rather than aborting the remaining items.
  This is ideal for batch processing where a failure on item 7 should not
  prevent item 8 from being processed.
- **`on_failure: "fail"` (default)**: The failure stops the loop and the engine
  run immediately.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::run::{RunStatus, WorkflowExit};
use dataflow_rs::{Engine, Message};
use serde_json::json;

// `data.batch` is missing, so `over` evaluates to `null` instead of an array.
let engine = Engine::builder()
    .workflow_json(&json!({"id": "batch", "priority": 0, "on_failure": "continue",
        "loop": {"over": {"var": "data.batch"}, "as": "item", "max": 10},
        "tasks": [{"id": "t", "function": {"name": "log", "input": {"message": "item"}}}]}))
    .workflow_json(&json!({"id": "after", "priority": 1,
        "tasks": [{"id": "mark", "function": {"name": "map", "input": {"mappings": [
            {"path": "data.after", "logic": true}]}}}]}))
    .build()
    .unwrap();

let mut message = Message::from_json(&json!({}));
let run = engine.run(&mut message).await;

assert!(matches!(run.status(), RunStatus::CompletedWithErrors));
let exits: Vec<_> = run.workflows().map(|w| w.exit).collect();
assert_eq!(exits, [Some(WorkflowExit::Failed), Some(WorkflowExit::Completed)]);
let error = &message.errors()[0];
assert_eq!(error.code, "LOGIC_ERROR");
assert_eq!(error.workflow_id.as_deref(), Some("batch"));
assert_eq!(error.task_id, None);
assert_eq!(message.to_json()["context"]["data"]["after"], json!(true));
# });
```

## Audit Trail in Loops

Every task executed inside a loop records an `AuditEntry` stamped with
`loop_counter`, indicating which iteration produced it:

```json
{
  "workflow_id": "per_item",
  "task_id": "call",
  "status": 200,
  "loop_counter": 7,
  "changes": []
}
```

`loop_counter` is recorded even when `loop.counter` is omitted in the workflow
definition. Workflows without a `loop` omit `loop_counter` from their audit
entries. Trace steps and `TaskFinished` observer events also include
`loop_counter`.

### Memory in Long Loops

Each iteration appends one `AuditEntry` per executed task—so a 1,000-iteration
loop with 3 tasks produces 3,000 audit entries. When `capture_changes` is
enabled (the default), every entry also stores clones of the old and new values
for every context write. In long loops or loops that modify large fields, those
change snapshots can consume significant memory.

If you do not need `AuditEntry::changes`, disable change capture on
`RunOptions`:

```rust
# use dataflow_rs::RunOptions;
let opts = RunOptions::default().capture_changes(false);
# assert!(!opts.capture_changes);
```

(Execution traces capture their own changes independently of `capture_changes`,
so disabling `capture_changes` does not affect traced runs.)

If you also want to cap the number of `AuditEntry` records retained on the
message during a long loop, configure `AuditMode::Last(n)` or `AuditMode::Off`:

```rust
# use dataflow_rs::run::AuditMode;
# use dataflow_rs::RunOptions;
let opts = RunOptions::default()
    .capture_changes(false)
    .audit(AuditMode::Last(64));
# assert_eq!(opts.audit, AuditMode::Last(64));
```

See [Bounding Audit Trail Memory](./audit-trails.md#bounding-audit-trail-memory)
for more details.

## Performance

Workflows without a `loop` incur zero overhead from loop support.

During a loop run, the engine maintains a single arena-backed view of the
message context and splices each write directly into that view, so each
iteration only pays for the fields it reads and writes rather than the total
size of the message context. See [Performance](./performance.md) for benchmark
details.

## Compile-Time Validation

`def::parse` validates loop configurations at authoring and build time,
reporting any invalid configuration with `Rejected` severity:

- `max <= init`: The loop range is empty (`LOOP_BOUND_EMPTY`).
- `increment < 1`: The counter would never advance (`LOOP_INCREMENT_TOO_SMALL`).
- Empty or malformed `counter`, `as`, or `scratch` path
  (`LOOP_COUNTER_INVALID`, `LOOP_SLOT_INVALID`).
- `as` configured without `over` (`LOOP_ITEM_WITHOUT_OVER`).
- `over` set to a scalar literal (string, number, boolean, or `null`) that can
  never evaluate to an array, or `init < 0` combined with `over`
  (`LOOP_OVER_INVALID`).
- Overlapping paths among `counter`, `as`, and `scratch`—for example, `"it"` and
  `"it.item"` (`LOOP_SLOT_COLLISION`).
- Invalid steps inside `setup`, or step IDs in `setup` that duplicate step IDs
  in `tasks` (`DUPLICATE_STEP_ID`).

See [Authoring-Time Validation](./authoring-validation.md) for the full list of
issue codes.
