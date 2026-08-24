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

## Version note

`terminal` and groups need engine **3.6.0** or newer.

A group sent to an older engine fails to parse — the group object has no
`function`, so it is rejected with a clear error. A bare `terminal: true` on an
older engine is **silently ignored**, and every later task runs. If you deploy
workflow definitions to engines you do not control, gate on the engine version
before authoring `terminal`.
