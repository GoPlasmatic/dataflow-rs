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
| `max` | **yes** | — | Sweeps run while `counter < max`. The bound is half-open. |
| `counter` | no | none | `temp_data` field the engine maintains, e.g. `"i"` → `temp_data.i`. Dot-paths nest. |
| `init` | no | `0` | First counter value. |
| `increment` | no | `1` | Added after each sweep. Must be `>= 1`. |

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

A loop ends when the counter reaches `max`, when the condition goes false, when
a task halts the workflow, or when a task error stops it. Reaching `max` is
normal completion, not an error — the bound was author-supplied, so hitting it
is the stated intent.

The engine owns the counter. It is rewritten before every sweep, so a task in
the body that writes the same `temp_data` path has its value replaced at the
next increment.

## Per-item processing

The most common use: run a set of tasks — including async ones like
`http_call` — once per element of an array.

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

Note that audit volume scales with iteration count: a 1,000-sweep loop over 3
tasks records 3,000 entries, each with its changes when `capture_changes` is
on. The `max` bound is what keeps that finite.

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
- an empty or malformed `counter` path.
