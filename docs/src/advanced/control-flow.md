# Control Flow

A workflow's `tasks` array holds **steps**. A step is either:

- a **task**: an object with a `function` key;
- a **group**: an object with a `tasks` key, holding its own nested steps.

Both accept `condition` and `terminal`. A task additionally carries two
**policies**, `on_failure` and `on_rejection`, which say what its outcome
does. Between them that gives the shapes every procedural language has:

| Step | Reads as |
|---|---|
| group, no `terminal` | `if (…) { … }` |
| task with `terminal: true` | early `return` |
| group with `terminal: true` | `if (…) { …; return; }` |
| task with `on_rejection: "halt"` | `if (rejected) return;` |
| task with `on_failure: "halt"` | `if (failed) return;` |
| task with `on_failure: "continue"` | `try { … } catch { log }` |

Every field defaults to the plain behaviour: `terminal` to `false`,
`on_failure` to `"fail"`, `on_rejection` to `"continue"`, and a step with no
nested `tasks` is exactly the task it always was.

## The problem they solve

Without them, every task after a branch has to restate that the branch did
*not* fire, so conditions grow with position:

```json
{
    "tasks": [
        { "id": "load_user",
          "function": { "name": "user_lookup", "input": {} } },

        { "id": "respond_404",
          "condition": {"!": [{"var": "temp_data.user"}]},
          "function": { "name": "map", "input": { "mappings": [
              { "path": "data.status", "logic": 404 } ] } } },

        { "id": "check_pw",
          "condition": {"!!": [{"var": "temp_data.user"}]},
          "function": { "name": "map", "input": { "mappings": [] } } },

        { "id": "issue_tokens",
          "condition": {"and": [{"!!": [{"var": "temp_data.user"}]},
                                {"var": "temp_data.pw.ok"}]},
          "function": { "name": "map", "input": { "mappings": [] } } }
    ]
}
```

With `terminal`, each guard states only its own reason:

```json
{
    "tasks": [
        { "id": "load_user",
          "function": { "name": "user_lookup", "input": {} } },

        { "id": "respond_404", "terminal": true,
          "condition": {"!": [{"var": "temp_data.user"}]},
          "function": { "name": "map", "input": { "mappings": [
              { "path": "data.status", "logic": 404 } ] } } },

        { "id": "check_pw",
          "function": { "name": "map", "input": { "mappings": [] } } },

        { "id": "issue_tokens",
          "function": { "name": "map", "input": { "mappings": [] } } }
    ]
}
```

## `terminal`

`terminal: true` ends the workflow once the task has run.

It is a statement about **position** ("nothing after this runs"), not about
outcome:

| Situation | Halts? |
|---|---|
| The task ran and succeeded | yes |
| Its `condition` was false | no; it never ran |
| It returned `Outcome::Skip` | no |
| It returned a `4xx`, `on_rejection: "continue"` | yes |
| It failed, `on_failure: "continue"` | **yes**; the failure is still recorded |
| It failed, `on_failure: "fail"` | the failure propagates, as always |

Halting stops **this workflow only**. Later workflows on the same engine
still process the message, exactly as with a `filter` task's
`on_reject: "halt"`. Inside a workflow carrying a [`loop`](./loops.md) it
breaks the whole loop, not one sweep.

The audit entry keeps the task's **own** status (`200`, `404`, whatever it
returned) rather than the `299` a `filter` halt or an `Outcome::Halt`
records. The task did its job; only the position is special.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::run::WorkflowExit;
use dataflow_rs::{Engine, Message};
use serde_json::json;

let engine = Engine::builder()
    .workflow_json(&json!({"id": "login", "tasks": [
        {"id": "respond_404", "terminal": true,
         "condition": {"!": [{"var": "data.user"}]},
         "function": {"name": "map", "input": {"mappings": [
             {"path": "data.status", "logic": 404}]}}},
        {"id": "issue_tokens",
         "function": {"name": "map", "input": {"mappings": [
             {"path": "data.tokens", "logic": true}]}}}
    ]}))
    .build()
    .unwrap();

let mut message = Message::from_json(&json!({}));
let run = engine.run(&mut message).await;

// The 404 branch ran and ended the workflow; `issue_tokens` never ran.
let login = run.workflows().next().unwrap();
assert_eq!(login.exit, Some(WorkflowExit::Halted));
assert_eq!(message.to_json()["context"]["data"], json!({"status": 404}));
// The entry keeps the task's own status, not a halt's 299.
assert_eq!(message.audit().len(), 1);
assert_eq!(message.audit()[0].status, 200);
# });
```

For the *outcome* axis (halt only if the task failed or was rejected), use
the policies.

## Policies

A call ends one of four ways, and two policies say what each does:

| The task's outcome | Means | Governed by |
|---|---|---|
| `Ok`, or a status below `400` | success | nothing: the next task runs |
| a status `400` to `499` | **rejection**: the input was refused | `on_rejection` |
| a status of `500` or more, or an `Err` | **failure**: the task could not do its job | `on_failure` |
| `Skip` | nothing happened | nothing: no entry, no progress, the next task runs |

`on_rejection` is `"continue"` (the default: the status is recorded and the
next task runs) or `"halt"` (the workflow stops). `on_failure` is `"fail"`
(the default: the failure passes to the workflow), `"halt"` (the workflow
stops, and the run goes on) or `"continue"` (the failure is recorded and the
next task runs).

A failure that reaches the workflow meets the workflow's own `on_failure`:
`"fail"` (the default) stops the run, which ends `Stopped` with the error,
and `"continue"` records it and goes on to the next workflow, or the next
sweep of a loop.

Whatever the policy, the outcome is recorded first: a `5xx` records
`TASK_STATUS_ERROR` and an `Err` its own code on `message.errors()`, and
the audit entry carries the status (`500` for an `Err`). `terminal` is
applied after the policies, and only upgrades "go on to the next task" into
a halt, so a terminal task that fails under `on_failure: "fail"` still
fails.

### Rejecting with `validation`

A failing `validation` rule returns `400`, a rejection, and rejections
continue by default: plenty of workflows validate to record errors rather
than to gate. To make the assertion reject, say so:

```json
{
    "tasks": [
        { "id": "check_state", "on_rejection": "halt",
          "function": { "name": "validation", "input": { "rules": [
              { "logic": {"==": [{"var": "data.state"}, {"var": "data.cookie"}]},
                "message": "state mismatch" } ] } } },

        { "id": "exchange",
          "function": { "name": "map", "input": { "mappings": [
              { "path": "data.exchanged", "logic": true } ] } } }
    ]
}
```

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
# use dataflow_rs::datavalue::OwnedDataValue;
# use dataflow_rs::run::WorkflowExit;
# use dataflow_rs::{Engine, Message};
# use serde_json::json;
# let engine = Engine::builder()
#     .workflow_json(&json!({"id": "oauth", "tasks": [
#         {"id": "check_state", "on_rejection": "halt",
#          "function": {"name": "validation", "input": {"rules": [
#              {"logic": {"==": [{"var": "data.state"}, {"var": "data.cookie"}]},
#               "message": "state mismatch"}]}}},
#         {"id": "exchange",
#          "function": {"name": "map", "input": {"mappings": [
#              {"path": "data.exchanged", "logic": true}]}}}
#     ]}))
#     .build()
#     .unwrap();
# let message_with = |data: serde_json::Value| Message::from_parts(
#     OwnedDataValue::Null, OwnedDataValue::from(&data), OwnedDataValue::Null);
let mut mismatch = message_with(json!({"state": "a", "cookie": "b"}));
let run = engine.run(&mut mismatch).await;
assert_eq!(run.workflows().next().unwrap().exit, Some(WorkflowExit::Halted));
assert_eq!(mismatch.audit()[0].status, 400);
assert!(mismatch.to_json()["context"]["data"].get("exchanged").is_none());

let mut matching = message_with(json!({"state": "a", "cookie": "a"}));
let run = engine.run(&mut matching).await;
assert_eq!(run.workflows().next().unwrap().exit, Some(WorkflowExit::Completed));
assert_eq!(matching.to_json()["context"]["data"]["exchanged"], json!(true));
# });
```

A `validation` whose rejection nothing acts on is reported by
`Compiler::check` as `UNGUARDED_VALIDATION`, an advisory at the task's
`on_rejection`: the key that fixes it.

**Halting is not a security control.** Later workflows still process the
message, so to reject a message outright return an `Err` from a host
function under `on_failure: "fail"`, or gate the following workflow on the
[error-context path](../core-concepts/error-handling.md).

## Groups

A group states a condition once for a contiguous run of tasks:

```json
{
    "id": "have_videos",
    "condition": {"!!": [{"var": "temp_data.videos"}]},
    "tasks": [
        { "id": "rank", "function": { "name": "map", "input": { "mappings": [] } } },
        { "id": "trim", "function": { "name": "map", "input": { "mappings": [] } } }
    ]
}
```

| Field | Required | Default | Meaning |
|---|---|---|---|
| `id` | **yes** | none | Shares the task id namespace: a group cannot reuse a task's id. |
| `tasks` | **yes** | none | The nested steps. Must not be empty. |
| `condition` | no | `true` | Gates the whole span. |
| `terminal` | no | `false` | Ends the workflow once the group completes. |
| `name`, `description`, `ext` | no | none | For traces and tooling; `ext` is kept verbatim and never read. |

A group has no outcome of its own, so it takes no policy and no `for_each`:
`for_each` on a group is refused as `INVALID_FOR_EACH`. Put either on the
task that can fail or fan out.

**The condition is evaluated once, on entry.** A false result skips the whole
span without evaluating the members' own conditions. This matters when a task
inside the group writes to what the condition reads:

```json
{
    "id": "drain",
    "condition": {"!!": [{"var": "temp_data.queue"}]},
    "tasks": [
        { "id": "take",
          "function": { "name": "map", "input": { "mappings": [
              { "path": "temp_data.taken", "logic": {"var": "temp_data.queue"} },
              { "path": "temp_data.queue", "logic": [] } ] } } },
        { "id": "process",
          "function": { "name": "map", "input": { "mappings": [] } } }
    ]
}
```

`take` empties `temp_data.queue`, but `process` still runs: the block was
entered, and a block runs to its end. Repeating the condition on both tasks
instead would silently skip `process`.

Groups nest, up to `def::MAX_GROUP_DEPTH` (8) levels. A group whose
condition is false skips its whole subtree; an inner false condition skips
only the inner span.

Traces stay task-granular: a skipped group records one `skipped` step per
member task, not a group-level step.

## Choosing between them

`terminal` removes the **negations** of earlier branches. Groups remove the
repetition of a **positive** condition across the tasks that make up one branch.
They compose: a terminal group is a guard clause with a multi-task body:

```json
{
    "id": "reject_unverified",
    "condition": {"!": [{"var": "temp_data.user.verified"}]},
    "terminal": true,
    "tasks": [
        { "id": "audit",
          "function": { "name": "log", "input": { "message": "unverified" } } },
        { "id": "respond_403",
          "function": { "name": "map", "input": { "mappings": [
              { "path": "data.status", "logic": 403 } ] } } }
    ]
}
```

## Compared with `filter`

A [`filter`](../built-in-functions/filter.md) task with `on_reject: "halt"`
also stops a workflow, and still has its place: it is a gate that decides
whether the *rest* of the pipeline should run at all, and it can
`"skip"` instead of halting.

`terminal` is the better fit for a branch that has already done its work,
because the alternative is two tasks per exit whose conditions are hand-written
negations of each other:

```jsonc
{ "id": "respond_404",
  "condition": {"!": [{"var": "temp_data.user"}]},
  "function": { "name": "map", "input": { "mappings": [] } } },
{ "id": "gate",
  "function": { "name": "filter", "input": {
      "condition": {"!!": [{"var": "temp_data.user"}]},
      "on_reject": "halt" } } }
```

Nothing keeps those two conditions in sync. `terminal: true` on the first task
says the same thing once.

## Inspecting the authored shape

`def::parse` reads a definition into a `WorkflowDef` whose `tasks` keep the
authored tree: a `StepDef::Task` or a `StepDef::Group` with its own `tasks`.
A tool that validates, lints or renders definitions walks that tree with
`WorkflowDef::walk`, which yields every step with the path the author typed
and its kind, groups before their members, `loop.setup` first:

```rust
use dataflow_rs::def::{MAX_GROUP_DEPTH, ParseOptions, parse};
use dataflow_rs::program::StepKind;
use serde_json::json;

let parsed = parse(
    &json!({"id": "w", "tasks": [
        {"id": "load", "function": {"name": "map", "input": {"mappings": []}}},
        {"id": "have_user", "condition": true, "tasks": [
            {"id": "greet", "function": {"name": "map", "input": {"mappings": []}}}
        ]}
    ]}),
    ParseOptions::default(),
);
let def = parsed.def.expect("the definition parses");

let steps: Vec<(String, StepKind, &str)> = def
    .walk()
    .map(|(path, kind, step)| (path.to_string(), kind, step.id()))
    .collect();
assert_eq!(steps, [
    ("tasks[0]".to_owned(), StepKind::Task, "load"),
    ("tasks[1]".to_owned(), StepKind::Group, "have_user"),
    ("tasks[1].tasks[0]".to_owned(), StepKind::Task, "greet"),
]);
assert_eq!(MAX_GROUP_DEPTH, 8);
```

Filtering to `StepKind::Task` (and `StepKind::Setup`, for steps under
`loop.setup`) gives exactly the tasks the engine will run, in the order it
runs them. The rules are the engine's own: `parse` is the linter, so an
empty group, a group too deep or a duplicate id is an issue in
`parsed.issues` at the path it yields, and `MAX_GROUP_DEPTH` is the limit
the parser enforces. See
[Authoring-Time Validation](./authoring-validation.md).

## Coming from 3.x

3.x spelt the policies as two flags, `continue_on_error` (a bool on a task
or a workflow) and `halt_on` (`"never"` or `"failure"` on a task). The
compat reader, on by default in `def::parse`, reads them and rewrites each
into the 4.0 policies, reporting a `V3_SPELLING` advisory at the 3.x key:

| Where | 3.x | 4.0 |
|---|---|---|
| Workflow | `continue_on_error: true` | `on_failure: "continue"` |
| Workflow | `continue_on_error: false` | `on_failure: "fail"` |
| Task | `continue_on_error: true` | `on_failure: "continue"`, `on_rejection: "continue"` |
| Task | `halt_on: "failure"` | `on_failure: "fail"`, `on_rejection: "halt"` |
| Task | both | `on_failure: "halt"`, `on_rejection: "halt"` |
| Task | neither, or `halt_on: "never"` | the defaults |

A 3.x flag beside a 4.0 policy on one node is refused (`MIXED_SPELLING` at
the 3.x key): the two flags make one policy, and a mix would be a guess at
how the author meant them to combine. A group's `continue_on_error`, which
3.x reported and never honoured, moves into the group's `ext` and is
reported as `GROUP_CONTINUE_ON_ERROR`; a group's `halt_on` is refused as
`INVALID_HALT_ON`, as 3.x refused it. With the reader off
(`ParseOptions::default().compat(false)`) the 3.x keys are unknown keys and
are refused.

`compat::migrate_v3` rewrites a stored definition into the 4.0 spelling
once, so it reads back with the reader off. The definition it writes leaves
every default out, so a policy at its default (`on_failure: "fail"` here)
has no key:

```rust
use dataflow_rs::compat::migrate_v3;
use serde_json::json;

let migrated = migrate_v3(&json!({
    "id": "w",
    "tasks": [{"id": "t", "halt_on": "failure",
               "function": {"name": "validation", "input": {"rules": []}}}]
}))
.unwrap();
let task = &migrated.json["tasks"][0];
assert_eq!(task["on_rejection"], json!("halt"));
assert!(task.get("on_failure").is_none());
assert!(task.get("halt_on").is_none());
assert_eq!(migrated.rewrites[0].path.to_string(), "tasks[0].halt_on");
assert_eq!(
    migrated.rewrites[0].to_string(),
    "`halt_on: failure` is a v3 spelling, read as `on_failure: fail, on_rejection: halt`"
);
```

See [Migrating from 3.x](../migration/from-3.md).
