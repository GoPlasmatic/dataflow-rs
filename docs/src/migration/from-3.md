# Migrating from 3.x

dataflow-rs 4 keeps every stored definition and every stored message
loading, and changes the Rust API. This page is for two readers. A workflow
author reads [What still loads](#what-still-loads) and
[What changes for an author](#what-changes-for-an-author). A Rust host
reads those and then [What changes for a host](#what-changes-for-a-host),
which walks the API change step by step. Both end at the
[checklist](#before-switching).

## What still loads

Every 3.x definition loads through the compat reader, which `def::parse`
runs first when `ParseOptions::compat` is on, and it is on by default. The
reader rewrites each 3.x spelling into its 4.0 counterpart and reports the
rewrite as a `V3_SPELLING` Advisory at the key the author typed. Nothing
else happens to a definition: the engine runs the rewritten form, and the
spelling in the store is the author's business.

### The spellings

| 3.x | 4.0 | Reported as |
|---|---|---|
| `continue_on_error: true` / `false` on a workflow | `on_failure: continue` / `fail` | `V3_SPELLING` Advisory |
| `continue_on_error` and `halt_on` on a task | `on_failure` and `on_rejection`, by the table below | `V3_SPELLING` Advisory |
| `validate` as a function name | `validation` | `V3_SPELLING` Advisory |
| An unknown key on a workflow, task or group, such as the UI's `path` | Moved into that node's `ext`, so `path` becomes `ext.path` | `UNKNOWN_KEY` Advisory |
| An unknown key that is a near miss of a known one, such as `conditon` | Moved into `ext` the same way | `LIKELY_TYPO` Defect |
| An unknown key inside `loop`, `for_each` or `rollout` | The owning node's `ext`, as `loop.<key>` | The same two |
| An unknown key inside a built-in's input, or inside a `map` mapping or a `validation` rule | The task's `ext`, named by its path, such as `function.input.mappings[0].note` | The same two |
| `continue_on_error` on a group | The group's `ext` | `GROUP_CONTINUE_ON_ERROR` Advisory, as in 3.x, and still never honoured |
| A timestamp spelled `2024-01-01 00:00:00 UTC` | Read as 3.x read it; written as RFC 3339 | Nothing |

A near miss is an edit distance of 1 for keys of up to five characters and
2 for longer ones, against the known keys at that level. It is a Defect
because a misspelled key used to load silently with its feature off.

### The policy mapping

3.x had two task flags; 4.0 has two policies, `on_failure` for a 5xx or an
`Err` and `on_rejection` for a 4xx. The four combinations map exactly:

| 3.x task flags | 4.0 task policies | Behaviour |
|---|---|---|
| Neither | `on_failure: fail`, `on_rejection: continue` | A 5xx or `Err` goes to the workflow's policy; a 4xx is recorded and the next task runs |
| `continue_on_error: true` | `on_failure: continue`, `on_rejection: continue` | Everything is recorded and the next task runs |
| `halt_on: "failure"` | `on_failure: fail`, `on_rejection: halt` | A 4xx halts this workflow; a 5xx or `Err` goes to the workflow's policy |
| Both | `on_failure: halt`, `on_rejection: halt` | A 4xx, 5xx or `Err` halts this workflow, and the run goes on |
| `terminal: true` | `terminal: true` | Unchanged: applied after the policy, so a failure still propagates |
| Workflow `continue_on_error: false` / `true` | Workflow `on_failure: fail` / `continue` | `continue` ends this workflow (in a loop, this sweep) and the next one runs |

`halt_on: "never"` and `continue_on_error: false` spell the defaults; they
are rewritten to nothing and still reported.

**Mixing the spellings on one node is refused.** A task or workflow that
carries a 3.x policy key (`continue_on_error`, `halt_on`) beside a 4.0 one
(`on_failure`, `on_rejection`) is `MIXED_SPELLING` at the 3.x key. 3.x ignored
the 4.0 keys, so a stray one must not change a migrated definition's policy
silently.

### Storing the 4.0 spelling

`compat::migrate_v3` parses with the reader on and writes the definition in
the 4.0 spelling, which parses with the reader off to an equal definition.
Its `rewrites` say what changed, in the document's order, and its `issues`
carry everything the definition has, none Rejected:

```rust
use dataflow_rs::compat::migrate_v3;
use dataflow_rs::program::IssueCode;
use serde_json::json;

let stored = json!({
    "id": "checkout",
    "continue_on_error": true,
    "tasks": [
        {"id": "guard", "halt_on": "failure", "path": "guards/1",
         "function": {"name": "validate", "input": {"rules": [
             {"logic": {"!!": [{"var": "data.order"}]}, "message": "no order"}
         ]}}}
    ]
});

let migrated = migrate_v3(&stored).unwrap();
assert_eq!(migrated.json, json!({
    "id": "checkout",
    "on_failure": "continue",
    "tasks": [
        {"id": "guard", "on_rejection": "halt", "ext": {"path": "guards/1"},
         "function": {"name": "validation", "input": {"rules": [
             {"logic": {"!!": [{"var": "data.order"}]}, "message": "no order"}
         ]}}}
    ]
}));

let rewrites: Vec<(String, String)> = migrated.rewrites.iter()
    .map(|r| (r.path.to_string(), r.to_string()))
    .collect();
// In the document's key order, as written.
assert_eq!(rewrites, [
    (
        "continue_on_error".to_string(),
        "`continue_on_error: true` is a v3 spelling, read as `on_failure: continue`".to_string(),
    ),
    (
        "tasks[0].halt_on".to_string(),
        "`halt_on: failure` is a v3 spelling, read as `on_failure: fail, on_rejection: halt`".to_string(),
    ),
    (
        "tasks[0].function.name".to_string(),
        "`validate` is a v3 spelling, read as `validation`".to_string(),
    ),
]);

let codes: Vec<IssueCode> = migrated.issues.iter().map(|i| i.code).collect();
assert_eq!(codes, [
    IssueCode::V3Spelling, IssueCode::V3Spelling, IssueCode::V3Spelling, IssueCode::UnknownKey,
]);
assert_eq!(migrated.issues[3].path.to_string(), "tasks[0].path");
# // The output parses with the reader off to the definition the reader read.
# use dataflow_rs::def::{ParseOptions, parse};
# let off = parse(&migrated.json, ParseOptions::default().compat(false));
# assert_eq!(off.def, parse(&stored, ParseOptions::default()).def);
# assert!(off.issues.is_empty());
```

A definition that mixes spellings, or breaks any structural rule, comes
back as `Err` with every issue.

## What changes for an author

These are the behaviours a definition can observe. Nothing else about a
workflow's semantics moved: a failing `validation` still returns 400 and
gates only when `on_rejection` says so, a `null` map result is still
skipped, `terminal` and the halt policies still apply after the status is
classified, loops and `for_each` are unchanged, a group's condition is still
evaluated once, and `metadata.progress` is still written after every task.

- **A `map` path outside the three roots is refused.** A literal `path` that
  does not start at `data`, `metadata` or `temp_data` is `INVALID_PATH`,
  Rejected, at compile. 3.x wrote it as a new top-level context key. A
  computed path that names no root fails the mapping at run time, so the
  task returns 500.
- **A literal `payload` path is a Defect.** `{"var": "payload.x"}` is
  `PAYLOAD_VAR`: the evaluation context is `{data, metadata, temp_data}`,
  so the read always resolved to nothing in 3.x too. Parse the payload into
  `data` first.
- **No `WORKFLOW_ERROR` records.** The task's record carries the workflow
  id. A workflow's own failure, such as a `loop.over` that is not an array,
  records one `LOGIC_ERROR` with the workflow id and no task id.
- **`name` is optional** on every workflow, task and group.
- **Only `active` workflows run,** with or without a channel. A `paused` or
  `archived` workflow is not considered, and appears in no run report.
- **An unregistered function rejects its workflow at load,** as an
  `UNKNOWN_FUNCTION` issue at `tasks[i].function.name`. 3.x refused the
  build too, with one `FUNCTION_NOT_FOUND` error naming the function; 4.0
  reports every such task and rejects only the workflows that name one.
- **The three integration configs left the engine.** `http_call`, `enrich`
  and `publish_kafka` are a host's functions now, registered like any other,
  and their 3.x spellings (`path_logic`, `body_logic`, `key_logic`,
  `value_logic`) are not rewritten: they reach the host's config as written,
  so a host that still stores them rewrites them in its own loader. A
  definition that names one in a host that has not registered it is
  `UNKNOWN_FUNCTION`.
- **Every unknown key is reported,** as the table above says. Inside a host
  function's `input` the key is Rejected, near miss or not: a derived config
  refuses what it does not declare.
- **An array index stops at 1,048,575.** A bare numeric segment larger than
  that is an object key, so no path can grow an array without bound; 3.x
  padded an array to any index.
- **A write at a root takes only an object,** a host function's or a
  `map`'s. `data`, `metadata` and `temp_data` cannot be replaced with an
  array, a string or a number: a host function's write is a `CONFIG_ERROR`,
  and a `map` whose `logic` is always a non-object at a literal root is
  `INVALID_MAPPING` at build, any other non-object result failing the mapping
  with status 500. 3.x's `map` replaced the root.

## What changes for a host

Every Rust host takes a major version. The table maps the 3.x surface onto
4.0; the numbered steps after it show each change as code.

### The surface, name by name

| 3.x | 4.0 |
|---|---|
| `Workflow`, `Task`, `TaskGroup`, `LoopConfig`, `ForEach`, with hidden compiled fields | `def::WorkflowDef`, `TaskDef`, `GroupDef`, `LoopDef`, `ForEachDef`: read-only data, entered through `def::parse` and read through accessors (`def.id()`, `task.on_failure()`) |
| `Workflow::validate_authored`, `check_workflow` | `def::parse` and `Compiler::check`, same issue vocabulary |
| `walk_authored_steps`, `is_group`, `MAX_GROUP_DEPTH` | `WorkflowDef::walk`, `StepDef::Group`, `def::MAX_GROUP_DEPTH` |
| `Engine::new`, `new_with_operators`, `builder`, `with_new_workflows` | `Registry` + `Compiler` + `Engine::new`, or `Engine::builder`; a reload is `compiler()`, `link`, `with_program` |
| `process_message` and its channel and trace twins | `Engine::run`, `Engine::run_with` |
| `AsyncFunctionHandler`, `type Input`, `execute` | `Function`, a derived `type Config`, `call` |
| `TaskContext` | `Ctx`; handler tests use `function::testing` |
| `Template`, `PathTemplate`, `TemplateCompiler` | `Expr<T>`, `Output`, `Interpolated` |
| `FunctionConfig` enum, `HttpCallConfig`, `EnrichConfig`, `PublishKafkaConfig` | Registry entries; the integration configs are the host's |
| `DataflowError`, `ErrorInfo`, `ServiceErrorBuilder` | `Error` and `ErrorKind`; `ErrorRecord` on the wire |
| `ExecutionObserver` with five methods | `Observer`: `interest` plus one `on` |
| `ExecutionTrace` with snapshots | `Trace`: the initial context plus changes |
| `process_message_tracing`, `TraceOptions` | `RunOptions::trace(TraceOptions)`, then `Run::trace` or `take_trace` |
| `with_error_context_path`, `with_error_context_limit` | `EngineConfig::error_context(ErrorContext::new(path, limit)?)` |
| `MessageBuilder`: `capture_changes`, `audit_mode`, `routing_bucket` | `RunOptions` fields; `MessageBuilder` is gone |
| `get_nested_value`, `set_nested_value`, `remove_nested_value` | `Path::get`, `Path::set`, `Path::remove`, and `Context::get`, `set`, `remove` |
| `declared_secrets`, `operator_names`, `template_key_escape`, `datalogic` | `ExprEngine::secrets().names()`, `operators()`, `escape()`, the `ExprEngine` itself |
| `Rollout::partition`, `validate_set` | Unchanged, in `def` |
| `RetryPolicy`, `retry_with_policy`, `retry_with_attempts` | Removed; retry is the host's policy |
| `Rule`, `Action`, `RulesEngine`; `continue_on_error`, `halt_on` | The aliases are gone; the compat reader rewrites the spellings |

### 1. Build

3.x parsed each definition into a `Workflow` and built one engine over all
of them:

```text
// 3.x
let workflow = Workflow::from_json(json_text)?;
let engine = Engine::builder()
    .with_workflow(workflow)
    .register("fetch", Fetch)
    .build()?;
```

4.0 has two shapes. A host that does not quarantine uses the builder, whose
`build` fails with every rejected definition's issues:

```rust
use dataflow_rs::Engine;
# use dataflow_rs::{Ctx, Function, Outcome, Result};
# use serde_json::json;
# struct Fetch;
# impl Function for Fetch {
#     type Config = ();
#     async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> { Ok(Outcome::Ok) }
# }
# let json = json!({"id": "w", "tasks": [{"id": "t", "function": {"name": "fetch"}}]});

let engine = Engine::builder()
    .register("fetch", Fetch)
    .workflow_json(&json)
    .build()
    .map_err(|issues| issues.len())?;
# Ok::<(), usize>(())
```

A host that stores definitions and runs the ones that compile owns the
quarantine: one `Compiler`, one `compile_one` per workflow, then `link` and
`Engine::new`:

```rust
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::program::CompileOptions;
use dataflow_rs::run::EngineConfig;
use dataflow_rs::{Engine, Registry};
# use serde_json::json;
# let stored = vec![
#     json!({"id": "a", "tasks": [{"id": "t", "function": {"name": "log", "input": {"message": "a"}}}]}),
#     json!({"id": "b", "tasks": [{"id": "t", "function": {"name": "nowhere"}}]}),
# ];

let compiler = Registry::default().compiler(CompileOptions::default());
let mut workflows = Vec::new();
for json in &stored {
    let Some(def) = parse(json, ParseOptions::default()).def else { continue };
    match compiler.compile_one(&def) {
        Ok(compiled) => workflows.push(compiled.workflow),
        Err(issues) => eprintln!("{}: {} issues, quarantined", def.id(), issues.len()),
    }
}
let engine = Engine::new(compiler.link(workflows), EngineConfig::default());
# assert_eq!(engine.program().workflows().len(), 1);

// A reload keeps the registry and the config.
let reloaded = engine.with_program(engine.compiler().link([]));
# assert_eq!(reloaded.program().workflows().len(), 0);
```

### 2. Handlers

3.x handlers implemented `AsyncFunctionHandler` with a serde `Input` and
wrote through a `TaskContext`:

```text
// 3.x
#[derive(Deserialize)]
struct FetchInput { url: Template, target: String }

#[async_trait]
impl AsyncFunctionHandler for Fetch {
    type Input = FetchInput;
    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &FetchInput)
        -> Result<TaskOutcome> {
        let url = input.url.resolve_str(ctx)?;
        ctx.set(&input.target, response);
        Ok(TaskOutcome::Success)
    }
}
```

4.0 handlers implement `Function`. The config is derived: each field's type
says what it is (`Expr<T>` an expression, `Output` a write destination,
`Interpolated` a document, a plain type a literal, and `#[config(serde)]`
a type serde already reads), unknown keys are refused, and every issue is
reported at its own path when the workflow compiles, not on the first
message:

```rust
use dataflow_rs::expr::Output;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::{Ctx, Expr, Function, Outcome, Result};
# use serde_json::json;

#[derive(FunctionConfig)]
struct FetchConfig {
    url: Expr<String>,
    #[config(default = 5000)]
    timeout_ms: Expr<u64>,
    #[output(default = "data.response")]
    output: Output,
}

struct Fetch;

impl Function for Fetch {
    type Config = FetchConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &FetchConfig) -> Result<Outcome> {
        let url = ctx.eval(&cfg.url)?;
        let timeout_ms = ctx.eval(&cfg.timeout_ms)?;
        let response = OwnedDataValue::from(&json!({"url": &*url, "timeout_ms": timeout_ms}));
        ctx.emit(&cfg.output, response)?;
        Ok(Outcome::Ok)
    }
}
# let _ = Fetch;
```

`TaskContext` becomes `Ctx`, and the methods map one to one:

| 3.x `TaskContext` | 4.0 `Ctx` |
|---|---|
| `ctx.get("data.x")` | `ctx.get(&path)`, a `Path` parsed once |
| `ctx.set("data.x", value)` | `ctx.set(&path, value)?`; at a root it takes only an object |
| `ctx.eval(&logic)` | `ctx.eval(&expr)`, typed by the `Expr<T>` |
| `input.target`, a path string | `ctx.emit(&cfg.output, value)?` through an `Output` |
| `ctx.add_error(info)` | `ctx.record_error(record)` |
| `ctx.workflow_id()`, `task_id()`, `loop_counter()`, `element_index()` | `ctx.info().workflow_id`, `task_id`, `loop_counter`, `element_index` |
| `ctx.secret(name)` | `ctx.secret(name)`, unchanged |
| `ctx.message_mut()` | Gone: `set`, `remove` and `emit` are the only writes |
| `TaskOutcome::Success` | `Outcome::Ok`; `Status`, `Skip` and `Halt` are unchanged |

A call's writes go to an overlay of their own: the call reads them at
once, and they reach the message only when it returns `Ok`, whatever the
`Outcome`. An `Err` drops them. Test a function with
`function::testing::call`, which runs one call through the real compile,
overlay and layers, instead of building a one-workflow engine.

### 3. Calls

3.x returned `Err` when the engine stopped early, and left the rest on the
message:

```text
// 3.x
match engine.process_message(&mut message).await {
    Ok(()) => {}
    Err(e) => eprintln!("stopped: {e}"),
}
```

4.0 returns a `Run` on every outcome and the status is read from it.
`message.errors()` remains the only error list, with two additions: a failed
`map` mapping records `MAPPING_ERROR` (its destination as the `path`) before
the task's `TASK_STATUS_ERROR`, and a `filter` condition that fails to
evaluate records `EVALUATION_ERROR` where 3.x recorded nothing:

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::run::RunStatus;
use dataflow_rs::{Engine, Message, RunOptions};
# use serde_json::json;
# let engine = Engine::builder()
#     .workflow_json(&json!({"id": "w", "tasks": [
#         {"id": "t", "function": {"name": "validation", "input": {"rules": [
#             {"logic": false, "message": "always"}]}}}
#     ]}))
#     .build().unwrap();

let mut message = Message::from_json(&json!({}));
let run = engine.run_with(&mut message, &RunOptions::default().channel("default")).await;
match run.status() {
    RunStatus::Completed => {}
    RunStatus::CompletedWithErrors => eprintln!("{} records", message.errors().len()),
    RunStatus::Stopped(e) => eprintln!("stopped: {}", e.code()),
    RunStatus::TimedOut | RunStatus::Cancelled => eprintln!("cut short"),
    _ => {}
}
# assert!(matches!(run.status(), RunStatus::CompletedWithErrors));
# assert_eq!(message.errors()[0].code, "VALIDATION_ERROR");
# });
```

`Run::into_result()` gives the 3.x shape back, `Err` on a stop, but drops
the report and its trace.

### 4. Observers, traces and the error context

3.x observers implemented up to five methods and were attached on the
builder:

```text
// 3.x
impl ExecutionObserver for Metrics {
    fn task_finished(&self, event: &TaskEvent<'_>) { /* event.function, event.status, event.duration */ }
    fn message_finished(&self, event: &MessageFinished<'_>) { /* event.errors, event.stopped_early */ }
}
Engine::builder().with_observer(Arc::new(Metrics::default()))
```

4.0 observers implement one `on(&Event)` and say what they want with
`interest()`, read once per run. The default, `WORKFLOWS | TASKS`, reads no
clock; `TIMINGS` makes every `elapsed` `Some`. The five callbacks map one to
one: `message_started` to `RunStarted` (its `workflows_considered` is
`considered`), `task_finished` to `TaskFinished` (its `function` is the
registered name, `validation` where 3.x said `validate`),
`workflow_finished` to `WorkflowFinished` (its `halted` is
`record.exit == Some(WorkflowExit::Halted)`), and `message_finished` to
`RunFinished` (its `stopped_early` is a `Stopped`, `TimedOut` or
`Cancelled` status). Each variant is `#[non_exhaustive]`, so match with
`..`:

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::observe::{Event, Interest, Observer};
use dataflow_rs::run::{EngineConfig, RunStatus};
use dataflow_rs::{Engine, Message};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
# use serde_json::json;

#[derive(Default)]
struct Metrics { tasks: AtomicUsize, stopped: AtomicUsize }

impl Observer for Metrics {
    fn interest(&self) -> Interest {
        Interest::TASKS | Interest::TIMINGS
    }

    fn on(&self, event: &Event<'_>) {
        match event {
            Event::TaskFinished { info, status, elapsed, .. } => {
                let _ = (info.function, status, elapsed.expect("TIMINGS was asked for"));
                self.tasks.fetch_add(1, Ordering::Relaxed);
            }
            Event::RunFinished { status, .. } => {
                if !matches!(status, RunStatus::Completed | RunStatus::CompletedWithErrors) {
                    self.stopped.fetch_add(1, Ordering::Relaxed);
                }
            }
            _ => {}
        }
    }
}

let metrics = Arc::new(Metrics::default());
let engine = Engine::builder()
    .config(EngineConfig::default().observer(metrics.clone()))
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.a", "logic": 1}]}}}
    ]}))
    .build()
    .unwrap();
engine.run(&mut Message::from_json(&json!({}))).await;
assert_eq!(metrics.tasks.load(Ordering::Relaxed), 1);
# assert_eq!(metrics.stopped.load(Ordering::Relaxed), 0);
# });
```

An observer on `EngineConfig::observer` sees every run and survives a
reload; one on `RunOptions::observer` sees one run.

3.x traced through a second entry point and a trace it filled:

```text
// 3.x
let mut trace = ExecutionTrace::with_options(TraceOptions { snapshots: true, ..Default::default() });
engine.process_message_tracing(&mut message, &mut trace).await?;
for step in &trace.steps { /* step.message is a snapshot */ }
```

4.0 asks for the trace on the options and reads it from the `Run`, on every
status. `redact_paths` is `redact`, `max_snapshot_bytes` is `max_bytes`, and
the snapshot knobs are gone, because there are no snapshots: the trace is
the initial context plus each step's changes, and `Trace::contexts()`
replays them:

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::observe::{StepResult, TraceOptions};
use dataflow_rs::{Engine, Message, RunOptions};
# use serde_json::json;
# let engine = Engine::builder()
#     .workflow_json(&json!({"id": "w", "tasks": [
#         {"id": "t", "function": {"name": "map", "input": {
#             "mappings": [{"path": "data.a", "logic": 1}, {"path": "data.b", "logic": 2}]}}}
#     ]}))
#     .build().unwrap();

let mut message = Message::from_json(&json!({}));
let opts = RunOptions::default().trace(TraceOptions::default().timings(true));
let mut run = engine.run_with(&mut message, &opts).await;

let trace = run.take_trace().expect("a trace was asked for");
for (step, context) in trace.steps.iter().zip(trace.contexts()) {
    // `context` is the whole context after this step.
    let _ = (step.result, step.status, &step.changes, &step.errors, &step.mapping_ends, context);
}
# assert_eq!(trace.steps.len(), 1);
# assert_eq!(trace.steps[0].result, StepResult::Executed);
# assert_eq!(trace.steps[0].mapping_ends, [1, 2]);
# assert!(trace.steps[0].elapsed.is_some());
# let last = trace.contexts().last().unwrap();
# assert_eq!(last.data(), message.context().data());
# });
```

3.x configured the error context on the builder:

```text
// 3.x
Engine::builder()
    .with_error_context_path("metadata.errors")
    .with_error_context_limit(50)
```

4.0 puts it on the `EngineConfig`, and refuses a path at or below a
`metadata` slot the engine writes (`progress`, `processed_at`,
`engine_version`, `channel`). The path is reset at run start, so a message
never brings records in:

```rust
use dataflow_rs::run::{EngineConfig, ErrorContext};
use dataflow_rs::Engine;

let config = EngineConfig::default()
    .error_context(ErrorContext::new("metadata.errors".parse()?, 50)?);
let builder = Engine::builder().config(config);
# let _ = builder;
# Ok::<(), dataflow_rs::Error>(())
```

### 5. Errors

3.x had a `DataflowError` with a variant per cause and a builder for a
service's own:

```text
// 3.x
return Err(DataflowError::service("HTTP_ERROR", "upstream 502").retryable(true).build());
```

4.0 has `Error` with an `ErrorKind`. A host makes its own with
`Error::service(code, message)`, and keeps a stored code by naming it:

```rust
use dataflow_rs::{Error, ErrorKind};

let e = Error::service("HTTP_ERROR", "upstream 502").with_retryable(true);
assert_eq!(e.code(), "HTTP_ERROR");
assert!(matches!(e.kind(), ErrorKind::Service { .. }));
assert!(e.is_retryable());
```

The recorded codes `LOGIC_ERROR`, `TIMEOUT_ERROR`, `BUDGET_EXCEEDED`,
`TASK_STATUS_ERROR` and `VALIDATION_ERROR` are unchanged; `CANCELLED` and
`MAPPING_ERROR` are new,
and `TIMEOUT_ERROR` is now also what the engine's own deadline writes.
`FUNCTION_NOT_FOUND` and `DESERIALIZATION_ERROR` are gone: an unregistered
function is the `UNKNOWN_FUNCTION` issue at build, and a value that does not
convert to a field's type records `LOGIC_ERROR`. See the
[error codes](../api/reference.md#error-codes).

### 6. Options, paths and retries

`capture_changes`, `audit_mode` and `routing_bucket` were set on the
message:

```text
// 3.x
let message = Message::builder()
    .payload_json(&payload)
    .capture_changes(false)
    .audit_mode(AuditMode::Last(10))
    .routing_bucket(7)
    .build();
```

They are choices about a run, so they moved to `RunOptions`, with unchanged
defaults. None of the three ever serialized, so no stored message changes:

```rust
use dataflow_rs::run::AuditMode;
use dataflow_rs::RunOptions;

let opts = RunOptions::default()
    .capture_changes(false)
    .audit(AuditMode::Last(10))
    .routing_bucket(7);
# assert_eq!(opts.routing_bucket, Some(7));
```

The path helpers took a dotted string on every call:

```text
// 3.x
use dataflow_rs::engine::utils::{get_nested_value, set_nested_value, remove_nested_value};
set_nested_value(message.data_mut(), "user.name", value);
```

4.0 parses a `Path` once and reads or writes through it, or through the
`Context` it names:

```rust
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::{Message, Path};
use serde_json::json;

let mut message = Message::from_json(&json!({}));
let name: Path = "data.user.name".parse()?;
message.context_mut().set(&name, OwnedDataValue::from(&json!("Ada")))?;
assert_eq!(message.context().get(&name), Some(&OwnedDataValue::from(&json!("Ada"))));
let removed = message.context_mut().remove(&name)?;
assert!(removed.is_some());

// A path must start at a root.
assert!("payload.x".parse::<Path>().is_err());
# Ok::<(), dataflow_rs::Error>(())
```

`RetryPolicy`, `retry_with_policy` and `retry_with_attempts` are removed.
Retry is the host's policy: `Run::retryable()` says whether running the
message again may succeed, and `Error::is_retryable()` the same for a stop.

### The trace, on the wire

A 3.x trace carried a message snapshot per step. A 4.0 trace is
`{initial, steps, truncated}`: the context after the run's stamps, then one
`TraceStep` per call, skipped step, failure and engine write, each with its
`changes`. Three things are new in a step:

- `result` can be `failed`: the step whose failure went to the workflow's
  policy or stopped the run, which 3.x dropped.
- `errors` is every record the step added to the message, in place of the
  snapshot's error list.
- a `map` step carries `mapping_ends`, where each mapping's changes end in
  `changes`, in place of `mapping_contexts`; `Trace::mapping_contexts(step)`
  replays them.

Both keys are written only when non-empty. A trace reads back through
`Deserialize`, and `Trace::contexts()` replays it to the run's context on
every status, so a host that showed snapshots replays instead.

### Messages, on the wire

The message JSON is unchanged: `{id, payload, context, audit_trail,
errors}`, the audit entry and the error record with the same keys in the
same order, and every message 4.0 writes reads back byte for byte. Three
things are refused on read: a context with a key besides `data`, `metadata`
and `temp_data`, a context that is not an object, or a root named twice; a
root that is not an object; and an object anywhere in the message holding a
key twice, which no message 3.x wrote holds. 3.x's `map` could write a fourth key for a path
outside the roots, or replace a root with a scalar; a stored message that
carries either no longer reads. A `null` `metadata` used to drop every write
under it silently, `metadata.progress` included.

```rust
use dataflow_rs::Message;

let text = r#"{"id":"m","payload":{"a":1},"context":{"data":{},"metadata":{},"temp_data":{}},"audit_trail":[],"errors":[]}"#;
let message: Message = serde_json::from_str(text)?;
assert_eq!(serde_json::to_string(&message)?, text);

let fourth = text.replace(r#""temp_data":{}"#, r#""temp_data":{},"secrets":{}"#);
let refused = serde_json::from_str::<Message>(&fourth).unwrap_err().to_string();
assert!(refused.starts_with("a context holds only `data`, `metadata` and `temp_data`, not `secrets`"));
# Ok::<(), serde_json::Error>(())
```

### The wasm and UI packages

`@goplasmatic/dataflow-wasm` 4.0 has one `WasmEngine`, built with
`new WasmEngine(workflows, { secrets })` and run with
`engine.run(payload, options)`, which resolves on every status with
`{ message, run, trace? }` and throws a structured `EngineError` for a
refused definition; see [its page](../wasm/overview.md). The
`@goplasmatic/dataflow-ui` 4.0 debugger takes the wasm package's generated
types, replays each step's context from the trace instead of reading
snapshots, and reads a node's `ext.path`; see
[its page](../ui/overview.md).

## Before switching

Run 4.0 over every stored definition before any message does.

1. **Parse and check each definition** with `def::parse` and
   `Compiler::check`, against the registry the host will run with.
2. **Fix what is Rejected, and every `LIKELY_TYPO`.** A Rejected issue is a
   workflow that will not load; a `LIKELY_TYPO` is a setting that never
   took effect.
3. **Read the Advisories.** `V3_SPELLING` says what the reader rewrote;
   `UNKNOWN_KEY` says what moved into `ext`.
4. **Store `migrate_v3`'s output,** so the definitions are in the 4.0
   spelling and parse with the reader off.
5. **Store each workflow's `Fingerprint`,** the operators and data keys its
   expressions use, to see what a later change touches.
6. **For a host that stores messages,** find any whose context holds a key
   outside the three roots: it no longer reads.

```rust
use dataflow_rs::compat::migrate_v3;
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::program::{CompileOptions, IssueCode, Severity};
use dataflow_rs::Registry;
# use serde_json::json;
# let stored = vec![
#     json!({"id": "a", "continue_on_error": true, "tasks": [
#         {"id": "t", "function": {"name": "log", "input": {"message": "a"}}}]}),
#     json!({"id": "b", "tasks": [{"id": "t", "conditon": true, "function": {"name": "nowhere"}}]}),
# ];

let compiler = Registry::default().compiler(CompileOptions::default());
let mut ready = Vec::new();
for json in &stored {
    let parsed = parse(json, ParseOptions::default());
    let mut issues = parsed.issues;
    if let Some(def) = &parsed.def {
        issues.extend(compiler.check(def));
    }
    let blocking = issues.iter().any(|i| {
        i.severity == Severity::Rejected || i.code == IssueCode::LikelyTypo
    });
    if blocking {
        eprintln!("{json:#}: {issues:#?}");
        continue;
    }
    let def = parsed.def.expect("nothing was Rejected");
    let migrated = migrate_v3(json).expect("nothing was Rejected");
    let fingerprint = compiler.compile_one(&def).expect("checked").workflow.fingerprint();
    ready.push((migrated.json, fingerprint.to_string()));
}
# assert_eq!(ready.len(), 1);
# assert_eq!(ready[0].0["on_failure"], "continue");
# assert!(ready[0].1.starts_with("v1:"));
```

A `Fingerprint` displays as `v1:` and sixteen hex digits, and parses back
with `str::parse`.
