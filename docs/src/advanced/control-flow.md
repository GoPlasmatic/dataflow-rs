# Control Flow

A workflow's `tasks` array holds **steps**, not just tasks. A step is either:

- a **task** — an object with a `function` key;
- a **group** — an object with a `tasks` key, holding its own nested steps.

Both accept `condition` and `terminal`. Between them that gives the three
shapes every procedural language has:

| Step | Reads as |
|---|---|
| group, no `terminal` | `if (…) { … }` |
| task with `terminal: true` | early `return` |
| group with `terminal: true` | `if (…) { …; return; }` |

Both fields default to today's behaviour, so an existing workflow is unchanged:
`terminal` defaults to `false`, and a step with no nested `tasks` is exactly the
task it always was.

## The problem they solve

> The examples on this page use `length`, which belongs to the optional
> `ext-string` operator family. Operator families are **off by default**, and
> because the engine always evaluates in templating mode an operator whose
> family is disabled is not an error — the object passes through as literal
> data, and a condition built on it is silently never true. Build with
> `--features ext-string` (or `all-operators`) to run these examples, or
> rewrite them with core operators. (Going the other way, `{"$length": …}`
> is a literal object whatever is enabled.) See
> [JSONLogic](./jsonlogic.md#operator-families-cargo-features) for the full list.

Without them, every task after a branch has to restate that the branch did
*not* fire, so conditions grow with position:

```json
{
    "tasks": [
        { "id": "load_user", "name": "Load user",
          "function": { "name": "mongo_read", "input": {} } },

        { "id": "respond_404", "name": "404",
          "condition": {"==": [{"length": [{"var": "temp_data.users"}]}, 0]},
          "function": { "name": "map", "input": {} } },

        { "id": "check_pw", "name": "Check password",
          "condition": {">": [{"length": [{"var": "temp_data.users"}]}, 0]},
          "function": { "name": "map", "input": {} } },

        { "id": "issue_tokens", "name": "Issue tokens",
          "condition": {"and": [{">": [{"length": [{"var": "temp_data.users"}]}, 0]},
                                {"var": "temp_data.pw.ok"}]},
          "function": { "name": "map", "input": {} } }
    ]
}
```

With `terminal`, each guard states only its own reason:

```json
{
    "tasks": [
        { "id": "load_user", "name": "Load user",
          "function": { "name": "mongo_read", "input": {} } },

        { "id": "respond_404", "name": "404", "terminal": true,
          "condition": {"==": [{"length": [{"var": "temp_data.users"}]}, 0]},
          "function": { "name": "map", "input": {} } },

        { "id": "check_pw", "name": "Check password",
          "function": { "name": "map", "input": {} } },

        { "id": "issue_tokens", "name": "Issue tokens",
          "function": { "name": "map", "input": {} } }
    ]
}
```

## `terminal`

`terminal: true` ends the workflow once the task has run.

It is a statement about **position** — "nothing after this runs" — not about
outcome:

| Situation | Halts? |
|---|---|
| The task ran and succeeded | yes |
| Its `condition` was false | no — it never ran |
| It returned `TaskOutcome::Skip` | no |
| It failed, `continue_on_error: true` | **yes** — the error is still recorded |
| It failed, `continue_on_error: false` | the error propagates, as always |

Halting stops **this workflow only**. Later workflows registered on the same
engine still process the message, exactly as with a `filter` task's
`on_reject: "halt"`. Inside a workflow carrying a [`loop`](./loops.md) it breaks
the whole loop, not one sweep.

The audit-trail entry keeps the task's **own** status — `200`, `404`, whatever
it returned — rather than the `299` a filter-halt records. The task did its job;
only the position is special.

## Groups

A group states a condition once for a contiguous run of tasks:

```json
{
    "id": "have_videos",
    "name": "Rank and trim",
    "condition": {">": [{"length": [{"var": "temp_data.videos"}]}, 0]},
    "tasks": [
        { "id": "rank", "name": "Rank", "function": { "name": "map", "input": {} } },
        { "id": "trim", "name": "Trim", "function": { "name": "map", "input": {} } }
    ]
}
```

| Field | Required | Default | Meaning |
|---|---|---|---|
| `id` | **yes** | — | Shares the task id namespace — a group cannot reuse a task's id. |
| `tasks` | **yes** | — | The nested steps. Must not be empty. |
| `condition` | no | `true` | Gates the whole span. |
| `terminal` | no | `false` | Ends the workflow once the group completes. |
| `name`, `description` | no | none | For traces and tooling. |

**The condition is evaluated once, on entry.** A false result skips the whole
span without evaluating the members' own conditions. This matters when a task
inside the group writes to what the condition reads:

```json
{
    "id": "drain",
    "condition": {">": [{"length": [{"var": "temp_data.queue"}]}, 0]},
    "tasks": [
        { "id": "take", "name": "Take the queue",
          "function": { "name": "map", "input": { "mappings": [
              { "path": "temp_data.taken", "logic": {"var": "temp_data.queue"} },
              { "path": "temp_data.queue", "logic": [] } ] } } },
        { "id": "process", "name": "Process what we took",
          "function": { "name": "map", "input": {} } }
    ]
}
```

`take` empties `temp_data.queue`, but `process` still runs — the block was
entered, and a block runs to its end. Repeating the condition on both tasks
instead would silently skip `process`.

Groups nest, up to 8 levels. A group whose condition is false skips its whole
subtree; an inner false condition skips only the inner span.

Traces stay task-granular: a skipped group records one skipped step per member
task, not a group-level step.

## Choosing between them

`terminal` removes the **negations** of earlier branches. Groups remove the
repetition of a **positive** condition across the tasks that make up one branch.
They compose — a terminal group is a guard clause with a multi-task body:

```json
{
    "id": "reject_unverified",
    "condition": {"!": [{"var": "temp_data.user.verified"}]},
    "terminal": true,
    "tasks": [
        { "id": "audit", "name": "Audit the rejection",
          "function": { "name": "log", "input": { "message": "unverified" } } },
        { "id": "respond_403", "name": "403",
          "function": { "name": "map", "input": {} } }
    ]
}
```

## Compared with `filter`

A [`filter`](../built-in-functions/filter.md) task with `on_reject: "halt"`
also stops a workflow, and still has its place — it is a gate that decides
whether the *rest* of the pipeline should run at all, and it can `skip` instead
of halting.

`terminal` is the better fit for a branch that has already done its work,
because the alternative is two tasks per exit whose conditions are hand-written
negations of each other:

```json
{ "id": "respond_404", "name": "404",
  "condition": {"==": [{"length": [{"var": "temp_data.users"}]}, 0]},
  "function": { "name": "map", "input": {} } },
{ "id": "gate", "name": "Stop if we answered",
  "function": { "name": "filter", "input": {
      "condition": {"!": [{"==": [{"length": [{"var": "temp_data.users"}]}, 0]}]},
      "on_reject": "halt" } } }
```

Nothing keeps those two conditions in sync. `terminal: true` on the first task
says the same thing once.

## Inspecting the authored shape

The parser flattens the step tree: by the time you hold a `Workflow`, `tasks` is
a flat list and the grouping survives only as internal span bookkeeping. That is
right for the executor, but a tool that *validates* or *lints* definitions needs
the shape the author typed, so it can point at `tasks[1].tasks[0].id` rather
than a flat index.

`walk_authored_steps` walks the authored JSON without building any `Task`s:

```rust
use dataflow_rs::engine::steps::{StepKind, walk_authored_steps};
use serde_json::json;

let tasks = json!([
    {"id": "load", "function": {"name": "map", "input": {"mappings": []}}},
    {"id": "have_user", "condition": true, "tasks": [
        {"id": "greet", "function": {"name": "map", "input": {"mappings": []}}}
    ]}
]);

for step in walk_authored_steps(&tasks) {
    match step.kind {
        StepKind::Leaf => println!("task at {}", step.path),
        StepKind::Group => println!("group at {} (depth {})", step.path, step.depth),
        StepKind::TooDeep => println!("too deeply nested: {}", step.path),
    }
}
// task at tasks[0]
// group at tasks[1] (depth 0)
// task at tasks[1].tasks[0]
```

Traversal is document order, groups before their members — so filtering to
`StepKind::Leaf` gives you exactly the tasks the engine will run, in the order
it will run them.

Two properties matter for a validator:

- **The walk never fails.** Parsing stops at the first bad element; this walk
  reports malformed elements, empty groups and over-deep nesting as *nodes*, so
  you can collect every problem in one pass instead of one per round trip.
- **The rules are the engine's own.** `is_group` is the same test the parser
  makes — presence of a `tasks` key, nothing else — and `MAX_GROUP_DEPTH` is the
  limit it enforces. Read them rather than copying them, and a future change to
  either follows automatically:

```rust
use dataflow_rs::engine::steps::{MAX_GROUP_DEPTH, is_group};
use serde_json::json;

assert!(is_group(&json!({"id": "g", "tasks": []})));
assert!(!is_group(&json!({"id": "t", "function": {"name": "map"}})));
assert_eq!(MAX_GROUP_DEPTH, 8);
```

Note that a `tasks` key holding something that is not an array is still a
*group* — a malformed one, which the parser rejects as such. Reading it as a
task instead would classify it differently from the engine that has to run it.

## Version note

`terminal` and groups need engine **3.6.0** or newer.

A group sent to an older engine fails to parse — the group object has no
`function`, so it is rejected with a clear error. A bare `terminal: true` on an
older engine is **silently ignored**, and every later task runs. If you deploy
workflow definitions to engines you do not control, gate on the engine version
before authoring `terminal`.
