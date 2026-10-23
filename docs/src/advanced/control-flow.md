# Control Flow

A workflow's `tasks` array is a sequence of **steps**. Each step is either:

- a **task**: an object with a `function` key that executes a built-in or custom
  function
- a **group**: an object with a `tasks` key that holds a nested list of steps

Both tasks and groups accept `condition` and `terminal`. Tasks also support two
outcome **policies**—`on_failure` and `on_rejection`—that control what happens
after the task finishes. Together, these fields give you familiar structured
control-flow patterns:

| Step Configuration | Equivalent Control Flow |
|---|---|
| Group without `terminal` | `if (…) { … }` |
| Task with `terminal: true` | Early `return` |
| Group with `terminal: true` | `if (…) { …; return; }` |
| Task with `on_rejection: "halt"` | `if (rejected) return;` |
| Task with `on_failure: "halt"` | `if (failed) return;` |
| Task with `on_failure: "continue"` | `try { … } catch { log }` |

All control-flow fields have sensible defaults: `terminal` defaults to `false`,
`on_failure` defaults to `"fail"`, and `on_rejection` defaults to `"continue"`.

## Why Structured Control Flow Matters

Without early exits (`terminal`), every task following a conditional branch has
to repeat the negation of that branch's condition, causing conditions to grow
more complex with each step:

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

With `terminal: true`, guard clauses exit the workflow as soon as they run, so
subsequent steps don't need to re-check earlier conditions:

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

## Early Exit with `terminal`

Setting `terminal: true` stops the current workflow after the task or group
executes.

`terminal` is based on **whether the step ran**, not whether it succeeded or
rejected:

| Scenario | Halts the Workflow? |
|---|---|
| The task ran and succeeded | Yes |
| The task's `condition` evaluated to `false` | No (the task did not run) |
| The task returned `Outcome::Skip` | No |
| The task returned a `4xx` status with `on_rejection: "continue"` | Yes |
| The task failed with `on_failure: "continue"` | **Yes** (and the failure is still recorded) |
| The task failed with `on_failure: "fail"` | The failure propagates as usual |

Halting a workflow stops **only the current workflow**. Any subsequent workflows
in the engine still process the message, just like a `filter` task with
`on_reject: "halt"`. Inside a workflow with a [`loop`](./loops.md), `terminal`
exits the entire loop rather than just the current iteration.

When a `terminal` task halts a workflow, its audit entry records the task's
**actual status code** (such as `200` or `404`) rather than the `299` status
used by `filter` halts or `Outcome::Halt`.

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
// The audit entry preserves the task's own status (200), not 299.
assert_eq!(message.audit().len(), 1);
assert_eq!(message.audit()[0].status, 200);
# });
```

If you want to halt a workflow based on the task's *outcome* (for example, halt
only when a task fails or is rejected), use `on_failure` and `on_rejection`
instead.

## Outcome Policies (`on_failure` and `on_rejection`)

Every task execution finishes in one of four ways, governed by two policies:

| Task Outcome | Meaning | Governed By |
|---|---|---|
| `Ok`, or status `< 400` | **Success** | Continues to the next task |
| Status `400..=499` | **Rejection** (input did not pass a check) | `on_rejection` |
| Status `>= 500`, or `Err` | **Failure** (task could not complete its work) | `on_failure` |
| `Skip` | **No-op** | Continues to the next task (no audit entry or progress update) |

- **`on_rejection`** accepts:
  - `"continue"` (default): Records the `4xx` status and continues to the next
    task.
  - `"halt"`: Stops the current workflow.
- **`on_failure`** accepts:
  - `"fail"` (default): Propagates the failure to the workflow level.
  - `"halt"`: Stops the current workflow and allows the engine run to continue
    to the next workflow.
  - `"continue"`: Records the failure and continues to the next task.

When a task failure propagates to the workflow level, it is handled by the
workflow's own `on_failure` policy:
- `"fail"` (default): Stops the entire engine run with status `Stopped` and
  returns the error.
- `"continue"`: Records the failure and moves on to the next workflow (or the
  next iteration of a loop).

Regardless of the policy, the engine always records the task's outcome first: a
`5xx` status adds a `TASK_STATUS_ERROR` record to `message.errors()`, an `Err`
adds a record with the error's code, and the audit entry logs the status (`500`
for an `Err`). `terminal` is evaluated *after* outcome policies and only turns
"continue to the next task" into a workflow halt—so a terminal task that fails
under `on_failure: "fail"` still fails the workflow.

### Halting on Validation Rejections

When a `validation` rule fails, the task returns status `400` (a rejection).
Because `on_rejection` defaults to `"continue"`, workflows can run multiple
validation tasks to collect all errors before deciding what to do. If you want a
failed validation task to stop the workflow immediately, set
`on_rejection: "halt"`:

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

If a `validation` task leaves `on_rejection` at `"continue"` and nothing later
in the workflow checks for errors, `Compiler::check` reports an
`UNGUARDED_VALIDATION` advisory pointing at the task's `on_rejection` field.

**Note:** Halting a workflow is not a global security gate across multiple
workflows. Because subsequent workflows in the engine still process the message,
either return an `Err` from a host function with `on_failure: "fail"` to stop
the entire run, or check the [error-context path](../core-concepts/error-handling.md)
in downstream workflows.

## Task Groups

A **group** applies a single condition to a contiguous sequence of steps:

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

| Field | Required | Default | Description |
|---|---|---|---|
| `id` | **Yes** | None | Unique step ID within the workflow (shares the same ID namespace as tasks). |
| `tasks` | **Yes** | None | Non-empty array of nested tasks or groups. |
| `condition` | No | `true` | JSONLogic condition that gates the entire group. |
| `terminal` | No | `false` | If `true`, halts the workflow once the group finishes executing. |
| `name`, `description`, `ext` | No | None | Metadata for traces and UI tools (`ext` is preserved verbatim). |

Because a group does not execute a function of its own, it does not accept
`on_failure`, `on_rejection`, or `for_each` (setting `for_each` on a group is
rejected with `INVALID_FOR_EACH`). Place those fields on the individual tasks
inside the group.

**A group's condition is evaluated once when entering the group.** If the
condition is `false`, the entire group is skipped without evaluating the
conditions of its inner steps. If the condition is `true`, the engine enters the
group and runs its steps to completion—even if an earlier task inside the group
modifies the field that the group's condition checked:

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

Here, `take` empties `temp_data.queue`, and `process` still runs because the
`drain` group was already entered. (If you had duplicated the condition on both
tasks instead of using a group, `process` would have been skipped.)

Groups can be nested up to `def::MAX_GROUP_DEPTH` (8) levels deep. Execution
traces remain task-granular: when a group is skipped, the trace records a
`skipped` step for each task inside the group.

## Combining Groups and `terminal`

Use `terminal` to avoid repeating **negated** conditions after an early-exit
branch, and use groups to avoid repeating a **positive** condition across
multiple tasks in the same branch. You can combine both to create a multi-step
guard clause:

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

## `terminal` vs. `filter`

A [`filter`](../built-in-functions/filter.md) task with `on_reject: "halt"` can
also stop a workflow. Use `filter` when you want a standalone gate that decides
whether the rest of the workflow (or a single task via `"skip"`) should proceed.

Use `terminal: true` when a branch performs work (such as setting a `404`
response) and then needs to exit. Without `terminal`, you would need two
separate tasks with opposite conditions that must be kept in sync manually:

```jsonc
{ "id": "respond_404",
  "condition": {"!": [{"var": "temp_data.user"}]},
  "function": { "name": "map", "input": { "mappings": [] } } },
{ "id": "gate",
  "function": { "name": "filter", "input": {
      "condition": {"!!": [{"var": "temp_data.user"}]},
      "on_reject": "halt" } } }
```

Setting `"terminal": true` on `respond_404` expresses the same intent in a
single step.

## Walking the Authored Step Tree

`def::parse` parses a workflow definition into a `WorkflowDef` whose `tasks`
preserve the nested structure of `StepDef::Task` and `StepDef::Group` nodes.
Tools that validate, lint, or visualize workflows can traverse this tree with
`WorkflowDef::walk()`, which yields each step along with its authored JSON path
and `StepKind` (visiting `loop.setup` steps first, and each group before its
children):

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

Filtering `walk()` to `StepKind::Task` (and `StepKind::Setup` for loop setup
steps) gives the exact sequence of tasks the engine executes. See
[Authoring-Time Validation](./authoring-validation.md) for more details on
structural validation.

## Migrating from 3.x

In 3.x, failure and rejection behavior was configured using `continue_on_error`
(a boolean on tasks and workflows) and `halt_on` (`"never"` or `"failure"` on
tasks). By default, `def::parse` enables the 3.x compatibility reader, which
automatically translates these fields into 4.0 policies and emits a
`V3_SPELLING` advisory at the 3.x key:

| Scope | 3.x Configuration | 4.0 Equivalent |
|---|---|---|
| Workflow | `continue_on_error: true` | `on_failure: "continue"` |
| Workflow | `continue_on_error: false` | `on_failure: "fail"` |
| Task | `continue_on_error: true` | `on_failure: "continue"`, `on_rejection: "continue"` |
| Task | `halt_on: "failure"` | `on_failure: "fail"`, `on_rejection: "halt"` |
| Task | Both `continue_on_error: true` and `halt_on: "failure"` | `on_failure: "halt"`, `on_rejection: "halt"` |
| Task | Neither, or `halt_on: "never"` | Default policies (`on_failure: "fail"`, `on_rejection: "continue"`) |

- Combining a 3.x flag (`continue_on_error` or `halt_on`) with a 4.0 policy
  (`on_failure` or `on_rejection`) on the same node is rejected with
  `MIXED_SPELLING`.
- Setting `continue_on_error` on a group (which 3.x ignored) moves the field
  into the group's `ext` map and emits a `GROUP_CONTINUE_ON_ERROR` advisory.
- Setting `halt_on` on a group is rejected with `INVALID_HALT_ON`.
- When the compatibility reader is disabled (`ParseOptions::default().compat(false)`),
  3.x keys are treated as unknown keys and rejected.

To permanently upgrade stored workflow JSON to the 4.0 format, use
`compat::migrate_v3`. It rewrites 3.x fields into 4.0 policies (omitting keys
that match their default values) so the workflow can be parsed with
`.compat(false)`:

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

See [Migrating from 3.x](../migration/from-3.md) for the complete migration
guide.
