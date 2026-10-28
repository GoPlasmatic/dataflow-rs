# API Reference

Dataflow-rs 4.0 is organized into seven public modules with sixteen core types
re-exported at the crate root. This page provides an overview of each module,
the purpose of every public type, and common usage examples. Full API
documentation is available on
[docs.rs](https://docs.rs/dataflow-rs/4.0.0/dataflow_rs/).

At a high level:
1. Workflow JSON is parsed and linted by [`def::parse`](#validating-a-workflow-definition).
2. Parsed definitions are compiled against a `Registry` by [`program::Compiler`](#program-compilation-and-validation).
3. Messages are executed by [`run::Engine`](#run-executing-messages) over a `run::Message`.
4. Execution is observed via [`observe::Observer` or `observe::Trace`](#observe-observers-and-traces).
5. Custom functions implement [`function::Function`](#implementing-a-custom-host-function).

## Crate Root Re-exports

The crate root re-exports the sixteen most commonly used types and traits:

| Name | Module | Description |
|---|---|---|
| `Engine` | `run` | Holds a compiled `Program` and `EngineConfig`; executes messages. |
| `Message` | `run` | Carries the payload, three-root context (`data`, `metadata`, `temp_data`), audit trail, and error records. |
| `Run` | `run` | Execution report for a single run: overall status, per-workflow records, and optional trace. |
| `RunOptions` | `run` | Per-run configuration: channel, routing bucket, audit mode, deadline, cancellation token, layers, observer, and trace options. |
| `Error`, `ErrorKind` | root | Error type and classification for runs, compilation, and host functions (`Error::service(code, message)` constructs custom service errors). |
| `Result<T>` | root | Type alias for `std::result::Result<T, Error>`. |
| `Issue` | root | A single validation finding on a workflow definition: code, severity, authored path, and message. |
| `WorkflowDef` | `def` | A parsed workflow definition with uncompiled JSONLogic expressions. |
| `Expr<T>` | `expr` | A compiled JSONLogic expression that evaluates to `T`. |
| `Path` | `expr` | A pre-parsed context path (`data.*`, `metadata.*`, or `temp_data.*`). |
| `Function` | `function` | Trait implemented by custom host functions. |
| `Ctx` | `function` | Execution context passed to a host function call for reading, writing, and evaluating expressions. |
| `Outcome` | `function` | Return value of a host function call: `Ok`, `Status(u16)`, `Skip`, or `Halt`. |
| `Registry` | `function` | Holds registered custom functions, call layers, and the `ExprEngine` used to compile workflows. |
| `Observer` | `observe` | Trait for receiving inline execution events during a run. |

In addition, `VERSION` provides the crate version string (written to
`metadata.engine_version` on every run), and `datalogic_rs` and `datavalue` are
re-exported so downstream crates don't need to manage separate dependency
versions.

## `def` — Workflow Definitions

Contains the types representing authored workflow definitions. Definitions are
parsed and linted via `def::parse` (or `def::parse_owned`) and serialize back to
canonical 4.0 JSON.
([docs.rs](https://docs.rs/dataflow-rs/4.0.0/dataflow_rs/def/))

| Name | Description |
|---|---|
| `parse`, `parse_owned`, `ParseOptions`, `Parsed` | Parses and lints a workflow JSON value (`parse_owned` rewrites 3.x spellings in place). Returns all `Issue`s, with `Parsed::def` set to `Some` when no issue has `Rejected` severity. `ParseOptions::compat` (default `true`) enables 3.x compatibility rewrites. |
| `WorkflowDef` | Parsed workflow definition: ID, header, priority, condition, steps, `on_failure`, channel, status, version, rollout, loop config, tags, and timestamps. `walk()` traverses all steps with their authored JSON paths. |
| `StepDef` | An element in a `tasks` list: either `Task(TaskDef)` or `Group(GroupDef)`. |
| `TaskDef` | A single task step, including its `FunctionRef`, `on_failure`, `on_rejection`, `terminal`, and `for_each`. |
| `GroupDef` | A group of nested steps gated by a single condition evaluated once on entry. |
| `NodeHeader` | Common metadata shared by workflows, tasks, and groups (`name`, `description`, and `ext`). |
| `FunctionRef` | The target function `name` and raw JSON `input`. |
| `Logic` | Uncompiled JSONLogic expression as authored in the definition. |
| `LoopDef`, `ForEachDef` | Configuration for a workflow `loop` and a task `for_each` fan-out. |
| `FailurePolicy`, `RejectionPolicy`, `WorkflowFailurePolicy` | Task failure policy (`fail`, `halt`, `continue`), task rejection policy (`continue`, `halt`), and workflow failure policy (`fail`, `continue`). |
| `Status` | Workflow lifecycle status: `active`, `paused`, or `archived` (only `active` workflows execute). |
| `Rollout`, `RolloutError` | Percentage-based traffic routing configuration for a workflow. |
| `defaults` | Default constants used when optional fields are omitted (`PRIORITY`, `VERSION`, `CHANNEL`, `INIT`, `INCREMENT`, `MAX_CONCURRENCY`). |
| `MAX_GROUP_DEPTH`, `BUILTIN_FUNCTIONS`, `is_reserved_function` | Maximum group nesting depth (`8`), the eight built-in function names, and helper to check if a function name is reserved. |
| `definition_keys`, `DefinitionKeys` | Lists the recognized JSON keys at each level of a workflow definition. |

## `expr` — Expressions, Paths, and Secrets

Provides the JSONLogic compilation and evaluation engine, context path handling,
secret store, and configuration field types.
([docs.rs](https://docs.rs/dataflow-rs/4.0.0/dataflow_rs/expr/))

| Name | Description |
|---|---|
| `ExprEngine`, `ExprEngineBuilder` | Compiles and evaluates JSONLogic expressions; builder configures operator families, custom operators, `Secrets`, and optional `ops_budget`. |
| `Expr<T>`, `FromValue`, `Evaluated`, `ResolveExpr` | Compiled JSONLogic expression typed to return `T` (`OwnedDataValue`, `String`, `u64`, `i64`, `f64`, `bool`, or `Path`); constant-folds pure expressions at compile time. |
| `Path`, `Root`, `MAX_ARRAY_INDEX` | Pre-parsed context path rooted at `data`, `metadata`, or `temp_data`; supports `get`, `set`, and `remove` with numeric array indices up to `1,048,575`. |
| `Output` | A destination path field (`Expr<Path>`) used by custom functions to emit results. |
| `Interpolated`, `Rendered` | A literal JSON document containing `{"var": …}` placeholders, and its rendered view for a specific call. |
| `Secrets` | Secure store for sensitive values accessed via `{"secret": "name"}` and never recorded in messages or traces. |
| `Operator`, `OperatorFamily` | Metadata for an active operator and the operator family it belongs to. |
| `ConfigField`, `Resolver`, `CompileCx`, `Visit`, `ExprVisitor`, `FieldSpec`, `FieldKind`, `ValueKind` | Traits and types backing `#[derive(FunctionConfig)]` compilation, resolution, and static analysis. |
| `EvalRoot` | Trait implemented by types that can serve as an expression evaluation root (`&OwnedDataValue` and `&serde_json::Value`). |
| `escaped_keys`, `secret_refs` | Helpers that walk raw JSON to find all `$`-escaped template keys or `{"secret": …}` references. |

## `function` — Custom Host Functions and Layers

Defines the `Function` trait, configuration derive macro, call context,
middleware layers, and function testing utilities.
([docs.rs](https://docs.rs/dataflow-rs/4.0.0/dataflow_rs/function/))

| Name | Description |
|---|---|
| `Function` | Trait for custom host functions: defines `type Config`, `spec()`, `prepare()`, and `async fn call()`. |
| `FunctionConfig` (trait & derive) | Compiles a task's `input` JSON into a typed Rust struct once at build time. Supports `#[config(...)]` and `#[output(...)]` attributes. |
| `FunctionSpec`, `RetrySafety`, `Reference` | Declares function metadata for catalogues and compilers: description, category, idempotency, external resource references, and runtime fields. |
| `Ctx`, `CallInfo` | Per-call execution context (`get`, `set`, `remove`, `eval`, `render`, `emit`, `input`, `input_without_primary_output`, `input_with_primary_output`, `resolve`, `record_error`, `deadline`, `cancel`) and call metadata (`info()`). |
| `Outcome` | Execution outcome: `Ok` (`200`), `Status(u16)`, `Skip` (no audit entry), or `Halt` (`299`). |
| `Prepare` | Compile-time context passed to `Function::prepare` (registered name, workflow ID, task ID, and `FunctionSpec`). |
| `CallLayer`, `Next`, `BoxFuture` | Middleware trait for wrapping host function calls at registry or per-run scope. |
| `layers::Answer` | Built-in middleware layer that intercepts matching calls using a closure (ideal for dry runs and mocks). |
| `Registry`, `FunctionInfo` | Stores registered host functions, layers, and the `ExprEngine`; `catalogue()` lists all registered functions and their specs. |
| `Templated`, `Rendered` | Dynamic configuration type for manifest-driven plugins whose fields are declared at runtime via `FunctionSpec::runtime_fields`. |
| `testing` | Unit-testing harness: `testing::call(f, input)` runs a single function call through compilation, layers, and context commit; `testing::config` tests config compilation standalone. |

## `program` — Compilation and Validation

Compiles parsed `WorkflowDef`s against a `Registry` into an immutable `Program`.
([docs.rs](https://docs.rs/dataflow-rs/4.0.0/dataflow_rs/program/))

| Name | Description |
|---|---|
| `Compiler`, `CompileOptions` | `check(&def)` returns all issues; `check_steps(&steps, root)` validates a step fragment; `compile_one(&def)` returns a `Compiled` workflow or `Err(issues)`; `link(workflows)` builds a `Program`. |
| `Compiled`, `CompiledWorkflow` | A successfully compiled workflow along with any non-rejected `Issue`s (`Defect` and `Advisory`) and its `fingerprint()`. |
| `Program`, `WorkflowInfo`, `TaskInfo` | Immutable linked collection of compiled workflows sorted by priority, with metadata accessors. |
| `ProgramId`, `WorkflowIx`, `TaskIx` | Unique identifier for a compiled program and dense indices for its workflows and tasks. |
| `IssueCode`, `Severity` | Validation issue code (`as_str()` returns `SCREAMING_SNAKE_CASE`) and severity (`Rejected`, `Defect`, or `Advisory`). |
| `AuthoredPath`, `PathSeg`, `StepKind` | Path in the authored JSON (`tasks[1].tasks[0].id`) and the enclosing step kind (`Task`, `Group`, or `Setup`). |
| `ResourceOracle` | Trait for validating external resource names (such as connectors) referenced by `FunctionSpec::references` at compile time. |
| `Fingerprint` | Hash of the operators and context paths used by a workflow's expressions (`Display` and `FromStr`). |
| `ParseIssueCodeError`, `ParsePathError`, `ParseFingerprintError` | Error types for `FromStr` parsing. |

## `run` — Executing Messages

Executes compiled programs over messages.
([docs.rs](https://docs.rs/dataflow-rs/4.0.0/dataflow_rs/run/))

| Name | Description |
|---|---|
| `Engine`, `EngineBuilder`, `EngineConfig` | `Engine::builder()` builds an engine directly from definitions; `Engine::new(program, config)` builds from a pre-linked `Program`; `compiler()` and `with_program()` support hot reloading. |
| `ErrorContext` | Configures mirroring of failure records into a context path (e.g., `metadata.errors`) up to a maximum count. |
| `RunOptions` | Per-run settings (channel, routing bucket, audit mode, change capture, deadline, cancellation token, layers, observer, and trace options). |
| `Run`, `RunStatus` | Execution report: `status()`, `workflows()`, `matched()`, `partial()`, `retryable()`, `trace()`, `take_trace()`, `into_result()`. Statuses: `Completed`, `CompletedWithErrors`, `Stopped(Error)`, `TimedOut`, `Cancelled`. |
| `WorkflowRecord`, `Selection`, `WorkflowExit` | Per-workflow execution summary: selection outcome, loop sweep count, and exit reason. |
| `Message` | `new`, `from_parts`, `from_json`; accessors `payload()`, `context()`, `context_mut()`, `audit()`, `errors()`; serializes as `{id, payload, context, audit_trail, errors}`. |
| `Context` | Holds the three context roots (`data`, `metadata`, `temp_data`) with `get`, `set`, and `remove` by `Path`; `take_data()` moves the `data` root out, leaving `{}`. |
| `AuditEntry`, `Change`, `AuditMode` | Task audit record, field change record, and retention mode (`Full`, `Last(n)`, `Off`). |
| `ErrorRecord` | Structured error record stored in `message.errors()`. |
| `CancelToken` | Cooperative cancellation token (`child()` creates a linked child token). |
| `Instant` | Cross-platform monotonic clock type (`std::time::Instant` on native targets, browser clock on `wasm32`). |
| `CHECK_EVERY` | Operation interval (`64`) between deadline and cancellation checks. |

## `observe` — Observers and Traces

Provides inline event observation and step-by-step execution traces.
([docs.rs](https://docs.rs/dataflow-rs/4.0.0/dataflow_rs/observe/))

| Name | Description |
|---|---|
| `Observer`, `Interest` | `interest()` declares which events (`WORKFLOWS`, `TASKS`, `TIMINGS`) to receive; `on(&Event)` handles events synchronously inline. |
| `Event` | `RunStarted`, `WorkflowStarted`, `TaskFinished`, `WorkflowFinished`, and `RunFinished` (all `#[non_exhaustive]`). |
| `TraceOptions` | Configures trace recording: `timings`, `changes`, `redact`, and `max_bytes`. |
| `Trace`, `TraceStep`, `StepResult` | Execution trace containing `initial` context and incremental `steps`; `contexts()` and `mapping_contexts()` replay state step by step; `validate()` checks deserialized traces. |
| `Replay` | Interactive trace cursor that maintains a single working context and seeks on demand (`context_at`, `mapping_context_at`). |

## `compat` — 3.x Migration Utilities

Provides tools for upgrading 3.x workflow definitions to canonical 4.0 JSON.
([docs.rs](https://docs.rs/dataflow-rs/4.0.0/dataflow_rs/compat/))

| Name | Description |
|---|---|
| `migrate_v3` | Parses a 3.x or 4.0 workflow JSON value and rewrites it into canonical 4.0 JSON (or returns `Err(issues)` if any issue is `Rejected`). |
| `Migrated`, `Rewrite`, `RewriteKind` | Contains the migrated `WorkflowDef`, canonical `json`, ordered list of `rewrites`, and any non-rejected `issues`. |

See [Migrating from 3.x](../migration/from-3.md) for details.

## Common Usage Patterns

### Building an Engine

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
    .expect("Err contains all issues from rejected definitions");
# assert_eq!(engine.program().workflows().len(), 1);
```

`EngineBuilder` supports `.register(name, function)`, `.layer(l)`,
`.registry(r)`, `.options(CompileOptions)`, `.config(EngineConfig)`,
`.workflow(WorkflowDef)`, `.workflow_json(&Value)`, and
`.workflow_json_owned(Value)`, followed by `.build() -> Result<Engine, Vec<Issue>>`.

If your application needs to quarantine invalid workflows and run the valid
ones, compile workflows individually with `Compiler`:

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
let compiled = compiler.compile_one(&def).expect("Err contains all issues");
let program: Program = compiler.link([compiled.workflow]);
let engine = Engine::new(program, EngineConfig::default());
# assert_eq!(engine.program().workflows()[0].id, "w");
```

To reload workflows on a running engine without disrupting in-flight runs, call
`engine.compiler()`, link the new workflows, and call
`engine.with_program(program)`.

### Running a Message

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

// Run with default options:
let run: Run = engine.run(&mut message).await;

// Run with custom per-run options (channel, routing bucket, deadline, trace, etc.):
let opts = RunOptions::default().channel("default");
let run: Run = engine.run_with(&mut message, &opts).await;
# assert_eq!(run.matched(), 1);
# });
```

### Inspecting the Run Report

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
    RunStatus::CompletedWithErrors => { /* inspect message.errors() */ }
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

If you prefer a `Result<(), Error>` that returns `Err` when a run stops, times
out, or is cancelled, call `run.into_result()` (extracting `run.take_trace()`
first if you need the trace).

### Constructing a Message

```rust
use dataflow_rs::Message;
use dataflow_rs::datavalue::OwnedDataValue;
use serde_json::json;
use std::sync::Arc;

// From a serde_json::Value payload:
let m = Message::from_json(&json!({"name": "Ada"}));

// From an OwnedDataValue or a shared Arc<OwnedDataValue>:
let payload = Arc::new(OwnedDataValue::from(&json!({"name": "Ada"})));
let m = Message::new(Arc::clone(&payload));

// With pre-populated `data` and `metadata` objects:
let m = Message::from_parts(
    payload,
    OwnedDataValue::from(&json!({"stage": "intake"})),
    OwnedDataValue::Null,
);
# assert_eq!(m.context().data(), &OwnedDataValue::from(&json!({"stage": "intake"})));
```

### Implementing a Custom Host Function

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

### Validating a Workflow Definition

```rust
use dataflow_rs::def::{ParseOptions, Parsed, parse};
use dataflow_rs::program::{CompileOptions, Severity};
use dataflow_rs::{Issue, Registry};
# use serde_json::json;

let json = json!({"id": "w", "tasks": [
    {"id": "t", "function": {"name": "map", "input": {
        "mappings": [{"path": "data.x", "logic": {"var": "payload.x"}}]}}}
]});

// 1. Validate structure (`def` is `Some` when no issue is `Rejected`):
let parsed: Parsed = parse(&json, ParseOptions::default());
let def = parsed.def.expect("the structure is fine");

// 2. Validate functions, inputs, expressions, paths, and secrets:
let compiler = Registry::default().compiler(CompileOptions::default());
let issues: Vec<Issue> = compiler.check(&def);
let rejected = issues.iter().filter(|i| i.severity == Severity::Rejected).count();
# assert_eq!(rejected, 0);
# assert_eq!(issues[0].code.as_str(), "PAYLOAD_VAR");
# assert_eq!(issues[0].path.to_string(), "tasks[0].function.input.mappings[0].logic");
```

## Error Codes

`Error::code()` returns the error code string written to `ErrorRecord`s on
`message.errors()` and reported on `RunStatus::Stopped(error)`:

| `ErrorKind` | Error Code | When It Occurs |
|---|---|---|
| `Eval` | `LOGIC_ERROR` | A JSONLogic expression failed to evaluate. |
| `Budget` | `BUDGET_EXCEEDED` | An expression exceeded the configured `ops_budget` (`budget` feature). |
| `TimedOut` | `TIMEOUT_ERROR` | The run's deadline expired (marked retryable by default). |
| `Cancelled` | `CANCELLED` | The run's cancellation token was triggered. |
| `Config` | `CONFIG_ERROR` | Invalid runtime or engine configuration. |
| `Compile` | `COMPILATION_ERROR` | An expression or definition failed to compile. |
| `Service { code }` | `code` (or `TASK_ERROR` if empty) | Custom error returned by a host function via `Error::service(code, message)`. |
| `Service { code }` | `VALIDATION_ERROR` | `parse_xml`, `publish_json`, or `publish_xml` encountered missing or malformed input data. |

In addition, the engine records the following codes directly on
`message.errors()` without producing a standalone `Error`:

| Recorded Code | When It Is Recorded |
|---|---|
| `TASK_STATUS_ERROR` | A task returned `Outcome::Status(5xx)` (if the failure stops the run, the stop error code is `TASK_ERROR`). |
| `EVALUATION_ERROR` | A `validation` rule or `filter` condition failed to evaluate. |
| `MAPPING_ERROR` | A `map` item failed to evaluate or write (recorded with `path` set to the mapping destination, followed by `TASK_STATUS_ERROR`). |
| `VALIDATION_ERROR` | A `validation` rule evaluated to a non-`true` value (status `400`). |

## Cargo Features

No Cargo features are enabled by default.

| Feature | Description |
|---|---|
| `datetime` | Compiles in `OperatorFamily::Datetime` (enabled at runtime on `ExprEngineBuilder`) and enables ISO-8601 date/duration parsing in `==`, `<`, `<=`, `>`, and `>=`. |
| `tensor` | Compiles in `Tensor` values and `OperatorFamily::Tensor` (must be enabled explicitly on `ExprEngineBuilder`; excluded from `all_operators()`). |
| `budget` | Enables per-evaluation operation limits via `ExprEngineBuilder::ops_budget`. |
| `wasm-web` | Uses browser clock and randomness APIs on `wasm32-unknown-unknown`. |
| `ts` | Derives `ts_rs::TS` on wire types to generate `bindings/dataflow.d.ts` (internal tooling; not covered by semver). |

```toml
[dependencies]
dataflow-rs = { version = "4.1", features = ["datetime", "budget"] }
```
