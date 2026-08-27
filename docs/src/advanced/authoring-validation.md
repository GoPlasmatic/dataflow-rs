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

That is the *shape* question, and it is the whole of it — but it is not the
same as "this engine can run it". `Engine::build()` also resolves every task to
a handler, so a structurally perfect definition naming an unregistered function
still aborts a build. The next section covers that half.

The guarantee still matters, because the schema is much larger than the semantic
rules: `"priority": "high"`, a `map` task missing its `mappings`, a misspelled
`status` — none of these break a *rule*, and none of them can load.

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
| `UNKNOWN_FUNCTION` | No handler registered, and not a built-in — usually a typo |
| `MISSING_HANDLER` | A config-only integration with nothing registered under its name |
| `INPUT_PARSE` | A custom task's `input` does not match its handler's `Input` type |
| `TEMPLATE_COMPILE` | A handler rejected the input at construction time |
| `UNKNOWN_SECRET` | An expression names a secret the engine does not declare — see [Secrets](./secrets.md) |
| `SECRET_IN_MESSAGE_WRITE` | A `map` mapping or `log` expression reads a secret, which the engine would record |

## Checking against the handlers

Shape is only half the question. The other half needs the *registry*: will every
task name a function this engine can actually run, with an input its handler can
parse? That is what `Engine::build()` decides — and a host that lets it decide
finds out at reload, when one bad row takes down every workflow in the process.

`check_workflow` asks the same questions and reports instead of aborting:

```rust
use dataflow_rs::{Engine, IssueCode, Workflow};

let workflow = Workflow::from_json(r#"{
    "id": "w", "name": "w", "priority": 0,
    "tasks": [{"id": "lookup", "name": "lookup",
               "function": {"name": "enrich",
                            "input": {"connector": "c", "merge_path": "data.out"}}}]
}"#).unwrap();

let issues = Engine::builder().check_workflow(&workflow);

assert_eq!(issues[0].code, IssueCode::MissingHandler);
assert_eq!(issues[0].task_id.as_deref(), Some("lookup"));
```

Both `EngineBuilder` and `Engine` carry it, with identical semantics — screen
before you build, or against the engine you are already running. The second is
usually what a live host wants: the submission endpoint holds a built engine
behind its reload mechanism, not the builder that made it.

### Why `MISSING_HANDLER` is its own code

`enrich`, `http_call` and `publish_kafka` ship as config schemas with no
implementation. A workflow using one deserializes into a *typed* variant, so
`Engine::build()` accepts it without complaint — and then every message fails
with `FunctionNotFound`. Reporting that as `UNKNOWN_FUNCTION` would send the
author hunting for a typo that isn't there; the fix is a registration, and the
code says so.

### Anchoring and paths

Issues from `check_workflow` are anchored on `task_id` — step ids are unique
across tasks and groups — with a path *relative to that task*:

```text
task_id: "lookup"
path:    "function.input"
```

To point at the authored document, join it with the coordinate
`walk_authored_steps` reports for that id:

```text
tasks[1].tasks[0]  +  function.input
```

The reason it works this way is that `check_workflow` receives an already-parsed
`Workflow`, whose `tasks` is flattened — the authored nesting is gone. Emitting a
flat `tasks[3]` would point at the wrong element in the author's document, which
is worse than not pointing at all.

That flattening does mean tasks inside groups are checked automatically, with no
extra traversal.

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
3. **`engine.check_workflow(&workflow)`** — will this engine run it? Reject
   with a `400` naming the tasks and what each needs.

Only then store and activate the definition. `Engine::build()` stays
deliberately permissive — it is not a validation gate, and a host that treats it
as one discovers its problems at reload rather than at submission.
