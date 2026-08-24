# Authoring-Time Validation

The engine checks a workflow when `Engine::build()` runs. For a service that
*stores* definitions — accepting them from an API, holding them in a database,
building one engine over many rows — that is the wrong moment. One bad row
aborts the whole build, at reload, for every workflow in the process. And the
author who submitted it got no feedback at all.

This page covers the two APIs that move those checks to submission time.

## Checking a definition

`Workflow::validate_authored` takes the JSON a host stored and returns every
problem with it:

```rust
use dataflow_rs::{IssueCode, Workflow};
use serde_json::json;

let submitted = json!({
    "id": "", "name": "checkout",
    "tasks": [
        {"id": "charge", "name": "charge",
         "function": {"name": "map", "input": {"mappings": []}}},
        {"id": "charge", "name": "again",
         "function": {"name": "map", "input": {"mappings": []}}}
    ]
});

let issues = Workflow::validate_authored(&submitted);

// Both problems, not just the first.
let codes: Vec<IssueCode> = issues.iter().map(|i| i.code).collect();
assert!(codes.contains(&IssueCode::EmptyWorkflowId));
assert!(codes.contains(&IssueCode::DuplicateStepId));
```

Each issue carries the coordinate the author typed, so a `400` can point at the
exact field:

```rust
# use dataflow_rs::{IssueCode, Workflow};
# use serde_json::json;
let nested = json!({
    "id": "w", "name": "w",
    "tasks": [
        {"id": "first", "name": "first",
         "function": {"name": "map", "input": {"mappings": []}}},
        {"id": "guard", "condition": true, "tasks": [
            {"id": "first", "name": "collides",
             "function": {"name": "map", "input": {"mappings": []}}}
        ]}
    ]
});

let issues = Workflow::validate_authored(&nested);
assert_eq!(issues[0].code, IssueCode::DuplicateStepId);
assert_eq!(issues[0].path.as_deref(), Some("tasks[1].tasks[0].id"));
assert_eq!(issues[0].task_id.as_deref(), Some("first"));
```

Note the path is `tasks[1].tasks[0]` — where the author wrote it — not the flat
index that task ends up at once the engine flattens the group.

### The guarantee

> `validate_authored` returns empty **if and only if** the JSON parses into a
> `Workflow` and that workflow validates.

Empty is therefore a promise that the engine will accept the definition, not
merely that some list of rules was satisfied. That matters, because the schema
is much larger than the semantic rules: `"priority": "high"`, a `map` task
missing its `mappings`, a misspelled `status` — none of these break a *rule*,
and none of them can load.

Rather than mirror the whole schema, `validate_authored` finishes by actually
parsing the document and reports any failure as `IssueCode::ParseFailed`,
carrying the parser's own message:

```rust
# use dataflow_rs::{IssueCode, Workflow};
# use serde_json::json;
let issues = Workflow::validate_authored(&json!({
    "id": "w", "name": "w", "priority": 0,
    "tasks": [{"id": "t", "name": "t", "function": {"name": "map", "input": {}}}]
}));

assert_eq!(issues[0].code, IssueCode::ParseFailed);
assert!(issues[0].message.contains("mappings"));
```

So a host does **not** need its own round-trip check as a safety net. This is
that safety net, inside the crate where it cannot drift.

## Issue codes

`IssueCode` is `#[non_exhaustive]` — a later minor may add a rule — so match the
codes you care about and let the rest fall through. `as_str()` gives the stable
string form for an API response:

```rust
use dataflow_rs::IssueCode;

assert_eq!(IssueCode::DuplicateStepId.as_str(), "DUPLICATE_STEP_ID");
```

| Code | Means |
|---|---|
| `EMPTY_WORKFLOW_ID` / `EMPTY_WORKFLOW_NAME` | Required identity field missing or blank |
| `NO_TASKS` | `tasks` missing, not an array, or empty |
| `MISSING_STEP_ID` | A task or group carries no `id` |
| `DUPLICATE_STEP_ID` | Two steps share an id — groups share the task namespace |
| `EMPTY_GROUP` | A group's `tasks` is not a non-empty array |
| `GROUP_TOO_DEEP` | Groups nested past `MAX_GROUP_DEPTH` |
| `MISSING_FUNCTION` | A task carries no `function` |
| `INVALID_FUNCTION_NAME` | `function` is not an object with a non-empty `name` |
| `INVALID_TERMINAL` | `terminal` is present but not a boolean |
| `LOOP_INCREMENT_TOO_SMALL` | `increment < 1` — the counter would never reach `max` |
| `LOOP_BOUND_EMPTY` | `max <= init` — no sweep could ever run |
| `LOOP_COUNTER_INVALID` | `counter` is not a non-empty dotted path |
| `PARSE_FAILED` | Does not deserialize; message carries the field and type |
| `VALIDATE_FAILED` | Backstop — parses, but `validate()` still rejects it |

## Checking against the handlers

Shape is only half the question. The other half needs the *registry*: will every
task name a function this engine can actually run?

That is a separate check, because it depends on what you registered rather than
on the document:

```rust
use dataflow_rs::{Engine, Workflow};

let workflow = Workflow::from_json(r#"{
    "id": "w", "name": "w", "priority": 0,
    "tasks": [
        {"id": "a", "name": "a", "function": {"name": "map", "input": {"mappings": []}}},
        {"id": "b", "name": "b",
         "function": {"name": "enrich",
                      "input": {"connector": "c", "merge_path": "data.out"}}}
    ]
}"#).unwrap();

let builder = Engine::builder();
let unrunnable: Vec<&str> = workflow
    .tasks
    .iter()
    .map(|t| t.function.function_name())
    .filter(|name| !builder.can_dispatch(name))
    .collect();

assert_eq!(unrunnable, vec!["enrich"]);
```

`Workflow::tasks` is already flattened, so this covers tasks inside groups
without any extra traversal. See
[Integrations](../built-in-functions/integrations.md) for why `enrich` builds
cleanly and then fails every message.

## Walking the authored tree yourself

For anything the checks above do not cover — your own lint rules, dependency
extraction, a renderer — `walk_authored_steps` gives you the authored tree with
the engine's own grammar:

```rust
use dataflow_rs::engine::steps::{StepKind, walk_authored_steps};
use serde_json::json;

let tasks = json!([
    {"id": "load", "function": {"name": "map", "input": {"mappings": []}}},
    {"id": "guard", "condition": true, "tasks": [
        {"id": "greet", "function": {"name": "map", "input": {"mappings": []}}}
    ]}
]);

let leaves: Vec<&str> = walk_authored_steps(&tasks)
    .filter(|s| s.kind == StepKind::Leaf)
    .map(|s| s.node["id"].as_str().unwrap())
    .collect();

assert_eq!(leaves, vec!["load", "greet"]);
```

See [Control Flow](./control-flow.md#inspecting-the-authored-shape) for the full
walker contract.

## Putting it together

A submission endpoint checks in this order, stopping at the first stage that
reports anything:

1. **`Workflow::validate_authored(&json)`** — shape, with field paths. Reject
   with a `400` listing every issue.
2. **`Workflow::from_json(&text)`** — now guaranteed to succeed if step 1 was
   empty, so `unwrap` is honest here if you prefer.
3. **`builder.can_dispatch(..)` over the tasks** — will this engine run it?
   Reject with a `400` naming the unrunnable functions.

Only then store and activate the definition. `Engine::build()` stays
deliberately permissive — it is not a validation gate, and a host that treats it
as one discovers its problems at reload rather than at submission.
