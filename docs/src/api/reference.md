# API Reference

dataflow-rs 4 is seven public modules and fifteen names re-exported at the
crate root. This page is the map: what each module holds, the one-line
purpose of each type, and the handful of calls a host makes most, with their
signatures. The full rustdoc is on docs.rs at
`https://docs.rs/dataflow-rs/4.0.0/dataflow_rs/<module>/`; the crate is
unpublished today, so those links resolve from the 4.0.0 release.

A definition enters through [`def::parse`](#checking-a-definition), compiles
through a [`program::Compiler`](#program-compiling), runs on a
[`run::Engine`](#run-running) over a `run::Message`, and reports through an
[`observe::Observer`](#observe-observers-and-traces) or a trace. Host
functions implement [`function::Function`](#a-host-function).

## The root

Fifteen names, enough for a host that registers functions and runs messages:

| Name | Lives in | Purpose |
|---|---|---|
| `Engine` | `run` | A compiled program and its config; runs messages |
| `Message` | `run` | A payload, the three-root context, the audit trail and the error records |
| `Run` | `run` | The report of one run: its status, each workflow's record, its trace |
| `RunOptions` | `run` | What one run chooses: channel, bucket, audit, deadline, cancel, layers, observer, trace |
| `Error` | `run` | Why a run, a compile or a call failed; `code()` is the recorded code |
| `Result<T>` | `run` | `std::result::Result<T, Error>` |
| `Issue` | `program` | One finding about a definition: code, severity, authored path, message |
| `WorkflowDef` | `def` | A parsed definition: plain data with raw expressions |
| `Expr<T>` | `expr` | Compiled JSONLogic that evaluates to a `T` |
| `Path` | `expr` | A context path, parsed once: a root plus segments |
| `Function` | `function` | The trait a host function implements |
| `Ctx` | `function` | A function's view of one call: reads, writes, its config |
| `Outcome` | `function` | What a call returns: `Ok`, `Status(u16)`, `Skip` or `Halt` |
| `Registry` | `function` | The functions and layers a program compiles against |
| `Observer` | `observe` | What a run tells a host as it goes |

Beside them, `VERSION` is the crate version (stamped on every message as
`metadata.engine_version`), and the `datalogic_rs` and `datavalue` crates
are re-exported so a host names one version of each.

## `def`: definitions

The authored form. A definition is plain data with raw `Logic`; it enters
only through `parse`, which is also the linter, and serializes back in the
4.0 spelling. [docs.rs](https://docs.rs/dataflow-rs/4.0.0/dataflow_rs/def/)

| Name | Purpose |
|---|---|
| `parse`, `ParseOptions`, `Parsed` | Read one definition; every problem is an `Issue`, and `Parsed::def` is `Some` exactly when none is Rejected. `ParseOptions::compat` (default `true`) runs the v3 reader first |
| `WorkflowDef` | Id, header, priority, condition, steps, `on_failure`, channel, status, version, rollout, loop, tags, timestamps; `walk()` visits every step with its authored path |
| `StepDef` | One element of a step list: `Task` or `Group`. A closed enum |
| `TaskDef` | One function call with its `on_failure`, `on_rejection`, `terminal` and `for_each` |
| `GroupDef` | A span of steps sharing one condition, evaluated once on entry |
| `NodeHeader` | `name`, `description` and `ext`, the keys every node shares; `ext` holds host data verbatim |
| `FunctionRef` | The registered name and the raw `input` |
| `Logic` | Raw JSONLogic as authored; only a compiler turns it into an `Expr` |
| `LoopDef`, `ForEachDef` | A workflow's bounded loop; a task's fan-out |
| `FailurePolicy`, `RejectionPolicy`, `WorkflowFailurePolicy` | `fail` / `halt` / `continue` for a 5xx or `Err`; `continue` / `halt` for a 4xx; `fail` / `continue` for the workflow |
| `Status` | `active`, `paused` or `archived`; only `active` workflows run |
| `Rollout`, `RolloutError` | A workflow's slice of its channel's traffic |
| `defaults` | The values `parse` reads for an absent key (`PRIORITY`, `VERSION`, `CHANNEL`, `INIT`, `INCREMENT`, `MAX_CONCURRENCY`) |
| `MAX_GROUP_DEPTH`, `BUILTIN_FUNCTIONS` | The nesting limit (8); the eight reserved names |

## `expr`: expressions

The JSONLogic surface a definition is written in, and the field types a
function's config is made of. [docs.rs](https://docs.rs/dataflow-rs/4.0.0/dataflow_rs/expr/)

| Name | Purpose |
|---|---|
| `ExprEngine`, `ExprEngineBuilder` | Compiles and evaluates expressions; the builder takes custom operators, `Secrets` and (with the `budget` feature) an ops ceiling |
| `Expr<T>`, `FromValue`, `Evaluated`, `ResolveExpr` | Compiled JSONLogic resolving to a `T` (`OwnedDataValue`, `String`, `u64`, `i64`, `f64`, `bool` or `Path`); a constant folds at compile |
| `Path`, `Root`, `MAX_ARRAY_INDEX` | A context path; `get`, `set` and `remove` over the context by v3's segment rules; a bare numeric segment is an array index up to 1,048,575 |
| `Output` | A write destination: an `Expr<Path>` a function emits through |
| `Interpolated`, `Rendered` | A document of `var` holes in literal data, and one filled for a call |
| `Secrets` | Values an expression reads through `{"secret": "name"}` and the engine never records |
| `OperatorInfo`, `Family` | One operator an engine evaluates, and the feature family it belongs to |
| `ConfigField`, `Resolver`, `CompileCx`, `Visit`, `ExprVisitor`, `FieldSpec`, `FieldKind`, `ValueKind` | The machinery behind `#[derive(FunctionConfig)]`: how a field compiles, resolves and is walked |
| `EvalRoot` | What an evaluation runs against (`&OwnedDataValue` or `&serde_json::Value`) |
| `escaped_keys`, `secret_refs` | Walks over raw JSON: every `$`-escaped template key; every secret reference |

## `function`: host functions

The typed units of work a task runs. The eight built-ins are here too, but
run inline; a host function is one `Function` with one derived config.
[docs.rs](https://docs.rs/dataflow-rs/4.0.0/dataflow_rs/function/)

| Name | Purpose |
|---|---|
| `Function` | `type Config`, `spec()`, `prepare()` at build, and `call()` per task run |
| `FunctionConfig` (trait and derive) | A function's `input`, compiled once at build; `#[config(default, default_json, rename, serde, records)]` and `#[output(default, primary)]` on fields |
| `FunctionSpec`, `RetrySafety`, `Reference` | What a function says about itself, for tooling: description, category, retry safety, the config fields that name resources |
| `Ctx`, `CallInfo` | The call's view: `get`, `set`, `remove`, `eval`, `render`, `emit`, `input`, `resolve`, `record_error`, `deadline`, `cancel`; `info()` is the call's static facts |
| `Outcome` | `Ok` (200), `Status(u16)`, `Skip` (no entry) or `Halt` (299) |
| `Prepare` | What `prepare` is told: the name, workflow id, task id and spec |
| `CallLayer`, `Next`, `BoxFuture` | Around-call middleware for host calls, at registry or run scope |
| `layers::Answer` | A ready-made layer answering calls from a closure, for a dry run |
| `Registry`, `FunctionInfo` | Functions and layers plus the `ExprEngine` they compile against; `catalogue()` lists them |
| `Templated`, `Rendered` | A plugin's config whose fields a manifest declares at runtime; a filled document |
| `testing` | `call(f, input)` runs one call through the real compile, overlay and layers; `config` compiles a config alone |

## `program`: compiling

A `Compiler` comes from a `Registry` and compiles one workflow at a time, so
a host owns quarantine: a Rejected issue rejects only its own workflow.
[docs.rs](https://docs.rs/dataflow-rs/4.0.0/dataflow_rs/program/)

| Name | Purpose |
|---|---|
| `Compiler`, `CompileOptions` | `check(&def)` returns every issue; `compile_one(&def)` returns a `Compiled` or every issue; `link(workflows)` makes a `Program` |
| `Compiled`, `CompiledWorkflow` | An accepted workflow with its Defects and Advisories; `fingerprint()` |
| `Program`, `WorkflowInfo`, `TaskInfo` | Linked workflows, immutable, with dense indices; what the program lists about each |
| `ProgramId`, `WorkflowIx`, `TaskIx` | The identity of a build and the positions within it |
| `Issue`, `IssueCode`, `Severity` | A finding; its code (v3's 34 and v4's 8, `as_str()` is the stable spelling); `Rejected`, `Defect` or `Advisory` |
| `AuthoredPath`, `PathSeg`, `StepKind` | Where in the authored JSON, as v3 spelled it (`tasks[1].tasks[0].id`); the kind of step it sits in |
| `ResourceOracle` | A host's answer to "does this connector exist", checked against `Reference` fields |
| `Fingerprint` | The operators and data keys a workflow's expressions use; `Display` and `FromStr` |
| `ParseIssueCodeError`, `ParsePathError`, `ParseFingerprintError` | The `FromStr` errors |

## `run`: running

Executing a program over a message. [docs.rs](https://docs.rs/dataflow-rs/4.0.0/dataflow_rs/run/)

| Name | Purpose |
|---|---|
| `Engine`, `EngineBuilder`, `EngineConfig` | `Engine::builder()` for a host that does not quarantine; `Engine::new(program, config)` for one that does; `compiler()` and `with_program()` reload |
| `ErrorContext` | Where failure records are mirrored into the context, and how many are kept |
| `RunOptions` | Per-run choices; `Default` is every active workflow, changes captured, full audit, no limits |
| `Run`, `RunStatus` | `status()`, `workflows()`, `matched()`, `partial()`, `retryable()`, `trace()`, `take_trace()`, `into_result()`; `Completed`, `CompletedWithErrors`, `Stopped(Error)`, `TimedOut`, `Cancelled` |
| `WorkflowRecord`, `Selection`, `WorkflowExit` | One workflow's part in a run: whether it was selected, how many sweeps, how it ended |
| `Message` | `new`, `from_parts`, `from_json`; `payload()`, `context()`, `context_mut()`, `audit()`, `errors()`; serializes as `{id, payload, context, audit_trail, errors}` |
| `Context` | The three roots, always `data`, `metadata` and `temp_data` in that order; `get`, `set`, `remove` by `Path` |
| `AuditEntry`, `Change`, `AuditMode` | One task's audit record; one recorded write or removal; `Full`, `Last(n)` or `Off` |
| `ErrorRecord` | One recorded error, on v3's wire shape |
| `Error`, `ErrorKind`, `Result` | The error type and its class; `Error::service(code, message)` is a host's own |
| `CancelToken` | A cancellation flag; a `child()` is cancelled with its parent |
| `Instant` | The clock a deadline is read on: `std::time::Instant` natively, a browser clock on wasm32 |

## `observe`: observers and traces

What a run reports about itself, inline to an `Observer` and, once it ends,
on its `Run`. [docs.rs](https://docs.rs/dataflow-rs/4.0.0/dataflow_rs/observe/)

| Name | Purpose |
|---|---|
| `Observer`, `Interest` | `interest()` (read once per run) and `on(&Event)` (inline, synchronous); `WORKFLOWS`, `TASKS`, `TIMINGS` decide which events arrive and whether a clock is read |
| `Event` | `RunStarted`, `WorkflowStarted`, `TaskFinished`, `WorkflowFinished`, `RunFinished`; every variant is `#[non_exhaustive]`, so match with `..` |
| `TraceOptions` | `timings`, `changes`, `redact`, `max_bytes` |
| `Trace`, `TraceStep`, `StepResult` | The initial context plus one step per call, skip, failure and engine write; `contexts()` replays them; `executed`, `skipped` or `failed` |

## `compat`: 3.x definitions

[docs.rs](https://docs.rs/dataflow-rs/4.0.0/dataflow_rs/compat/)

| Name | Purpose |
|---|---|
| `migrate_v3` | Parses with the compat reader on and writes the definition in the 4.0 spelling; `Err` holds every issue when any is Rejected |
| `Migrated`, `Rewrite` | The result: the JSON, each rewrite (its path, the v3 and v4 spellings) and every issue |

See [Migrating from 3.x](../migration/from-3.md) for what the reader rewrites.

## The calls a host makes most

### Building an engine

```rust
use dataflow_rs::Engine;
use dataflow_rs::run::EngineBuilder;
# use serde_json::json;

let builder: EngineBuilder = Engine::builder();
let engine: Engine = builder
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.greeting", "logic": "hello"}]}}}
    ]}))
    .build()
    .expect("Err holds every rejected definition's issues");
# assert_eq!(engine.program().workflows().len(), 1);
```

`EngineBuilder` chains `register(name, function)`, `layer(l)`,
`registry(r)`, `options(CompileOptions)`, `config(EngineConfig)`,
`workflow(WorkflowDef)` and `workflow_json(&Value)`, then
`build() -> Result<Engine, Vec<Issue>>`. A host that quarantines compiles
each workflow on its own instead:

```rust
use dataflow_rs::program::{CompileOptions, Compiler, Program};
use dataflow_rs::run::EngineConfig;
use dataflow_rs::{Engine, Registry};
# use dataflow_rs::def::{ParseOptions, parse};
# use serde_json::json;
# let def = parse(&json!({"id": "w", "tasks": [
#     {"id": "t", "function": {"name": "log", "input": {"message": "hi"}}}
# ]}), ParseOptions::default()).def.unwrap();

let compiler: Compiler = Registry::default().compiler(CompileOptions::default());
let compiled = compiler.compile_one(&def).expect("Err holds every issue");
let program: Program = compiler.link([compiled.workflow]);
let engine = Engine::new(program, EngineConfig::default());
# assert_eq!(engine.program().workflows()[0].id, "w");
```

A reload is `engine.compiler()`, then `link`, then
`engine.with_program(program)`; the old engine stays valid for runs in
flight.

### Running a message

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::{Engine, Message, Run, RunOptions};
# use serde_json::json;
# let engine = Engine::builder()
#     .workflow_json(&json!({"id": "w", "tasks": [
#         {"id": "t", "function": {"name": "map", "input": {
#             "mappings": [{"path": "data.greeting", "logic": "hello"}]}}}
#     ]}))
#     .build().unwrap();

let mut message = Message::from_json(&json!({"name": "Ada"}));

// With the default options.
let run: Run = engine.run(&mut message).await;

// With options: a channel, a routing bucket, a deadline, a trace, ...
let opts = RunOptions::default().channel("default");
let run: Run = engine.run_with(&mut message, &opts).await;
# assert_eq!(run.matched(), 1);
# });
```

`run(&self, &mut Message)` and `run_with(&self, &mut Message, &RunOptions)`
are both `async` and return a `Run` on every outcome. A deadline is raced on a Tokio timer, so a run with one needs a
Tokio runtime with its time driver.

### Reading the result

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::run::RunStatus;
# use dataflow_rs::{Engine, Message};
# use serde_json::json;
# let engine = Engine::builder()
#     .workflow_json(&json!({"id": "w", "tasks": [
#         {"id": "t", "function": {"name": "map", "input": {
#             "mappings": [{"path": "data.greeting", "logic": "hello"}]}}}
#     ]}))
#     .build().unwrap();
# let mut message = Message::from_json(&json!({}));
# let run = engine.run(&mut message).await;

match run.status() {
    RunStatus::Completed => {}
    RunStatus::CompletedWithErrors => { /* read message.errors() */ }
    RunStatus::Stopped(error) => eprintln!("{}: {}", error.code(), error.message()),
    RunStatus::TimedOut | RunStatus::Cancelled => {}
    _ => {}
}
for workflow in run.workflows() {
    println!("{} {:?} {:?}", workflow.id, workflow.selection, workflow.exit);
}
# assert!(matches!(run.status(), RunStatus::Completed));
# });
```

`Run::status(&self) -> &RunStatus`. `into_result()` turns a stopped, timed
out or cancelled run into `Err`, dropping the report and its trace; take the
trace first with `take_trace()` when it is wanted on every status.

### Building a message

```rust
use dataflow_rs::Message;
use dataflow_rs::datavalue::OwnedDataValue;
use serde_json::json;
use std::sync::Arc;

// From JSON: the payload converted.
let m = Message::from_json(&json!({"name": "Ada"}));

// From an owned value, or an `Arc` shared across messages.
let payload = Arc::new(OwnedDataValue::from(&json!({"name": "Ada"})));
let m = Message::new(Arc::clone(&payload));

// Seeded `data` and `metadata`; a seed that is not an object is ignored.
let m = Message::from_parts(
    payload,
    OwnedDataValue::from(&json!({"stage": "intake"})),
    OwnedDataValue::Null,
);
# assert_eq!(m.context().data(), &OwnedDataValue::from(&json!({"stage": "intake"})));
```

`Message::new(payload: impl Into<Arc<OwnedDataValue>>)` and
`Message::from_parts(payload, data: OwnedDataValue, metadata: OwnedDataValue)`.
The id is a UUID v7; `with_id` replaces it. The payload is not part of the
evaluation context: a `parse_json` task copies it into `data`.

### A host function

```rust
use dataflow_rs::expr::Output;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::{Ctx, Expr, Function, Outcome, Result};
# use serde_json::json;

#[derive(FunctionConfig)]
struct FetchConfig {
    url: Expr<String>,
    #[output(default = "data.response")]
    output: Output,
}

struct Fetch;

impl Function for Fetch {
    type Config = FetchConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &FetchConfig) -> Result<Outcome> {
        let url = ctx.eval(&cfg.url)?;
        let response = OwnedDataValue::from(&json!({"url": &*url, "status": 200}));
        ctx.emit(&cfg.output, response)?;
        Ok(Outcome::Ok)
    }
}
# let _ = Fetch;
```

`fn call(&self, ctx: &mut Ctx<'_>, cfg: &Self::Config) -> impl Future<Output = Result<Outcome>> + Send`,
so `async fn` works. The config compiles once at build, and a malformed one
is an `INPUT_PARSE` issue before any message runs. A call's writes go to an
overlay and reach the message only when the call returns `Ok`; an `Err`
drops them and keeps the records the call added.
`function::testing::call(f, input)` runs one call outside any workflow.

### Checking a definition

```rust
use dataflow_rs::def::{ParseOptions, Parsed, parse};
use dataflow_rs::program::{CompileOptions, Issue, Severity};
use dataflow_rs::Registry;
# use serde_json::json;

let json = json!({"id": "w", "tasks": [
    {"id": "t", "function": {"name": "map", "input": {
        "mappings": [{"path": "data.x", "logic": {"var": "payload.x"}}]}}}
]});

// Structure: never fails, `def` is `Some` when nothing is Rejected.
let parsed: Parsed = parse(&json, ParseOptions::default());
let def = parsed.def.expect("the structure is fine");

// Meaning: functions, expressions, paths, secrets.
let compiler = Registry::default().compiler(CompileOptions::default());
let issues: Vec<Issue> = compiler.check(&def);
let rejected = issues.iter().filter(|i| i.severity == Severity::Rejected).count();
# assert_eq!(rejected, 0);
# assert_eq!(issues[0].code.as_str(), "PAYLOAD_VAR");
# assert_eq!(issues[0].path.to_string(), "tasks[0].function.input.mappings[0].logic");
```

`def::parse(json: &Value, opts: ParseOptions) -> Parsed` and
`Compiler::check(&self, def: &WorkflowDef) -> Vec<Issue>`. Every issue
carries an `AuthoredPath` the author typed, such as
`tasks[0].function.input.mappings[0].logic`. See
[Authoring-Time Validation](../advanced/authoring-validation.md) for the
codes.

## Error codes

`Error::code()` is what an `ErrorRecord` on `message.errors()` carries, what
a `Run`'s `error` writes, and what an `Err` from a host function records:

| `ErrorKind` | Code | When |
|---|---|---|
| `Eval` | `LOGIC_ERROR` | An expression failed to evaluate |
| `Budget` | `BUDGET_EXCEEDED` | The operation ceiling was exceeded (`budget` feature) |
| `FunctionNotFound` | `FUNCTION_NOT_FOUND` | No function is registered under the name |
| `Parse` | `DESERIALIZATION_ERROR` | A document or payload did not parse |
| `TimedOut` | `TIMEOUT_ERROR` | A deadline passed; retryable by default |
| `Cancelled` | `CANCELLED` | The run's token was cancelled |
| `Config` | `CONFIG_ERROR` | A configuration is invalid |
| `Compile` | `COMPILATION_ERROR` | An expression or definition did not compile |
| `Service { code }` | the code, verbatim | A host function's own, from `Error::service(code, message)`; an empty code records `TASK_ERROR` |
| `Service { code }` | `VALIDATION_ERROR` | `parse_xml`, `publish_json` or `publish_xml` found the source not what it needs: missing, not a string, or not a document |

`Error::service` is the one public constructor: a host names its own code,
such as `HTTP_ERROR`, and the engine keeps it. `is_retryable()` is `false` unless
`with_retryable(true)` says otherwise, except for a timeout.

Two codes are records only, never an `Error`, and `validation` records a
third without an `Error` of its own:

| Code | Recorded when |
|---|---|
| `TASK_STATUS_ERROR` | A call returned `Status(5xx)`; the stop error, when the policy stops the run, is `TASK_ERROR` |
| `EVALUATION_ERROR` | A `validation` rule failed to evaluate |
| `VALIDATION_ERROR` | A `validation` rule evaluated to anything but `true`: one record per failed rule, status `400`, no `Error` |

A call that returns `Err(e)` records `e.code()` with status 500. There is no
`WORKFLOW_ERROR` wrapper: the task's record carries the workflow id.

## Features

None is on by default.

| Feature | What it turns on |
|---|---|
| `datetime`, `ext-string`, `ext-array`, `ext-math`, `ext-control`, `ext-object`, `error-handling` | The seven `datalogic-rs` operator families. Not a no-op: an operator whose family is off is literal data, so `{"length": ...}` starts evaluating once `ext-string` is on, and `{"$length": ...}` pins the literal. `datetime` also changes `==` and the ordering operators on date-shaped strings |
| `all-operators` | The seven families |
| `tensor` | The `Tensor` value and its operators. Not in `all-operators`: `shape`, `full`, `cast` and `concat` are ordinary JSON keys |
| `budget` | A ceiling on one evaluation's operations, set by `ExprEngineBuilder::ops_budget` |
| `wasm-web` | The browser's clock and randomness, for `wasm32-unknown-unknown` |
| `ts` | Derives `ts_rs::TS` on the wire shapes, from which `bindings/dataflow.d.ts` is rendered. Outside semver |

```toml
[dependencies]
dataflow-rs = { version = "4.0", features = ["all-operators"] }
```
