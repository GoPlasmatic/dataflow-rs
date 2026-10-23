# Authoring-Time Validation

Services that accept workflow definitions from an API or store them in a
database need to validate definitions as soon as they are submitted—rather than
waiting until the engine is next rebuilt. Dataflow-rs provides two validation
stages, both of which report every issue at the exact JSON path where it was
written:

1. **`def::parse`** parses the JSON into a `WorkflowDef` and checks all
   structural rules, unknown keys, and 3.x compatibility spellings.
2. **`Compiler`** compiles a parsed `WorkflowDef` against a `Registry`, checking
   that every task references a registered function, every function input
   compiles cleanly, and every JSONLogic expression passes static checks.

`Engine::builder().build()` runs both stages automatically and fails if any
definition is rejected. That works well when your application loads a fixed set
of workflows at startup. If your application loads workflows dynamically from a
database and needs to quarantine invalid definitions while running the valid
ones, call `def::parse` and `Compiler` directly.

## Parsing a Definition (`def::parse`)

`def::parse` reads a single workflow JSON value into a `WorkflowDef` and lints
its structure, unknown keys, and 3.x spellings. It returns a `Parsed` struct
containing all discovered `issues`, and sets `def` to `Some(WorkflowDef)` when
no issue has `Rejected` severity.

If you already own the `serde_json::Value`, use `def::parse_owned(value, options)`
(or `EngineBuilder::workflow_json_owned(value)`), which rewrites 3.x
compatibility spellings in place without cloning the document.

```rust
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::program::IssueCode;
use serde_json::json;

let submitted = json!({
    "id": "",
    "tasks": [
        {"id": "charge", "function": {"name": "map", "input": {"mappings": []}}},
        {"id": "charge", "function": {"name": "map", "input": {"mappings": []}}}
    ]
});

let parsed = parse(&submitted, ParseOptions::default());
assert!(parsed.def.is_none());

// Reports all problems in a single pass, not just the first one encountered.
let codes: Vec<IssueCode> = parsed.issues.iter().map(|i| i.code).collect();
assert_eq!(codes, [IssueCode::EmptyWorkflowId, IssueCode::DuplicateStepId]);
```

Each `Issue` includes the exact authored path in the submitted JSON and the step
where the problem occurred, making it easy to return detailed `400 Bad Request`
responses:

```rust
# use dataflow_rs::def::{ParseOptions, parse};
# use dataflow_rs::program::{IssueCode, StepKind};
# use serde_json::json;
let nested = json!({
    "id": "w",
    "tasks": [
        {"id": "first", "function": {"name": "map", "input": {"mappings": []}}},
        {"id": "guard", "condition": true, "tasks": [
            {"id": "first", "function": {"name": "map", "input": {"mappings": []}}}
        ]}
    ]
});

let parsed = parse(&nested, ParseOptions::default());
let issue = &parsed.issues[0];
assert_eq!(issue.code, IssueCode::DuplicateStepId);
assert_eq!(issue.path.to_string(), "tasks[1].tasks[0].id");
assert_eq!(issue.step_kind, Some(StepKind::Task));
assert_eq!(issue.step_id.as_deref(), Some("first"));
```

Notice that `issue.path` is `tasks[1].tasks[0].id`—matching the nested structure
in the submitted JSON (`WorkflowDef::walk` uses the same coordinates) rather
than a flattened task index. Missing or mistyped fields are reported as
`PARSE_FAILED` at their exact path with a message explaining what was expected.

### `Issue` Fields

| Field | Description |
|---|---|
| `code` | The `IssueCode` (`as_str()` returns its stable `SCREAMING_SNAKE_CASE` identifier). |
| `severity` | `Rejected`, `Defect`, or `Advisory`. |
| `path` | The `AuthoredPath` in the submitted JSON (e.g., `tasks[1].tasks[0].id`). |
| `step_kind`, `step_id` | The enclosing step (`task`, `group`, or `setup` under `loop.setup`). |
| `field` | For function input issues, the top-level field name inside `function.input`. |
| `message` | Human-readable explanation of the issue. |

`Issue` serializes to a JSON object with these seven fields, and its `Display`
implementation formats as
`tasks[0].id: every step needs a non-empty id [MISSING_STEP_ID]`.

## Issue Severity

Not every issue prevents a workflow from loading. `Severity` categorizes issues
into three levels, and `IssueCode::severity()` returns the default severity for
each code:

```rust
use dataflow_rs::program::{IssueCode, Severity};

// Advisory: reported for review, and the workflow loads and runs normally.
assert_eq!(IssueCode::UnguardedValidation.severity(), Severity::Advisory);
// Defect: loads, but will likely fail or misbehave when processing messages.
assert_eq!(IssueCode::LikelyTypo.severity(), Severity::Defect);
// Rejected: invalid definition; cannot be loaded or compiled.
assert_eq!(IssueCode::DuplicateStepId.severity(), Severity::Rejected);

assert_eq!(IssueCode::DuplicateStepId.as_str(), "DUPLICATE_STEP_ID");
assert_eq!(Severity::Defect.as_str(), "DEFECT");
```

- **`Rejected`**: The workflow definition is invalid and cannot run (`parse`
  returns `def: None`, or `Compiler::compile_one` returns `Err`).
- **`Defect`**: The workflow compiles and loads, but contains a bug that will
  cause it to misbehave at runtime—such as a misspelled field name
  (`LIKELY_TYPO`), a `var` reading from `payload` (`PAYLOAD_VAR`), or a missing
  external connector (`UNRESOLVED_REFERENCE`). Checking only whether a workflow
  builds will miss defects; always check for `Severity::Defect` when validating
  submitted workflows.
- **`Advisory`**: The workflow loads and runs, but has a pattern worth bringing
  to the author's attention (such as an unguarded `validation` task).

`Severity::Advisory` is the only level that can be safely ignored when gating
workflow activation. Both `IssueCode` and `Severity` are marked
`#[non_exhaustive]`, so always include a wildcard arm when matching on them.

## Unknown Keys and the Compatibility Reader

`ParseOptions::default()` enables the **3.x compatibility reader**. It
translates 3.x policy flags (`continue_on_error`, `halt_on`) and `validate` into
their 4.0 equivalents and emits a `V3_SPELLING` advisory (see
[Control Flow](./control-flow.md#migrating-from-3x)).

When the compatibility reader is enabled, unknown keys on workflow, task, or
group objects are handled as follows:

| Unknown Key Type | Issue Code | Severity | Behavior |
|---|---|---|---|
| Close to a known key at that level (e.g., `conditon`, `on_failrue`) | `LIKELY_TYPO` | `Defect` | Workflow still loads; the key is moved into the node's `ext` object. |
| Any other unknown key | `UNKNOWN_KEY` | `Advisory` | Workflow still loads; the key is moved into the node's `ext` object. |

Unknown keys inside `loop`, `for_each`, `rollout`, or `function` are preserved
in `ext` under `loop.<key>`, `for_each.<key>`, etc., and unknown keys inside a
built-in function's `input` are preserved under `function.input.<key>`.

```rust
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::program::{IssueCode, Severity};
use serde_json::json;

let json = json!({"id": "w", "tasks": [
    {"id": "t", "conditon": true, "colour": "teal", "function": {"name": "map", "input": {"mappings": []}}}
]});

// With the compatibility reader enabled (default), both keys load and are reported in order.
let parsed = parse(&json, ParseOptions::default());
let rows: Vec<(IssueCode, Severity, String)> = parsed
    .issues
    .iter()
    .map(|i| (i.code, i.severity, i.path.to_string()))
    .collect();
assert_eq!(rows, [
    (IssueCode::LikelyTypo, Severity::Defect, "tasks[0].conditon".to_owned()),
    (IssueCode::UnknownKey, Severity::Advisory, "tasks[0].colour".to_owned()),
]);
let def = parsed.def.unwrap();
let dataflow_rs::def::StepDef::Task(task) = &def.tasks()[0] else { unreachable!() };
assert_eq!(task.header().ext()["colour"], json!("teal"));

// With `.compat(false)`, all unknown keys are rejected.
let strict = parse(&json, ParseOptions::default().compat(false));
assert!(strict.def.is_none());
assert!(strict.issues.iter().all(|i| i.severity == Severity::Rejected));
```

Inside a custom host function's `input`, validation is always strict:
`#[derive(FunctionConfig)]` rejects any unknown key (`LIKELY_TYPO` or
`UNKNOWN_KEY` with `Rejected` severity) regardless of the `compat` setting.

## Issue Code Reference

### Workflow and Step Structure

| Code | Severity | Description |
|---|---|---|
| `EMPTY_WORKFLOW_ID` | `Rejected` | Workflow `id` is missing or empty. |
| `NO_TASKS` | `Rejected` | Workflow `tasks` is missing, not an array, or empty. |
| `MISSING_STEP_ID` | `Rejected` | A task or group is missing its `id`. |
| `DUPLICATE_STEP_ID` | `Rejected` | Two steps share the same `id` (tasks, groups, and `loop.setup` steps all share one ID namespace). |
| `EMPTY_GROUP` | `Rejected` | A group's `tasks` field is not a non-empty array. |
| `GROUP_TOO_DEEP` | `Rejected` | Groups are nested deeper than `MAX_GROUP_DEPTH` (8). |
| `MISSING_FUNCTION` | `Rejected` | A task is missing its `function` field. |
| `INVALID_FUNCTION_NAME` | `Rejected` | `function` is not an object with a non-empty `name`. |
| `INVALID_TERMINAL` | `Rejected` | `terminal` is present but is not a boolean. |
| `PARSE_FAILED` | `Rejected` | A required key is missing or has the wrong JSON type. |
| `NULL_CONDITION` | `Advisory` | `condition` is literally `null` (which is falsy, so the step never runs; omit `condition` for an unconditional step). |
| `INVALID_ROLLOUT` | `Defect` | A workflow's `rollout` range can never match (`bucket_end < bucket_start` or `bucket_end > 100`). |
| `MIXED_SPELLING` | `Rejected` | A 3.x policy key (`continue_on_error`, `halt_on`) is combined with a 4.0 policy key on the same node. |
| `EXT_COLLISION` | `Rejected` | An unknown key moved into `ext` collides with a key already present in `ext`. |

### Loops and Fan-Out

| Code | Severity | Description |
|---|---|---|
| `LOOP_INCREMENT_TOO_SMALL` | `Rejected` | `loop.increment < 1` (the counter would never reach `max`). |
| `LOOP_BOUND_EMPTY` | `Rejected` | `loop.max <= loop.init` (zero iterations could run). |
| `LOOP_COUNTER_INVALID` | `Rejected` | `loop.counter` is not a valid non-empty dotted path. |
| `LOOP_SLOT_INVALID` | `Rejected` | `loop.as` or `loop.scratch` is not a valid non-empty dotted path. |
| `LOOP_ITEM_WITHOUT_OVER` | `Rejected` | `loop.as` is set without `loop.over`. |
| `LOOP_SLOT_COLLISION` | `Rejected` | Two of `counter`, `as`, and `scratch` point to the same or overlapping paths. |
| `LOOP_OVER_INVALID` | `Rejected` | `loop.over` is a scalar literal that can never evaluate to an array, or `loop.init < 0` is used alongside `loop.over` (see [Loops](./loops.md)). |
| `INVALID_FOR_EACH` | `Rejected` | A task's `for_each` configuration is invalid, or `for_each` is placed on a built-in function or group (see [Fan-Out](./for-each.md#validation)). |

### Functions and Inputs

| Code | Severity | Description |
|---|---|---|
| `UNKNOWN_FUNCTION` | `Rejected` | No built-in or registered host function matches `function.name` (includes a "did you mean" suggestion when close to a registered name). |
| `INPUT_PARSE` | `Rejected` | Function configuration failed validation (missing field, wrong type, custom check failure, or `Function::prepare` error). |
| `TEMPLATE_COMPILE` | `Rejected` | A JSONLogic expression failed to compile. |
| `INVALID_MAPPING` | `Rejected` | A `map` item violates mapping constraints (see [Map](../built-in-functions/map.md)). |
| `NULL_MAPPING` | `Advisory` | A `map` item's `logic` is a constant `null`, which is a no-op by default (use `"unset": true` to remove a field). |
| `UNGUARDED_VALIDATION` | `Advisory` | A `validation` task's rejection is neither halted on nor checked later in the workflow. |
| `INVALID_PATH` | `Rejected` | A literal context path or `Interpolated` placeholder is not rooted at `data`, `metadata`, or `temp_data`. |
| `PAYLOAD_VAR` | `Defect` | A literal `var` expression reads from `payload` (which is not part of the evaluation context). |
| `UNRESOLVED_REFERENCE` | `Defect` | A field marked as a `Reference` in `FunctionSpec` contains a literal name that the `ResourceOracle` could not resolve. |

### Secrets, Template Keys, and Interpolated Documents

| Code | Severity | Description |
|---|---|---|
| `UNKNOWN_SECRET` | `Rejected` | An expression references a literal secret name not present in the engine's `Secrets` store (see [Secrets](./secrets.md)). |
| `SECRET_IN_MESSAGE_WRITE` | `Rejected` | An expression whose output is recorded by the engine reads a secret (see [Secrets](./secrets.md#preventing-secret-leaks)). |
| `DUPLICATE_TEMPLATE_KEY` | `Rejected` | Two keys in the same template object resolve to the same name after stripping a leading `$`. |
| `ESCAPED_TEMPLATE_KEY` | `Advisory` | A template key uses a leading `$` escape prefix. |
| `OPERATOR_IN_DOCUMENT` | `Advisory` | An `Interpolated` document contains an operator-shaped object (which is kept as literal data rather than evaluated). |
| `SECRET_IN_DOCUMENT` | `Rejected` | An `Interpolated` document contains a `{"secret": …}` node. |

### Unknown Keys and 3.x Spellings

| Code | Severity | Description |
|---|---|---|
| `LIKELY_TYPO` | `Defect` | An unknown key is within a small edit distance of a valid key at that level. |
| `UNKNOWN_KEY` | `Advisory` | An unknown key does not resemble any valid key at that level (moved into `ext`). |
| `V3_SPELLING` | `Advisory` | A 3.x policy key or `validate` function name was rewritten to its 4.0 equivalent. |
| `GROUP_CONTINUE_ON_ERROR` | `Advisory` | A group has `continue_on_error` (moved into `ext`). |
| `INVALID_HALT_ON` | `Rejected` | `halt_on` has a value other than `"never"` or `"failure"`, or is placed on a group. |

## Compiling Against a Registry (`Compiler`)

Structural parsing (`def::parse`) checks the shape of the workflow JSON. The
second stage—compiling against a `Registry`—verifies that every task names a
registered function, every function config compiles, and all expressions pass
secret and path checks.

Create a `Compiler` from a `Registry` (or call `engine.compiler()` on a running
engine) to compile or check workflows individually:

```rust
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::program::{CompileOptions, IssueCode};
use dataflow_rs::Registry;
use serde_json::json;

let compiler = Registry::default().compiler(CompileOptions::default());

let def = parse(
    &json!({"id": "w", "tasks": [{"id": "t", "function": {"name": "fetch"}}]}),
    ParseOptions::default(),
)
.def
.unwrap();

// `compile_one` returns `Err(issues)` when any issue has `Rejected` severity,
// or `Ok(Compiled)` containing the compiled workflow plus any Defects and Advisories.
let issues = compiler.compile_one(&def).unwrap_err();
assert_eq!(issues[0].code, IssueCode::UnknownFunction);
assert_eq!(issues[0].path.to_string(), "tasks[0].function.name");
assert_eq!(issues[0].step_id.as_deref(), Some("t"));

// `check` returns all issues regardless of whether compilation succeeded or failed.
assert_eq!(compiler.check(&def), issues);
```

Issues inside a function's configuration are reported under
`tasks[i].function.input.<field>`, with `issue.field` set to the top-level input
field name.

### Validating Step Fragments (`check_steps`)

If you are building a workflow editor that validates a single branch or group as
the user edits it, use `Compiler::check_steps`. It parses and compiles a JSON
array of steps relative to a given root path (such as `"tasks[2].tasks"` or
`"loop.setup"`), reporting all issue paths under that prefix:

```rust
use dataflow_rs::program::{CompileOptions, IssueCode};
use dataflow_rs::Registry;
use serde_json::json;

let compiler = Registry::default().compiler(CompileOptions::default());
let steps = json!([
    {"id": "t", "function": {"name": "fetch", "input": {}}}
]);
let issues = compiler.check_steps(&steps, "tasks[2].tasks");
assert_eq!(issues[0].code, IssueCode::UnknownFunction);
assert_eq!(issues[0].path.to_string(), "tasks[2].tasks[0].function.name");
```

### Validating External Resource References (`ResourceOracle`)

A custom function's `FunctionSpec` can declare that certain config fields
reference external resources (such as database connectors) using
`Reference::new(field, kind)`. When you supply a `ResourceOracle` via
`CompileOptions::resources`, the compiler validates every literal resource name
against your oracle and reports missing resources as `UNRESOLVED_REFERENCE`
(`Severity::Defect`).

```rust
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::function::{FunctionConfig, FunctionSpec, Reference};
use dataflow_rs::program::{CompileOptions, IssueCode, ResourceOracle, Severity};
use dataflow_rs::{Ctx, Function, Outcome, Registry, Result};
use serde_json::json;
use std::sync::Arc;

#[derive(FunctionConfig)]
struct QueryConfig {
    connector: String,
    sql: String,
}

struct Query;

impl Function for Query {
    type Config = QueryConfig;

    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default().with_references(vec![Reference::new("connector", "connector")])
    }

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &QueryConfig) -> Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

/// Checks whether a connector name exists in the current deployment.
struct Connectors;

impl ResourceOracle for Connectors {
    fn resolve(&self, _kind: &str, name: &str) -> std::result::Result<(), String> {
        if name == "orders" { Ok(()) } else { Err(String::new()) }
    }
}

let mut registry = Registry::default();
registry.register("query", Query);
let compiler = registry.compiler(CompileOptions::default().resources(Arc::new(Connectors)));

let def = parse(
    &json!({"id": "w", "tasks": [{"id": "t", "function": {"name": "query",
        "input": {"connector": "sessions", "sql": "select 1"}}}]}),
    ParseOptions::default(),
)
.def
.unwrap();
let compiled = compiler.compile_one(&def).unwrap();
assert_eq!(compiled.issues[0].code, IssueCode::UnresolvedReference);
assert_eq!(compiled.issues[0].severity, Severity::Defect);
assert_eq!(compiled.issues[0].path.to_string(), "tasks[0].function.input.connector");
```

If `ResourceOracle::resolve` returns `Err(reason)` with a non-empty `reason`
string, that reason is included in the issue message; returning `Err(String::new())`
produces the default message (`` no connector named `sessions` resolves ``).

## Quarantining Invalid Workflows

`Engine::builder().build()` fails if *any* workflow definition is rejected. When
loading many workflow rows from a database, you can compile each workflow
individually, quarantine any rows that fail, and link the valid workflows into a
running `Engine` with `compiler.link(accepted)`:

```rust
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::program::CompileOptions;
use dataflow_rs::run::EngineConfig;
use dataflow_rs::{Engine, Issue, Registry};
use serde_json::{Value, json};

let submitted: Vec<Value> = vec![
    json!({"id": "good", "tasks": [{"id": "t", "function": {"name": "map",
        "input": {"mappings": [{"path": "data.ok", "logic": true}]}}}]}),
    json!({"id": "bad", "tasks": [{"id": "t", "function": {"name": "fetch"}}]}),
];

let compiler = Registry::default().compiler(CompileOptions::default());
let mut accepted = Vec::new();
let mut quarantined: Vec<(usize, Vec<Issue>)> = Vec::new();
for (row, raw) in submitted.iter().enumerate() {
    let parsed = parse(raw, ParseOptions::default());
    let Some(def) = parsed.def else {
        quarantined.push((row, parsed.issues));
        continue;
    };
    match compiler.compile_one(&def) {
        Ok(compiled) => accepted.push(compiled.workflow),
        Err(issues) => quarantined.push((row, issues)),
    }
}

let engine = Engine::new(compiler.link(accepted), EngineConfig::default());
assert_eq!(engine.program().workflows().len(), 1);
assert_eq!(quarantined.len(), 1);
assert_eq!(quarantined[0].0, 1);
```

`compiler.link(accepted)` sorts the compiled workflows by `priority` and builds
an immutable `Program`. To hot-reload workflows on a running engine, run the
same loop using `engine.compiler()` and call `engine.with_program(program)`: the
new `Engine` instance runs the updated program while any in-flight runs on the
previous instance finish undisturbed.

## Recommended Validation Flow for APIs

When validating a workflow submitted to an API endpoint:

1. **Run `def::parse(&json, ParseOptions::default())`** to validate structure
   and field paths. If `parsed.def` is `None` (or if any issue has `Defect`
   severity), reject the request with `400 Bad Request` and return
   `parsed.issues`.
2. **Run `compiler.compile_one(&def)`** against your target `Registry`. If it
   returns `Err(issues)` (or if `compiled.issues` contains any `Defect`), reject
   the request with `400 Bad Request` and return the issues.
3. **Store and activate the workflow**, keeping any `Advisory` issues to display
   as warnings in your UI.
