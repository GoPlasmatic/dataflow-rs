# Loops

By default a workflow runs its task list exactly once. Adding a `loop` field
turns that single pass into a bounded `for` loop: the engine repeats the task
list once per counter value, so a set of tasks can run per array item, a fixed
number of times, or until a condition on the message goes false.

```json
{
    "id": "per_item",
    "condition": {"<": [{"var": "temp_data.i"}, {"var": "temp_data.n"}]},
    "loop": { "counter": "i", "init": 0, "increment": 1, "max": 10000 },
    "tasks": [ ]
}
```

## Fields

| Field | Required | Default | Meaning |
|---|---|---|---|
| `max` | **yes** | none | Sweeps run while `counter < max`. The bound is half-open. |
| `counter` | no | none | `temp_data` field the engine maintains, e.g. `"i"` writes `temp_data.i`. Dot-paths nest. |
| `init` | no | `0` | First counter value. |
| `increment` | no | `1` | Added after each sweep. Must be `>= 1`. |
| `setup` | no | `[]` | Steps run once, before the first sweep, in the normal step grammar. See [Iterating an array](#iterating-an-array). |
| `over` | no | none | JSONLogic evaluated once, after `setup`. Must yield an array; the loop then also stops at its end. |
| `as` | no | none | `temp_data` field holding the current element of `over`. Requires `over`. |
| `scratch` | no | none | `temp_data` field reset to `{}` at the start of every sweep. |

`max` has no default on purpose. It makes termination structural: a loop stops
because of its bound, not because a condition was written correctly.
`init: 0, max: n` yields counter values `0..n-1`, exactly the array indices.
The defaults are `def::defaults::INIT` and `def::defaults::INCREMENT`.

## What happens per sweep

One iteration of the loop is called a *sweep*. Per sweep the engine:

```text
1. writes the counter to temp_data (if `counter` names it)
2. checks `counter < max`              -> stop if not
3. checks the run's deadline and token -> stop the run if either passed
4. re-evaluates the workflow condition -> stop if false
5. runs the whole task list, exactly as a non-looping workflow would
6. adds `increment` to the counter
```

The counter is written *before* the condition is evaluated, so a condition that
indexes by it resolves on the first sweep.

A loop ends when the counter reaches `max`, when an `over` array runs out, when
the condition goes false, when a task halts the workflow, or when a failure
stops it. Reaching `max` is normal completion, not an error: the author supplied
the bound, so hitting it is the stated intent.

**The engine owns the counter.** It lives in the engine's own frame and the
context only gets a copy, rewritten before every sweep, so a task in the body
that writes the same `temp_data` path has its value replaced at the next
sweep. After the loop the slot holds the first counter out of bounds.

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
// Each sweep's entries carry its counter.
let counters: Vec<Option<i64>> = message.audit().iter().map(|e| e.loop_counter).collect();
assert_eq!(counters, [Some(0), Some(0), Some(1), Some(1), Some(2), Some(2)]);
# });
```

## Iterating an array

The most common use is a batch: read it once, then run a set of tasks
(host functions included) once per element. `over` makes the array the
loop's second bound, `as` exposes the element, `setup` holds the once-only
steps, and `scratch` gives each sweep a fresh object for its own state.

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

In order, the engine:

```text
1. evaluates the workflow condition    -> a false result skips setup and loop alike
2. runs `setup` once                   -> `terminal` or a halt here ends the workflow
3. evaluates `over` once               -> must be an array; anything else fails the workflow
then, per sweep:
4. writes the counter to temp_data
5. stops if counter >= max, or counter >= the array's length
6. checks the run's deadline and token
7. writes over[counter] to temp_data.<as>
8. resets temp_data.<scratch> to {}
9. re-evaluates the workflow condition -> stop if false
10. runs the task list
11. adds `increment` to the counter
```

The workflow needs no guard on the setup steps, no `filter` comparing the
counter with the length, no `map` copying `items[i]` into a slot, and no
clearing of per-item slots: `temp_data.it.large` starts every item absent, so
an item after a large one is not reported as large.

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
// Setup is not a sweep: its entry carries no counter.
assert_eq!(message.audit()[0].loop_counter, None);
assert_eq!(&*message.audit()[0].task_id, "read");
# });
```

The counter **is** the element index. `init` is therefore a starting offset and
`increment` a stride, and `init` must be `>= 0`. With the defaults the loop
visits every element, up to `max`. Reaching `max` with elements left is normal
completion.

- **`over` must yield an array.** `null` (a missing batch) fails the
  workflow before the first sweep, with one error record naming `loop.over`
  under the workflow's id, not a silent zero-sweep run. Zero sweeps is
  spelled `[]`. What the failure does next is the workflow's
  `on_failure`; see [Errors inside a loop](#errors-inside-a-loop).
- **Setup is not a sweep.** Its audit entries and trace steps carry no
  `loop_counter`, `CallInfo::loop_counter` is `None` inside it, and the
  run's `WorkflowRecord::sweeps` counts sweeps only. Setup shares the step
  id namespace with `tasks`, and allows groups.
- **A setup failure ends the workflow.** There is no first sweep without a
  completed setup. With `on_failure: "continue"` on the workflow the failure
  is recorded and the *next workflow* still runs; with the default `"fail"`
  the run stops.
- **`scratch` is reset before the condition**, so a sweep, condition
  included, starts from `{}`. It is not cleared when the loop ends: like the
  counter and `as`, it is left holding the last sweep's state.
- **`as` needs `over`**; `setup` and `scratch` do not, and are equally useful
  on a counter loop.
- **The engine's own writes are not audit changes.** The counter, the element
  and the scratch reset land outside any task; the `loop_counter` stamp
  identifies the sweep. A trace records them as the engine's own step,
  with no task, as each sweep is entered.

### Indexing by counter

A loop whose length is not an array indexes by the counter and lets the
condition stop the loop:

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

Two things make this work:

- **`val` evaluates its path argument**, so `{"val": [["data", "items",
  {"var": "temp_data.i"}]]}` indexes the array by the current counter.
- **No `advance` task is needed.** The engine increments `i` after each sweep.

Every operator used here (`reduce`, `<`, `+`, and computed-path `val`) is a
core operator, available without enabling any `ext-*` cargo feature.

**`temp_data` carries over between sweeps**; only the engine-owned slots are
rewritten. For per-item state, use `scratch`; see
[Iterating an array](#iterating-an-array). Outside it, a per-item slot written
only *sometimes* (`{"if": [cond, value, null]}`, whose
`null` is skipped) still holds the previous item's value in a sweep that does
not write it. Clear it explicitly: `{"path": "temp_data.slot", "unset": true}`
at the end of the body, or `"on_null": "unset"` on the mapping that sets it.
`"logic": null` clears nothing, and `false` is a value that `missing` and `??`
still see. See [Map](../built-in-functions/map.md).

## Running a fixed number of times

Omit the condition; the bound alone drives the loop.

```json
{ "id": "three_times",
  "loop": { "max": 3 },
  "tasks": [ ] }
```

The counter does not have to be named. The engine still tracks it, and the
audit trail still records it.

## Repeating until a condition goes false

Let the condition do the work and treat `max` as the safety bound.

```json
{ "id": "paginate",
  "condition": {"!!": [{"var": "temp_data.next_cursor"}]},
  "loop": { "max": 1000 },
  "tasks": [ ] }
```

A condition that fails to evaluate counts as false, so an error in it ends
the loop as a `false` does.

## Breaking out mid-body

The workflow condition is only checked *between* sweeps. To stop part-way
through a sweep, use a `filter` task with `on_reject: "halt"`, a task with
`terminal: true`, or a task whose `on_rejection` or `on_failure` is
`"halt"`: each breaks the whole loop, not only the current sweep.

```json
{
  "id": "stop_on_error",
  "function": { "name": "filter", "input": {
    "condition": {"!": [{"var": "temp_data.item.invalid"}]},
    "on_reject": "halt"
  }}
}
```

Use `on_reject: "skip"` instead to skip only that task and let the sweep continue.

## Errors inside a loop

Error handling is unchanged from a non-looping workflow, with one addition: a
failure that reaches the workflow (a task's `5xx` or `Err` under its default
`on_failure: "fail"`) meets the workflow's own `on_failure`. With
`"continue"` the failure is recorded and the loop advances to the next
sweep rather than abandoning the remaining iterations. The per-item case
wants this: item 7 failing should not stop item 8 from being processed.
With the default `"fail"`, the failure stops the loop and the run, exactly
as it stops a non-looping workflow.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::run::{RunStatus, WorkflowExit};
use dataflow_rs::{Engine, Message};
use serde_json::json;

// `data.batch` is missing, so `over` gives `null`, not an array.
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

## Audit trail

Each sweep records its own audit entries, stamped with `loop_counter`, the
counter value for that sweep:

```json
{
  "workflow_id": "per_item",
  "task_id": "call",
  "status": 200,
  "loop_counter": 7,
  "changes": []
}
```

Because `increment` is at least 1, the counter strictly increases, so it both
identifies the iteration and tells you which item the entry refers to. It is
recorded even when the loop leaves its counter unnamed. Entries from workflows
without a `loop` omit the field entirely.

Trace steps carry the same field, so a trace can be grouped by iteration,
and a `TaskFinished` event's `CallInfo` carries it for an observer.

### Memory in long loops

Every sweep adds one audit entry per task: a 1,000-sweep loop over 3 tasks
records 3,000 entries, and the `max` bound keeps that finite. With
`capture_changes` on, which is the default, each entry also holds a copy of
the old and new value of every write, and none of it is released until the
run returns. So a loop's memory grows with sweeps, writes and value size.

If you do not read `AuditEntry::changes`, turn capture off for the run. The
loop still records one entry per task per sweep, without values:

```rust
# use dataflow_rs::RunOptions;
let opts = RunOptions::default().capture_changes(false);
# assert!(!opts.capture_changes);
```

A trace records its own changes whatever `capture_changes` says, so turning
capture off does not empty a traced run's diffs.

The entries themselves are small, but a long loop still holds one per task per
sweep. If nothing reads `Message::audit`, for example because you trace
the run or route on `metadata.progress`, bound them with `AuditMode::Last(n)`
or drop them with `AuditMode::Off`:

```rust
# use dataflow_rs::run::AuditMode;
# use dataflow_rs::RunOptions;
let opts = RunOptions::default()
    .capture_changes(false)
    .audit(AuditMode::Last(64));
# assert_eq!(opts.audit, AuditMode::Last(64));
```

The mode changes only what the message keeps. Status classification, error
records, `metadata.progress` and halting are the same in every mode. See
[Bounding the trail](./audit-trails.md#bounding-the-trail).

## Performance

A workflow without a `loop` is unaffected: it takes the same code path it
always did, with no added checks per message.

A run keeps one view of the context for its whole length and splices each
write into it, so a sweep pays for what it writes, not for the bytes the
context carries. The `loop_scaling` benchmark, a 1,000-sweep loop over a
256 KiB context appending to a log every sweep, measured against 3.15
before 4.0's release at about 2% of 3.15's time per sweep, with the per-sweep
figure flat between 250 and 1,000 sweeps. See
[Performance](./performance.md).

## Validation

`def::parse` reports each of these as a Rejected issue at its key, so
`Engine::builder().build()` refuses the workflow rather than letting it
fail at runtime:

- `max <= init`: the half-open bound could never run a sweep (`LOOP_BOUND_EMPTY`).
- `increment < 1`: the counter would never advance (`LOOP_INCREMENT_TOO_SMALL`).
- an empty or malformed `counter`, `as` or `scratch` path
  (`LOOP_COUNTER_INVALID`, `LOOP_SLOT_INVALID`).
- `as` without `over` (`LOOP_ITEM_WITHOUT_OVER`).
- an `over` that is a scalar literal (a string, number, boolean or `null`), which
  could never be an array, or `init < 0` alongside `over`, since an index
  cannot be negative (`LOOP_OVER_INVALID`).
- `counter`, `as` or `scratch` naming the same path, or one inside another
  (`it` and `it.item`) (`LOOP_SLOT_COLLISION`).
- a `setup` step that breaks the step grammar, or reuses an id from `tasks`.

The compiler checks the bounds again, since a host may edit a parsed
definition before compiling it. See
[Authoring-Time Validation](./authoring-validation.md).
