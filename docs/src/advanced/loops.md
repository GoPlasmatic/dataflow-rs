# Loops

By default a workflow runs its task list exactly once. Adding a `loop` field
turns that single pass into a bounded `for` loop: the engine repeats the task
list once per counter value, so a set of tasks can run per array item, a fixed
number of times, or until a condition on the message goes false.

```json
{
    "id": "per_item",
    "name": "Per item",
    "condition": {"<": [{"var": "temp_data.i"}, {"var": "temp_data.n"}]},
    "loop": { "counter": "i", "init": 0, "increment": 1, "max": 10000 },
    "tasks": [ ]
}
```

## Fields

| Field | Required | Default | Meaning |
|---|---|---|---|
| `max` | **yes** | — | Sweeps run while `counter < max`. The bound is half-open. |
| `counter` | no | none | `temp_data` field the engine maintains, e.g. `"i"` → `temp_data.i`. Dot-paths nest. |
| `init` | no | `0` | First counter value. |
| `increment` | no | `1` | Added after each sweep. Must be `>= 1`. |
| `setup` | no | `[]` | Steps run once, before the first sweep, in the normal step grammar. See [Iterating an array](#iterating-an-array). |
| `over` | no | none | JSONLogic evaluated once, after `setup`. Must yield an array; the loop then also stops at its end. |
| `as` | no | none | `temp_data` field holding the current element of `over`. Requires `over`. |
| `scratch` | no | none | `temp_data` field reset to `{}` at the start of every sweep. |

`max` has no default on purpose. It is what makes termination structural: a
loop stops because of its bound, not because a condition was written correctly.
`init: 0, max: n` yields counter values `0..n-1` — exactly array indices.

## What happens per sweep

One iteration of the loop is called a *sweep*. Per sweep the engine:

```text
1. writes the counter to temp_data (if `counter` names it)
2. checks `counter < max`            -> stop if not
3. re-evaluates the workflow condition -> stop if false
4. runs the whole task list, exactly as a non-looping workflow would
5. adds `increment` to the counter
```

The counter is written *before* the condition is evaluated, so a condition that
indexes by it resolves on the very first sweep.

A loop ends when the counter reaches `max`, when an `over` array runs out, when
the condition goes false, when
a task halts the workflow, or when a task error stops it. Reaching `max` is
normal completion, not an error — the bound was author-supplied, so hitting it
is the stated intent.

The engine owns the counter. It is rewritten before every sweep, so a task in
the body that writes the same `temp_data` path has its value replaced at the
next increment.

## Iterating an array

The most common use is a batch: read it once, then run a set of tasks —
including async ones like `http_call` — once per element. `over` makes the
array the loop's second bound, `as` exposes the element, `setup` holds the
once-only steps, and `scratch` gives each sweep a fresh object for its own
state.

```json
{
  "id": "batch", "name": "Batch",
  "loop": {
    "setup": [
      { "id": "claim", "name": "Claim the batch",
        "function": { "name": "http_call", "input": { "connector": "claims" } } },
      { "id": "read", "name": "Read it",
        "function": { "name": "map", "input": { "mappings": [
          { "path": "temp_data.batch", "logic": { "var": "data.claim.body" } } ] } } }
    ],
    "over": { "var": "temp_data.batch.items" },
    "as": "item",
    "counter": "i",
    "max": 64,
    "scratch": "it"
  },
  "tasks": [
    { "id": "call", "name": "Call the API for this item",
      "continue_on_error": true,
      "function": { "name": "http_call", "input": { "connector": "item_api" } } },
    { "id": "note", "name": "Note a failure",
      "condition": { ">=": [{ "var": "metadata.progress.status_code" }, 500] },
      "function": { "name": "map", "input": { "mappings": [
        { "path": "temp_data.it.failed", "logic": true } ] } } },
    { "id": "collect", "name": "Collect",
      "function": { "name": "map", "input": { "mappings": [
        { "path": "data.processed",
          "logic": { "merge": [{ "var": "data.processed" },
                     [{ "id": { "var": "temp_data.item.id" },
                        "failed": { "!!": [{ "var": "temp_data.it.failed" }] } }]] } } ] } } }
  ]
}
```

What the engine does, in order:

```text
1. evaluates the workflow condition    -> a false result skips setup and loop alike
2. runs `setup` once                   -> `terminal` or a halt here ends the workflow
3. evaluates `over` once               -> must be an array; anything else is an error
then, per sweep:
4. writes the counter to temp_data
5. stops if counter >= max, or counter >= the array's length
6. writes over[counter] to temp_data.<as>
7. resets temp_data.<scratch> to {}
8. re-evaluates the workflow condition -> stop if false
9. runs the task list
10. adds `increment` to the counter
```

No guard on the setup steps, no `filter` comparing the counter with the
length, no `map` copying `rows[i]` into a slot, and no clearing of per-item
slots: `temp_data.it.failed` starts every item absent, so an item after a
failed one is not reported as failed.

The counter **is** the element index. `init` is therefore a starting offset and
`increment` a stride, and `init` must be `>= 0`. With the defaults the loop
visits every element, up to `max`. Reaching `max` with elements left is normal
completion, logged as a warning.

- **`over` must yield an array.** `null` — a missing batch — is a workflow
  error naming `loop.over`, recorded as `WORKFLOW_ERROR`, not a silent
  zero-sweep run. Zero sweeps is spelled `[]`.
- **Setup is not a sweep.** Its audit entries and trace steps carry no
  `loop_counter`, `TaskContext::loop_counter()` is `None` inside it, and the
  observer's `WorkflowFinished::sweeps` counts sweeps only. Setup shares the
  step id namespace with `tasks`, and allows groups.
- **A setup error ends the workflow.** There is no first sweep without a
  completed setup. With `continue_on_error: true` on the workflow the error is
  recorded and the *next workflow* still runs; without it the message stops.
- **`scratch` is reset before the condition**, so a sweep — condition
  included — starts from `{}`. It is not cleared when the loop ends: like the
  counter and `as`, it is left holding the last sweep's state.
- **`as` needs `over`**; `setup` and `scratch` do not, and are just as useful
  on a counter loop.
- **The engine's own writes are not audit changes.** The counter, the element
  and the scratch reset land outside any task; the `loop_counter` stamp is what
  identifies the sweep.

### Indexing by counter

Before `over` existed, per-item processing indexed the array by the counter
and let the condition stop the loop. That still works, and is what a loop whose
length is not an array needs:

```json
[
  {
    "id": "setup", "name": "Setup", "priority": 0,
    "tasks": [{
      "id": "count", "name": "Count the items",
      "function": { "name": "map", "input": { "mappings": [
        { "path": "temp_data.n",
          "logic": {"reduce": [{"var": "data.items"},
                               {"+": [{"var": "accumulator"}, 1]}, 0]} },
        { "path": "data.processed", "logic": [] }
      ]}}
    }]
  },
  {
    "id": "per_item", "name": "Per item", "priority": 1,
    "condition": {"<": [{"var": "temp_data.i"}, {"var": "temp_data.n"}]},
    "loop": { "counter": "i", "max": 10000 },
    "tasks": [
      {
        "id": "pick", "name": "Pick the item at i",
        "function": { "name": "map", "input": { "mappings": [
          { "path": "temp_data.item",
            "logic": {"val": [["data", "items", {"var": "temp_data.i"}]]} }
        ]}}
      },
      {
        "id": "call", "name": "Call the API for this item",
        "function": { "name": "http_call", "input": { "connector": "item_api" } }
      },
      {
        "id": "collect", "name": "Collect the result",
        "function": { "name": "map", "input": { "mappings": [
          { "path": "data.processed",
            "logic": {"merge": [{"var": "data.processed"},
                                [{"var": "temp_data.item.id"}]]} }
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

Every operator used here — `reduce`, `<`, `+`, `merge`, and computed-path `val`
— is a core operator, available without enabling any `ext-*` cargo feature.

**`temp_data` carries over between sweeps**; only the engine-owned slots are
rewritten. For per-item state, `scratch` is the answer — see
[Iterating an array](#iterating-an-array). Outside it, a per-item slot written
only *sometimes* — `{"if": [cond, value, null]}`, whose
`null` is skipped — still holds the previous item's value in a sweep that does
not write it. Clear it explicitly: `{"path": "temp_data.slot", "unset": true}`
at the end of the body, or `"on_null": "unset"` on the mapping that sets it.
`"logic": null` clears nothing, and `false` is a value that `missing` and `??`
still see. See [Removing a Path](../built-in-functions/map.md#removing-a-path).

## Running a fixed number of times

Omit the condition; the bound alone drives the loop.

```json
{ "id": "three_times", "name": "Three times",
  "loop": { "max": 3 },
  "tasks": [ ] }
```

The counter does not have to be named. The engine still tracks it, and the
audit trail still records it.

## Repeating until a condition goes false

Let the condition do the work and treat `max` as the safety bound.

```json
{ "id": "paginate", "name": "Paginate",
  "condition": {"!!": [{"var": "temp_data.next_cursor"}]},
  "loop": { "max": 1000 },
  "tasks": [ ] }
```

If the loop stops because it hit `max` while the condition was still true, the
engine logs a warning — the bound beat the condition, which usually means the
condition never became false.

## Breaking out mid-body

The workflow condition is only checked *between* sweeps. To stop part-way
through a sweep, use a `filter` task with `on_reject: halt`; it breaks the whole
loop, not just the current sweep.

```json
{
  "id": "stop_on_error", "name": "Stop on error",
  "function": { "name": "filter", "input": {
    "condition": {"!": [{"var": "temp_data.item.invalid"}]},
    "on_reject": "halt"
  }}
}
```

Use `on_reject: skip` instead to skip only that task and let the sweep continue.

## Errors inside a loop

Error handling is unchanged from a non-looping workflow, with one addition: if
a task error propagates to the workflow level and the workflow has
`continue_on_error: true`, the loop advances to the next sweep rather than
abandoning the remaining iterations. That is what the per-item case wants —
item 7 failing should not stop item 8 from being processed. With
`continue_on_error: false`, the error stops the loop and the message, exactly
as it stops a non-looping workflow.

## Audit trail

Each sweep records its own audit entries, stamped with `loop_counter` — the
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

Execution traces carry the same field on each step, so a trace can be grouped
by iteration.

### Memory in long loops

Every sweep adds one audit entry per task — a 1,000-sweep loop over 3 tasks
records 3,000 entries, and the `max` bound is what keeps that finite. With
`capture_changes` on, which is the default, each entry also holds a deep copy of
the old and new value of every write, and none of it is released until
`process_message` returns. So a loop's memory grows with
sweeps × writes × value size, at about 65 bytes per number written.

Three `map` tasks each writing a 2,000-number array per sweep, release build:

| sweeps | `capture_changes(true)` | `capture_changes(false)` |
|---:|---:|---:|
| 250 | ~100 MB | 8 MB |
| 500 | ~196 MB | 8 MB |

At `max: 10000`, one 10,000-number array per sweep comes to several GB. If you
do not read `AuditTrail::changes`, turn capture off for the message. The loop
still records one entry per task per sweep, just without values:

```rust
# use dataflow_rs::Message;
# use serde_json::json;
let message = Message::builder()
    .data_json(&json!({"state": [0.0, 1.0, 2.0]}))
    .capture_changes(false)
    .build();
# assert!(!message.capture_changes());
```

Tracing with `TraceOptions { changes: true }` reports the captured diff and
does not turn capture on, so it shows empty diffs for such a message.

## Performance

A workflow without a `loop` is unaffected — it takes the same code path it
always did, with no added checks per message.

A looping workflow opens one arena scope per sweep rather than sharing one
across the whole loop. That is deliberate: the arena is a bump allocator and
never frees mid-scope, so a shared scope would grow memory with the iteration
count. A consequence is that a fully-synchronous looping workflow does not join
the shared-arena run that consecutive fully-sync workflows normally share.

## Validation

These are rejected at `Engine::build()` rather than at runtime:

- `max <= init` — the half-open bound could never run a sweep.
- `increment < 1` — the counter would never advance.
- an empty or malformed `counter`, `as` or `scratch` path.
- `as` without `over`.
- an `over` that is a scalar literal (a string, number, boolean or `null`) — it
  could never be an array.
- `init < 0` alongside `over` — an index cannot be negative.
- `counter`, `as` or `scratch` naming the same path, or one inside another
  (`it` and `it.item`).
- a `setup` step that breaks the step grammar, or reuses an id from `tasks`.
