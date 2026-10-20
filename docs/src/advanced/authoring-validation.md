# Authoring-Time Validation

A service that *stores* definitions (accepting them from an API, holding
them in a database, building one engine over many rows) wants to know what
is wrong with a definition when it is submitted, not when the engine is
next built. Two stages answer that, and both report every problem at the
path the author typed:

1. **`def::parse`** reads the JSON into a `WorkflowDef` and is the linter:
   every structural rule, every unknown key, every 3.x spelling.
2. **`Compiler`** compiles a parsed definition against a registry: does
   every task name a function this engine has, does every config compile,
   does every expression pass the checks.

`Engine::builder().build()` runs both and fails if any definition is
rejected. That is right for a host with a fixed set of workflows; a host
that quarantines bad rows and runs the rest uses the two stages directly.

## Parsing a definition

If the caller already owns the JSON `Value`, `def::parse_owned(value, options)`
rewrites compatibility spellings in place instead of copying the whole
document. It returns the same definition and ordered issues as `parse`.
`EngineBuilder::workflow_json_owned(value)` uses this path too; the borrowed
`parse` and `workflow_json` methods remain available.

`def::parse` reads one definition into a `WorkflowDef` and lints its structure,
unknown keys, and 3.x spellings. `parse` always returns a `Parsed` struct containing
all discovered `issues`, and sets `def` to `Some` when no issue has `Rejected`
severity:

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

// Both problems, not just the first.
let codes: Vec<IssueCode> = parsed.issues.iter().map(|i| i.code).collect();
assert_eq!(codes, [IssueCode::EmptyWorkflowId, IssueCode::DuplicateStepId]);
```

Each issue carries the coordinate the author typed, so a `400` can point at
the exact field, and the step it sits in:

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

The path is `tasks[1].tasks[0].id`, where the author wrote it. The
definition keeps the authored tree (`WorkflowDef::walk` yields it in the
same coordinates), so nothing is ever reported at a flattened index.

A missing or ill-typed key is `PARSE_FAILED` at its own path, carrying what
was expected, so a host needs no round-trip check of its own as a safety
net.

### What an issue carries

| Field | Holds |
|---|---|
| `code` | An `IssueCode`; `as_str()` is its stable `SCREAMING_SNAKE` form |
| `severity` | `Rejected`, `Defect` or `Advisory`: the code's own, unless overridden |
| `path` | An `AuthoredPath` into the submitted JSON, `tasks[1].tasks[0].id` |
| `step_kind`, `step_id` | The step it sits in: a `task`, a `group`, or a `setup` step under `loop.setup` |
| `field` | Inside a config, the first key under `function.input` |
| `message` | What is wrong, in words |

An `Issue` serializes as an object of those seven fields, the code and
severity as their strings and the path as it displays, and `Display`
renders `tasks[0].id: every step needs a non-empty id [MISSING_STEP_ID]`.

## Severity

Not every issue is a refusal. `Severity` says when an issue bites, and
`IssueCode::severity()` gives each code's default:

```rust
use dataflow_rs::program::{IssueCode, Severity};

// Reported, and the workflow loads and runs.
assert_eq!(IssueCode::UnguardedValidation.severity(), Severity::Advisory);
// Loads, then does not do what it says.
assert_eq!(IssueCode::LikelyTypo.severity(), Severity::Defect);
// Never loads.
assert_eq!(IssueCode::DuplicateStepId.severity(), Severity::Rejected);

assert_eq!(IssueCode::DuplicateStepId.as_str(), "DUPLICATE_STEP_ID");
assert_eq!(Severity::Defect.as_str(), "DEFECT");
```

- **Rejected**: the definition never runs. `parse` gives no `def`, or the
  compiler gives no workflow.
- **Defect**: it loads, then fails or misbehaves on messages. A near miss of
  a known key (`LIKELY_TYPO`), a `var` reading the payload (`PAYLOAD_VAR`),
  a connector no deployment declares (`UNRESOLVED_REFERENCE`). A host
  screening on "would this build" alone waves these through.
- **Advisory**: it loads and runs; the shape deserves an author's attention.

`Severity::Advisory` is the only class a host may safely ignore, so
screening a definition before activating it is one pass over its issues.
Both `IssueCode` and `Severity` are `#[non_exhaustive]`, so match the codes
you care about and let the rest fall through.

## Unknown keys and the compat reader

`ParseOptions::default()` has the **compat reader** on. It reads 3.x's
spellings of the policies and of the `validation` function's name and
rewrites each into the 4.0 spelling, reporting a `V3_SPELLING` advisory at
the 3.x key; see [Control Flow](./control-flow.md#coming-from-3x). It also
decides what an unknown key becomes:

| Key | Code | Severity | What happens |
|---|---|---|---|
| A near miss of a known key at its level (`conditon`, `on_failrue`) | `LIKELY_TYPO` | Defect | Loads; the key is kept in the node's `ext` |
| Any other unknown key | `UNKNOWN_KEY` | Advisory | Loads; the key is kept in the node's `ext` |

A key under `loop`, `for_each`, `rollout` or `function` is kept as
`loop.<key>` and so on, and a key a built-in's input does not know as
`function.input.<key>`. `ext` is the node's slot for host and UI data: the
authored `ext` object first, then what moved there.

```rust
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::program::{IssueCode, Severity};
use serde_json::json;

let json = json!({"id": "w", "tasks": [
    {"id": "t", "conditon": true, "colour": "teal", "function": {"name": "map", "input": {"mappings": []}}}
]});

// With the reader on, both keys load, and each is reported as what it is,
// in the order they were written.
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

// With it off, every unknown key is refused, and nothing moves.
let strict = parse(&json, ParseOptions::default().compat(false));
assert!(strict.def.is_none());
assert!(strict.issues.iter().all(|i| i.severity == Severity::Rejected));
```

Inside a host function's `input` the rule is stricter: a derived config
refuses an unknown key outright (`LIKELY_TYPO` or `UNKNOWN_KEY`, Rejected),
since the function could not honour it.

## Issue codes

### Structure

| Code | Severity | Means |
|---|---|---|
| `EMPTY_WORKFLOW_ID` | Rejected | `id` is missing or empty |
| `NO_TASKS` | Rejected | `tasks` missing, not an array, or empty |
| `MISSING_STEP_ID` | Rejected | A task or group carries no `id` |
| `DUPLICATE_STEP_ID` | Rejected | Two steps share an id; groups share the task namespace, and `loop.setup` shares it with `tasks` |
| `EMPTY_GROUP` | Rejected | A group's `tasks` is not a non-empty array |
| `GROUP_TOO_DEEP` | Rejected | Groups nested past `MAX_GROUP_DEPTH` (8) |
| `MISSING_FUNCTION` | Rejected | A task carries no `function` |
| `INVALID_FUNCTION_NAME` | Rejected | `function` is not an object with a non-empty `name` |
| `INVALID_TERMINAL` | Rejected | `terminal` is present but not a boolean |
| `PARSE_FAILED` | Rejected | A key is missing or of the wrong type; the message says what was expected |
| `NULL_CONDITION` | Advisory | A literal `null` `condition`: kept as the falsy expression `null`, so the step never runs; omit the key for no condition |
| `INVALID_ROLLOUT` | Defect | A workflow's `rollout` range can never match (`bucket_end < bucket_start` or `bucket_end > 100`), so the workflow is never selected |
| `MIXED_SPELLING` | Rejected | A 3.x policy key (`continue_on_error`, `halt_on`) beside a 4.0 one on one node |
| `EXT_COLLISION` | Rejected | A key kept in `ext` under a name `ext` already holds |

### Loops and fan-out

| Code | Severity | Means |
|---|---|---|
| `LOOP_INCREMENT_TOO_SMALL` | Rejected | `increment < 1`: the counter would never reach `max` |
| `LOOP_BOUND_EMPTY` | Rejected | `max <= init`: no sweep could ever run |
| `LOOP_COUNTER_INVALID` | Rejected | `counter` is not a non-empty dotted path |
| `LOOP_SLOT_INVALID` | Rejected | `as` or `scratch` is not a non-empty dotted path |
| `LOOP_ITEM_WITHOUT_OVER` | Rejected | `as` is set but there is no `over` to take elements from |
| `LOOP_SLOT_COLLISION` | Rejected | Two of `counter` / `as` / `scratch` name the same path, or one lies inside the other |
| `LOOP_OVER_INVALID` | Rejected | `over` is a scalar literal that can never be an array, or `init < 0` alongside `over`; see [Loops](./loops.md) |
| `INVALID_FOR_EACH` | Rejected | A task's `for_each` breaks one of its rules, or sits on a built-in or a group (reported at the offending key); see [Fan-Out](./for-each.md#validation) |

### Functions and their inputs

| Code | Severity | Means |
|---|---|---|
| `UNKNOWN_FUNCTION` | Rejected | No function is registered under the name, and it is not a built-in; the message names the nearest registered name when one is close |
| `INPUT_PARSE` | Rejected | A config refused its input: a missing field, a wrong type, a rule between fields, or a function's own `prepare` |
| `TEMPLATE_COMPILE` | Rejected | An expression does not compile |
| `INVALID_MAPPING` | Rejected | A `map` mapping breaks a rule between its keys; see [Map](../built-in-functions/map.md) |
| `NULL_MAPPING` | Advisory | A `map` mapping whose `logic` always evaluates to `null`, so it never writes; use `"unset": true` to remove a path |
| `UNGUARDED_VALIDATION` | Advisory | A `validation` whose rejection nothing acts on; reported at its `on_rejection` |
| `INVALID_PATH` | Rejected | A literal context path not rooted at `data`, `metadata` or `temp_data`, or a document hole whose path is not one |
| `PAYLOAD_VAR` | Defect | A literal `var` rooted at `payload`, which is not in the evaluation context |
| `UNRESOLVED_REFERENCE` | Defect | A field a function's spec names as a `Reference` holds a literal the host's `ResourceOracle` does not resolve |

### Secrets, template keys and documents

| Code | Severity | Means |
|---|---|---|
| `UNKNOWN_SECRET` | Rejected | An expression names a secret the engine does not declare; see [Secrets](./secrets.md) |
| `SECRET_IN_MESSAGE_WRITE` | Rejected | An expression whose result the engine records reads a secret; see [Secrets](./secrets.md#what-a-secret-may-not-do) |
| `DUPLICATE_TEMPLATE_KEY` | Rejected | Two keys in one template object collapse to the same name once the `$` escape is stripped |
| `ESCAPED_TEMPLATE_KEY` | Advisory | A `$`-prefixed template key, listed so the audit is mechanical |
| `OPERATOR_IN_DOCUMENT` | Advisory | An operator-shaped node inside an `Interpolated` document, which stays data |
| `SECRET_IN_DOCUMENT` | Rejected | A `{"secret": …}` node inside an `Interpolated` document, which would never be substituted |

### Keys and spellings

| Code | Severity | Means |
|---|---|---|
| `LIKELY_TYPO` | Defect | An unknown key within edit distance of a known key at its level |
| `UNKNOWN_KEY` | Advisory | An unknown key that resembles no known key; kept in `ext` |

### Codes for 3.x spellings

Only the compat reader reports these, for a definition written in 3.x's
spelling:

| Code | Severity | Means |
|---|---|---|
| `V3_SPELLING` | Advisory | A 3.x key was read as its 4.0 policy, or `validate` as `validation` |
| `GROUP_CONTINUE_ON_ERROR` | Advisory | A group carries `continue_on_error`, which 3.x never honoured either; kept in the group's `ext` |
| `INVALID_HALT_ON` | Rejected | A `halt_on` that is not `"never"` or `"failure"`, or one on a group |

See [Control Flow](./control-flow.md#coming-from-3x) for the rewrite table.

## Compiling against the registry

Shape is half the question. The other half needs the registry: will every
task name a function this engine can run, with an input its config
compiles? A `Compiler` comes from a `Registry`, or from a running engine
(`engine.compiler()`, with the program's registry and options), and
compiles one workflow at a time:

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

// `compile_one` is `Err` with every issue when any is Rejected, and `Ok`
// with the workflow and its Defects and Advisories otherwise.
let issues = compiler.compile_one(&def).unwrap_err();
assert_eq!(issues[0].code, IssueCode::UnknownFunction);
assert_eq!(issues[0].path.to_string(), "tasks[0].function.name");
assert_eq!(issues[0].step_id.as_deref(), Some("t"));

// `check` is every issue, whether or not the workflow is rejected.
assert_eq!(compiler.check(&def), issues);
```

A config's issues sit below the task's `function.input`, each with the
`field` it is in: `tasks[0].function.input.timeout_ms`, field `timeout_ms`.
The compile also runs the expression checks over every expression:
secrets, template keys and `payload` reads.

### Fragments

An editor that checks one branch of a workflow as the author types asks for
the issues of a fragment: a list of steps as it would sit at a path such as
`tasks[2].tasks` or `loop.setup`. `check_steps` parses it as `parse` reads
a workflow's steps and compiles it, every path under the root given:

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

A fragment under `loop.setup` makes setup steps, and each `tasks` key in
the root past the first counts one group of depth, so a group too deep is
reported as it would be in the whole.

### Resources a config names

A host function's `FunctionSpec` can name the config fields that refer to
another definition, a connector say, with `Reference::new(field, kind)`.
Given a `ResourceOracle` through `CompileOptions::resources`, the compiler
checks each one whose value is a literal string, and reports one the oracle
does not resolve as `UNRESOLVED_REFERENCE`, a defect: the workflow would
load and then fail every call.

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

/// The deployment's connectors.
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

An `Err(why)` with a non-empty `why` leads the issue's message; an empty one
gives the plain `` no connector named `sessions` resolves ``.

## Quarantine: run what compiles

`Engine::builder().build()` fails when any definition is rejected, with
every rejected definition's issues, parse issues before compile issues, in
input order. A host that holds many rows and wants the good ones running
compiles each on its own and links the ones that compile:

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

`link` sorts the workflows by `priority` and never fails. A reload is the
same loop over the new rows with `engine.compiler()`, then
`engine.with_program(program)`: the engine returned runs the new program,
and the old one, with any run in flight, keeps the old.

One function's config can be checked alone, with no definition around it:
`function::testing::config::<C>(input)` compiles it and returns every issue
at its path below the input. See
[Testing a function](./custom-functions.md#testing-a-function).

## Putting it together

A submission endpoint checks in this order, stopping at the first stage that
reports anything Rejected:

1. **`def::parse(&json, ParseOptions::default())`**: shape, with field
   paths. Reject with a `400` listing every issue; show the Defects and
   Advisories to the author either way.
2. **`compiler.compile_one(&def)`** against the engine the definition will
   run on: will this engine run it? Reject with a `400` naming the steps and
   what each needs.

Only then store and activate the definition, and keep the compiled
workflow for the next `link`. A host that lets `Engine::builder().build()`
decide instead finds out at reload, when one bad row takes every workflow
in the build down with it.
