# Engine

The `Engine` runs messages over a compiled program. It is the central component that selects workflows and orchestrates task execution.

## Overview

The Engine is responsible for:

- Running messages through the program's active workflows, in priority order
- Evaluating workflow and task conditions against the full message context
- Applying each task's outcome policies and recording what happened
- Routing a run to one channel's workflows, and admitting rollouts by bucket
- Reporting each run through a `Run`, to observers as it goes, and in a trace when asked
- Giving a host the compiler to build the next program for a reload

Building is a separate step. Definitions are parsed and linted by `def::parse`,
compiled by a `Compiler` that a `Registry` gives, and linked into an immutable
`Program`; the engine runs that program with its `EngineConfig`.

## Creating an Engine

`Engine::builder()` does the whole build for a host that does not quarantine:
every definition must compile, or `build()` fails.

```rust
use dataflow_rs::Engine;
use serde_json::json;

# fn main() -> Result<(), Vec<dataflow_rs::Issue>> {
let first = json!({
    "id": "first",
    "priority": 1,
    "tasks": [
        {"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.first", "logic": true}]}}}
    ]
});
let second = json!({
    "id": "second",
    "priority": 2,
    "tasks": [
        {"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.second", "logic": true}]}}}
    ]
});

let engine = Engine::builder()
    .workflow_json(&first)
    .workflow_json(&second)
    // .register("my_function", MyFunction)  // host functions go here
    .build()?;

// Every expression is compiled, every function config typed.
println!("Loaded {} workflows", engine.program().workflows().len());
# assert_eq!(engine.program().workflows().len(), 2);
# Ok(()) }
```

The builder's methods:

- `.workflow_json(&value)` parses a definition with the compat reader on, so a
  3.x spelling loads and is reported; `.workflow(def)` adds a `WorkflowDef`
  you parsed yourself
- `.register("name", function)` adds a host [`Function`](../advanced/custom-functions.md);
  `.layer(layer)` wraps every host call; `.registry(registry)` replaces the
  registry, and with it the expression engine's operators, secrets and
  budget, along with every function registered so far, so call it first
- `.options(CompileOptions)` and `.config(EngineConfig)` set how the
  definitions compile and how the engine runs
- `.build()` returns `Result<Engine, Vec<Issue>>`: every issue of every
  rejected definition, in input order, a definition's parse issues before its
  compile issues. On success the Defects and Advisories are dropped; a host
  that wants them calls `Compiler::check`

```rust
use dataflow_rs::Engine;
use dataflow_rs::program::IssueCode;
use serde_json::json;

let issues = Engine::builder()
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "t", "function": {"name": "fetch"}}
    ]}))
    .build()
    .unwrap_err();
assert_eq!(issues[0].code, IssueCode::UnknownFunction);
assert_eq!(issues[0].path.to_string(), "tasks[0].function.name");
println!("{}", issues[0]);
```

### Quarantining a bad definition

A host that loads definitions from storage usually wants the good ones to run
while a bad one is reported. `Engine::new(program, config)` takes the program
you link yourself, one workflow at a time:

```rust
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::program::CompileOptions;
use dataflow_rs::run::EngineConfig;
use dataflow_rs::{Engine, Registry};
use serde_json::json;

let definitions = vec![
    json!({"id": "good", "tasks": [
        {"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.ok", "logic": true}]}}}
    ]}),
    json!({"id": "bad", "tasks": [
        {"id": "t", "function": {"name": "no_such_function"}}
    ]}),
];

let compiler = Registry::default().compiler(CompileOptions::default());
let mut workflows = Vec::new();
for raw in &definitions {
    // `parse` never fails: the definition is `Some` exactly when no issue
    // is Rejected.
    let parsed = parse(raw, ParseOptions::default());
    let Some(def) = parsed.def else {
        for issue in &parsed.issues {
            eprintln!("{issue}");
        }
        continue;
    };
    match compiler.compile_one(&def) {
        Ok(compiled) => workflows.push(compiled.workflow),
        Err(issues) => {
            for issue in &issues {
                eprintln!("{}: {issue}", def.id());
            }
        }
    }
}
let engine = Engine::new(compiler.link(workflows), EngineConfig::default());
assert_eq!(engine.program().workflows().len(), 1);
```

`compile_one` returns `Err` with every issue when any is Rejected, and `Ok`
with the compiled workflow and its Defects and Advisories otherwise. `link`
never fails: it sorts the workflows by priority (input order breaks ties) and
indexes their tasks.

## Running Messages

```rust
# use dataflow_rs::Engine;
# async fn _demo(engine: Engine) {
use dataflow_rs::Message;
use dataflow_rs::run::RunStatus;
use serde_json::json;

// A message from a JSON payload.
let mut message = Message::from_json(&json!({
    "user": "john",
    "action": "login"
}));

// Run it through every active workflow. A `Run` comes back on every outcome.
let run = engine.run(&mut message).await;

match run.status() {
    RunStatus::Completed => {}
    RunStatus::CompletedWithErrors => eprintln!("{} error(s)", message.errors().len()),
    RunStatus::Stopped(error) => eprintln!("stopped: {error}"),
    RunStatus::TimedOut | RunStatus::Cancelled => eprintln!("cut short"),
    _ => {}
}
println!("{} workflow(s) ran", run.matched());
println!("data: {:?}", message.context().data());
println!("audit entries: {}", message.audit().len());
# }
```

The `Run` reports:

- `status()`: `Completed`, `CompletedWithErrors` (every considered workflow
  finished, but the message holds error records), `Stopped(error)` (a task's
  failure stopped the run), `TimedOut` or `Cancelled` (the run's own deadline
  or token)
- `matched()`: how many workflows were selected and ran
- `workflows()`: every workflow considered, in program order, up to and
  including the one that stopped the run: its `selection` (`Ran`,
  `SkippedByCondition`, `ConditionFailed(error)`, `SkippedByRollout`), its
  `sweeps`, and its `exit` (`Completed`, `Halted`, `Failed`, `Stopped`)
- `partial()`: whether the run ended before every workflow was considered;
  `retryable()`: whether running the message again may succeed
- `trace()` and `take_trace()`: the trace, when one was asked for
- `into_result()`: `Ok(run)` unless the run was stopped, timed out or
  cancelled, for a caller that wants a `?`

If you already hold the payload as an `OwnedDataValue`, `Message::new` takes
it directly, and an `Arc<OwnedDataValue>` is shared rather than copied:

```rust
use dataflow_rs::Message;
use dataflow_rs::datavalue::OwnedDataValue;
use serde_json::json;
use std::sync::Arc;

let payload = Arc::new(OwnedDataValue::from(&json!({"user": "john"})));
let a = Message::new(Arc::clone(&payload));
let b = Message::new(payload);
assert_eq!(a.payload(), b.payload());
```

### Run options

`run_with(&mut message, &RunOptions)` takes what a caller chooses about one
run; `run` uses the defaults.

| Option | Default | What it does |
|---|---|---|
| `channel(&str)` | every active workflow | Run only the active workflows of this channel; `metadata.channel` is stamped |
| `routing_bucket(u8)` | none | The bucket rollouts are tested against; no bucket is admitted by every rollout |
| `capture_changes(bool)` | `true` | Record each write's `Change` in its task's audit entry |
| `audit(AuditMode)` | `Full` | Which audit entries the message keeps: `Full`, `Last(n)` or `Off` |
| `deadline(Instant)` | none | End the run `TimedOut` once it passes; checked at every host call, fan-out element, sweep and every 64th op, and a host call is raced against it |
| `cancel(&CancelToken)` | none | End the run `Cancelled` once the token is cancelled; checked where the deadline is |
| `layers(&[Arc<dyn CallLayer>])` | none | Wrap every host call of this run, outside the registry's layers |
| `observer(&dyn Observer)` | none | Report this run to an observer, beside the engine's |
| `trace(TraceOptions)` | none | Record a trace, read back from `Run::trace` |

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message) {
use dataflow_rs::run::{AuditMode, RunOptions};

let opts = RunOptions::default()
    .channel("orders")
    .audit(AuditMode::Last(10));
let run = engine.run_with(&mut message, &opts).await;
# let _ = run;
# }
```

A deadline needs a Tokio runtime with its time driver, since a host call is
raced against it on a Tokio timer. Only the run's own limits end it `TimedOut`
or `Cancelled`: a function's own timeout error is its task's failure, handled
by the task's `on_failure`.

## Execution Tracing

Ask for a trace with `RunOptions::trace`. The `Run` then carries a `Trace`: the
context after the run's stamps and before any workflow (`initial`), then one
`TraceStep` per recorded call, skipped step, failure and engine write, each
with the changes it made. A trace comes back on every status, the step that
stopped the run included.

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message) {
use dataflow_rs::observe::TraceOptions;
use dataflow_rs::run::RunOptions;

let opts = RunOptions::default().trace(TraceOptions::default());
let run = engine.run_with(&mut message, &opts).await;
let trace = run.trace().expect("a trace was asked for");

for step in &trace.steps {
    println!(
        "{}/{}: {:?}, status {:?}, {} change(s)",
        step.workflow_id,
        step.task_id.as_deref().unwrap_or("-"),
        step.result,
        step.status,
        step.changes.len()
    );
}

// Replay the changes from `initial`: the context after each step.
for (step, context) in trace.steps.iter().zip(trace.contexts()) {
    println!("after {:?}: {:?}", step.task_id, context.data());
}
# }
```

A step's `result` is `Executed` (a call ran, or the engine wrote, and the run
went on), `Skipped` (a condition or a rollout refused it) or `Failed` (the
failure went to the workflow's policy or stopped the run). Its `errors` are
the records the step added to the message. A `map` step's `mapping_ends` say
where each mapping's changes end, and `Trace::mapping_contexts` replays them
one mapping at a time.

### Bounding what a trace captures

`TraceOptions` has four knobs:

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message) -> dataflow_rs::Result<()> {
use dataflow_rs::observe::TraceOptions;
use dataflow_rs::run::RunOptions;

let trace = TraceOptions::default()
    // Time each call, into its step's `elapsed`. Off by default: no clock
    // is read for a trace without it.
    .timings(true)
    // Never let these subtrees reach the trace. `initial` and every change
    // at or below them read `null`; the live message keeps its real values.
    .redact(vec!["data.card.pan".parse()?])
    // A ceiling on what the trace holds. The step that would cross it, and
    // every later one, keeps everything but its changes.
    .max_bytes(256 * 1024);
let run = engine.run_with(&mut message, &RunOptions::default().trace(trace)).await;

if run.trace().is_some_and(|t| t.truncated) {
    println!("byte budget hit: later steps carry no changes");
}
# Ok(()) }
```

The fourth, `.changes(false)`, keeps statuses and timings only, for
profiling; such a trace cannot be replayed. A trace records its own changes
whatever `RunOptions::capture_changes` says, which governs only the audit
trail.

## Always-on Per-task Metrics

A trace is a per-request allocation you keep. For aggregation (counters,
histograms, spans), attach an `Observer`. It is called inline with each
event its `interest` names, on every run, with no trace involved:

```rust
use dataflow_rs::observe::{Event, Interest, Observer};
use dataflow_rs::run::EngineConfig;
use dataflow_rs::{Engine, Message};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
struct Metrics {
    tasks: AtomicU64,
    total_us: AtomicU64,
}

impl Observer for Metrics {
    // Asking for TIMINGS is what makes the clock be read: an observer that
    // only counts leaves it unread.
    fn interest(&self) -> Interest {
        Interest::TASKS | Interest::TIMINGS
    }

    fn on(&self, event: &Event<'_>) {
        // Must be cheap and non-blocking; see the contract below.
        if let Event::TaskFinished { elapsed, .. } = event {
            self.tasks.fetch_add(1, Ordering::Relaxed);
            if let Some(elapsed) = elapsed {
                self.total_us
                    .fetch_add(elapsed.as_micros() as u64, Ordering::Relaxed);
            }
        }
    }
}

# #[tokio::main]
# async fn main() {
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
# }
```

An observer attached through `EngineConfig::observer` sees every run of the
engine; one passed as `RunOptions::observer(&o)` sees that run only, beside
the engine's. Each is called only with the events its `interest()` names,
read once per run: `WORKFLOWS` for `WorkflowStarted` and `WorkflowFinished`,
`TASKS` for `TaskFinished`, and `TIMINGS` to have each `elapsed` filled in.
Every observer receives `RunStarted` and `RunFinished`. The default interest
is `WORKFLOWS | TASKS`, which reads no clock.

`Event::TaskFinished` carries the call's `info` (`workflow_id`, `task_id`,
`function`, `loop_counter`, `element_index`), its `status` and its `elapsed`.
Notes on the edges:

- **`status`** is `None` when the function returned `Outcome::Skip` (the body
  ran, but no audit entry was recorded), and `Some(500)` when the task
  returned `Err`. Failing tasks are reported, not dropped.
- **A task whose condition evaluated false is not reported**: it was never
  dispatched, so there is nothing to time.
- **A fan-out** reports once per element it folds.
- **`elapsed`** is the call alone, the recording not included, and is `Some`
  only for an observer whose interest holds `TIMINGS`.

The callback runs on the run's thread. Keep it non-blocking, do not re-enter
the engine, and do not panic (a panic unwinds out of `run`). Push to a channel
or increment an atomic counter. Match event variants with `..` because each
variant is `#[non_exhaustive]`.

### Workflow and run lifecycle

The other events follow the same rules:

- `WorkflowStarted` fires once a workflow is selected, and `WorkflowFinished`
  when it ends, with its `WorkflowRecord` (`selection`, `sweeps`, `exit`) and,
  under `TIMINGS`, the time since it started. A workflow its rollout or its
  condition refused never starts: no event, the same way a skipped task goes
  unreported.
- A **looping** workflow reports **one** `WorkflowFinished` for the whole
  loop, carrying `sweeps`.
- `RunStarted` carries `considered`: how many workflows the run considers,
  which is its channel's active ones, before any rollout or condition is
  judged. `RunFinished` carries the `status` and how many error records the
  message holds, whether the run completed or was cut short.

## Workflow Execution Order

Workflows run in priority order (lowest first); equal priorities keep their
input order. Only `active` workflows run: a `paused` or `archived` one is kept
in the program and never selected.

```jsonc
[
    {"id": "high", "priority": 1, "tasks": [...]},
    {"id": "low", "priority": 10, "tasks": [...]}
]
```

With `RunOptions::channel`, only that channel's active workflows are
considered; an unknown channel runs nothing, and still stamps the message. A
workflow with a `rollout` is skipped when the run's `routing_bucket` falls
outside its range; a run with no bucket is admitted by every rollout. See
[Workflows](./workflow.md#workflow-lifecycle).

## Workflow Conditions

A workflow's condition decides whether it runs. The engine evaluates it
against the **full message context** (`data`, `metadata`, and `temp_data`):

```jsonc
{
    "id": "premium_order",
    "name": "Premium Order Processing",
    "condition": { ">=": [{"var": "data.order.total"}, 1000] },
    "tasks": [...]
}
```

Only a literal `true` selects the workflow: `1` or a non-empty string does
not. A condition that fails to evaluate counts as false; the `Run` reports
that workflow as `Selection::ConditionFailed(error)`, and nothing is recorded
on the message.

## Host Functions

Register host functions on the builder. `.register("name", f)` accepts any
[`Function`](../advanced/custom-functions.md); the engine compiles each task's
`input` into the function's typed `Config` at build time, so a mis-shaped
config fails `build()` rather than the first message.

```rust
use dataflow_rs::{Ctx, Engine, Function, Outcome, Result};
use serde_json::json;

/// A function that takes no input and writes nothing.
struct Ping;

impl Function for Ping {
    type Config = ();

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

# fn main() -> std::result::Result<(), Vec<dataflow_rs::Issue>> {
let engine = Engine::builder()
    .register("ping", Ping)
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "t", "function": {"name": "ping"}}
    ]}))
    .build()?;
# let _ = engine;
# Ok(()) }
```

A host function's writes go to an overlay of its own and reach the message
only when the call returns `Ok`; an `Err` drops them and keeps the records
the call added. The eight built-in names are reserved: registering one
panics.

## Thread Safety

The engine is `Send + Sync`, and `Clone` is an `Arc` bump: clones share the
program and the config. Each message is processed independently, so a server
runs many messages at once:

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, messages: Vec<Message>) {
let handles: Vec<_> = messages
    .into_iter()
    .map(|mut message| {
        let engine = engine.clone();
        tokio::spawn(async move {
            let run = engine.run(&mut message).await;
            (message, run)
        })
    })
    .collect();

for handle in handles {
    let (message, run) = handle.await.expect("the task ran");
    println!("{}: {:?}", message.id(), run.status());
}
# }
```

The run future is `Send`, host calls and layers included.

## Reloading Workflows

A program is immutable. To change the workflows, compile a new program with
the engine's own compiler, which carries the same registry (functions,
layers, operators and secrets) and compile options, and pair it with the
engine's config through `with_program`:

```rust
# use dataflow_rs::Engine;
# use serde_json::Value;
# fn _demo(engine: Engine, new_definitions: Vec<Value>) -> Result<(), Vec<dataflow_rs::Issue>> {
use dataflow_rs::def::{ParseOptions, parse};

let compiler = engine.compiler();
let mut workflows = Vec::new();
for raw in &new_definitions {
    let parsed = parse(raw, ParseOptions::default());
    let def = parsed.def.ok_or(parsed.issues)?;
    workflows.push(compiler.compile_one(&def)?.workflow);
}
let reloaded = engine.with_program(compiler.link(workflows));

// `engine` is unchanged and still serves the runs in flight; `reloaded`
// runs the new program with the same observer and error context.
# let _ = reloaded;
# Ok(()) }
```

A bad definition surfaces as issues here, before anything is swapped, so a
broken reload never replaces a working engine.

## API Reference

### `Engine::builder()`

Returns an `EngineBuilder`. Chain `.workflow_json(&value)`,
`.workflow_json_owned(value)`, `.workflow(def)`, `.register("name", function)`,
`.layer(layer)`, `.registry(registry)`, `.options(CompileOptions)`,
`.config(EngineConfig)`, then `.build() -> Result<Engine, Vec<Issue>>`.

### `EngineConfig`

What an engine is configured with beside its program; a reload keeps it.
`EngineConfig::default().observer(Arc<dyn Observer>)` reports every run to
an observer; `.error_context(ErrorContext::new(path, limit)?)` mirrors each
failure's code into the context at `path`, keeping the newest `limit`
records. See [Error Handling](./error-handling.md#branching-on-task-failures).

### `Engine::new(program, config)`

An engine running `program` with `config`. The program comes from a
`Compiler`: `Registry::default().compiler(CompileOptions::default())`, then
`compile_one` per definition and `link`.

### `engine.run(&mut message)` and `engine.run_with(&mut message, &opts)`

Run a message through the program's active workflows, those of
`opts.channel` when it names one, in priority order. Both return a `Run`.
The message is modified in place: its context, audit trail and errors.

### `engine.compiler()`

A compiler with the program's registry and options, whose workflows link into
a program this engine can run.

### `engine.with_program(program)`

An engine running `program` with this engine's config: a reload. This engine
is unchanged.

### `engine.program()`

The program this engine runs. `program().workflows()` lists every linked
workflow (`id`, `channel`, `priority`, `status`, `version`), the paused and
archived ones too, and `program().tasks()` every task.

```rust
# fn _demo(engine: dataflow_rs::Engine) {
for workflow in engine.program().workflows() {
    println!("{} on {} at priority {}", workflow.id, workflow.channel, workflow.priority);
}
# }
```

## From 3.x

In 3.x the engine was built from `Workflow` values and run with the
`process_message` family (`process_message_with_trace`,
`process_message_tracing`, `process_message_for_channel` and their twins),
observed through `ExecutionObserver`'s five callbacks, traced into an
`ExecutionTrace` of snapshots, and reloaded with `with_new_workflows`. In 4.0
every run goes through `run` or `run_with` and comes back as a `Run`; the
channel, the trace and the observer are run options; the trace holds changes
rather than snapshots; and a reload is `compiler()`, `link` and
`with_program`. See [Migrating from 3.x](../migration/from-3.md).
