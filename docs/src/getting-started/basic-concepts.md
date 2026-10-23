# Basic Concepts

An overview of the core building blocks in `dataflow-rs`.

## The IF → THEN → THAT Model

Dataflow-rs follows an IFTTT-style rules engine pattern:

- **IF**: Evaluate JSONLogic conditions against `data`, `metadata`, and `temp_data`.
- **THEN**: Execute tasks such as data transformation, validation, or custom async host functions.
- **THAT**: Chain multiple tasks and workflows in priority order so later steps build on earlier writes.

## Architecture Overview

Dataflow-rs operates in two distinct phases:

1. **Build** — `def::parse` parses and lints JSON definitions, `Compiler` compiles expressions and function configs, and `link` assembles an immutable `Program`.
2. **Run** — `Engine` processes messages against the compiled program.

```text
┌──────────────────────────────────────────────────────────────────┐
│                            Build                                 │
│  ┌──────────┐   ┌───────┐   ┌──────────┐   ┌─────────┐   ┌─────┐ │
│  │ JSON     │ → │ parse │ → │ Compiler │ → │ Program │ → │Engine│ │
│  │ (def)    │   │ +lint │   │          │   │         │   │     │ │
│  └──────────┘   └───────┘   └──────────┘   └─────────┘   └─────┘ │
│                   issues       issues                            │
└──────────────────────────────────────────────────────────────────┘
                              │
                              v
┌──────────────────────────────────────────────────────────────────┐
│                             Run                                  │
│  ┌─────────┐    ┌───────────────┐    ┌─────────────────────────┐ │
│  │ Message │ →  │ engine.run()  │ →  │ Run (how it ended) and  │ │
│  └─────────┘    └───────────────┘    │ the message (context,   │ │
│                                      │ audit trail, errors)    │ │
│                                      └─────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

## Core Components

| Component | Type | Description |
|---|---|---|
| **Workflow** | `WorkflowDef` | A condition plus a list of tasks: IF the condition matches, THEN the tasks run |
| **Task** | `TaskDef` | A single function call with an optional condition and outcome policies |
| **Registry** | `Registry` | Holds the functions a program can call (the 8 built-ins plus your custom host functions) |
| **Program** | `Program` | The compiled, immutable set of workflows executed by an engine |
| **Engine** | `Engine` | Executes messages against a `Program` using an `EngineConfig` |
| **Message** | `Message` | Carries the input `payload`, the `{data, metadata, temp_data}` context, the audit trail, and errors |
| **Run** | `Run` | Reports the outcome of a single execution: final status, matched workflows, and optional trace |

### Engine

The `Engine` processes messages through its compiled program.

```rust
# use dataflow_rs::Message;
# use serde_json::Value;
# async fn _demo(definitions: Vec<Value>, mut message: Message)
#     -> Result<(), Vec<dataflow_rs::Issue>> {
use dataflow_rs::Engine;

// Parse, compile, and link all workflow definitions. `build()` returns
// all issues if any definition is rejected. Use `.register("name", func)`
// to register custom host functions before building.
let mut builder = Engine::builder();
for definition in &definitions {
    builder = builder.workflow_json(definition);
}
let engine = builder.build()?;

// Process a message. `Run` describes how execution finished and what each workflow did.
let run = engine.run(&mut message).await;
println!("{} workflow(s) matched", run.matched());
# Ok(()) }
```

### Workflow

A workflow is an ordered sequence of tasks that run when its condition matches:

- **Priority** — Controls execution order across workflows (lower numbers run first).
- **Condition** — A JSONLogic expression evaluated against the full message context (`data`, `metadata`, `temp_data`). If omitted, the workflow always runs.
- **Failure Policy** — `on_failure` determines whether an unhandled task failure stops the entire run (`"fail"`, the default) or continues to the next workflow (`"continue"`).

```jsonc
{
    "id": "premium_order",
    "name": "Premium Order Processing",
    "priority": 1,
    "condition": { ">=": [{"var": "data.order.total"}, 1000] },
    "tasks": [...]
}
```

### Task

A task is an individual processing step within a workflow:

- Calls a single function—either one of the 8 built-ins (`map`, `validation`, `filter`, `log`, `parse_json`, `parse_xml`, `publish_json`, `publish_xml`) or a custom host function registered by name.
- Optionally specifies a `condition` to control whether the task runs.
- Configures outcome policies: `on_failure` for failures (`5xx` status or `Err`) and `on_rejection` for rejections (`4xx` status).

```jsonc
{
    "id": "apply_discount",
    "name": "Apply Discount",
    "condition": { "!!": {"var": "data.order.total"} },
    "function": {
        "name": "map",
        "input": { ... }
    }
}
```

### Message

The `Message` is the data container passed through the engine:

- **`id`** — Unique message ID (defaults to a generated UUID v7).
- **`payload`** — The raw input body received by the engine. It is read-only and separate from the JSONLogic evaluation context; use a `parse_json` task to copy it into `data`.
- **`context.data`** — Primary business data read and modified by tasks.
- **`context.metadata`** — Message and execution metadata (including engine stamps like `processed_at`, `engine_version`, and `progress`).
- **`context.temp_data`** — Scratch space for intermediate values shared between tasks.
- **`audit_trail`** — Chronological log of executed tasks and the field changes they made.
- **`errors`** — List of all errors recorded during execution.

```rust
use dataflow_rs::Message;
use serde_json::json;

// `from_json` sets the message *payload*. The three context roots (`data`,
// `metadata`, `temp_data`) start empty until populated by a task like `parse_json`.
let message = Message::from_json(&json!({
    "name": "John",
    "email": "john@example.com"
}));

// After a run, inspect the context, audit trail, and recorded errors.
println!("data: {:?}", message.context().data());
println!("audit entries: {}", message.audit().len());
println!("errors: {}", message.errors().len());
# assert!(message.audit().is_empty() && message.errors().is_empty());
```

### Run

`engine.run(&mut message).await` always returns a `Run` value, whether execution succeeded, recorded validation errors, stopped on a task failure, or hit a timeout:

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message) {
use dataflow_rs::run::RunStatus;

let run = engine.run(&mut message).await;
match run.status() {
    RunStatus::Completed => println!("every workflow finished, no errors"),
    RunStatus::CompletedWithErrors => println!("finished; see message.errors()"),
    RunStatus::Stopped(error) => println!("a task stopped the run: {error}"),
    RunStatus::TimedOut => println!("the run's deadline passed"),
    RunStatus::Cancelled => println!("the run's token was cancelled"),
    _ => {}
}
for workflow in run.workflows() {
    println!("{}: {:?} -> {:?}", workflow.id, workflow.selection, workflow.exit);
}
# }
```

## Data Flow

1. **Input** — Construct a `Message` with your input payload.
2. **Workflow Selection** — The engine evaluates active workflow conditions in priority order.
3. **Task Execution** — Matching workflows run their tasks sequentially.
4. **Output** — `Message` contains the updated context, audit trail, and error list, while `Run` reports how execution completed.

```text
Message (input)
    │
    v
┌─────────────────────────────────────────┐
│ Workflow 1 (priority: 1)                │
│   Task 1 -> Task 2 -> Task 3            │
└─────────────────────────────────────────┘
    │
    v
┌─────────────────────────────────────────┐
│ Workflow 2 (priority: 2)                │
│   Task 1 -> Task 2                      │
└─────────────────────────────────────────┘
    │
    v
Message (output with audit trail) + Run
```

## JSONLogic

Dataflow-rs uses [JSONLogic](https://jsonlogic.com/) for:

- **Conditions** — Deciding when workflows, task groups, and tasks execute.
- **Data Access** — Reading fields from `data`, `metadata`, and `temp_data`.
- **Transformations** — Computing new values in `map` tasks and custom function configs.

Common examples:

```jsonc
// Read a field from `data`
{"var": "data.name"}

// Concatenate strings
{"cat": ["Hello, ", {"var": "data.name"}]}

// Conditional expression
{"if": [{"var": "data.premium"}, "VIP", "Standard"]}

// Numeric comparison
{">=": [{"var": "data.order.total"}, 1000]}
```

## Next Steps

- [Engine](../core-concepts/engine.md) — Deep dive into building, running, and reloading the engine
- [JSONLogic](../advanced/jsonlogic.md) — Complete JSONLogic operator reference
- [Custom Functions](../advanced/custom-functions.md) — Extend the engine with async Rust functions
