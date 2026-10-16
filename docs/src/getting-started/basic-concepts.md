# Basic Concepts

The core components of dataflow-rs.

## The IF → THEN → THAT Model

Dataflow-rs follows an IFTTT-style rules engine pattern:

- **IF**: Define conditions using JSONLogic (evaluated against `data`, `metadata`, `temp_data`)
- **THEN**: Execute tasks: data transformation, validation, or host functions
- **THAT**: Chain multiple tasks and workflows with priority ordering

## Architecture Overview

Dataflow-rs has two phases:

1. **Build** - A definition is parsed and linted, every expression and
   function config is compiled, and the workflows are linked into a program
2. **Run** - The engine runs messages over the compiled program

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
| **Workflow** | `WorkflowDef` | A condition plus a list of tasks: IF the condition holds, THEN run the tasks |
| **Task** | `TaskDef` | One function call, with its own condition and outcome policies |
| **Registry** | `Registry` | The functions a program may call: the eight built-ins plus the host's |
| **Program** | `Program` | The compiled, immutable workflows an engine runs |
| **Engine** | `Engine` | Runs messages over a program, with its config |
| **Message** | `Message` | What flows through: a payload, the context, an audit trail and errors |
| **Run** | `Run` | What a run reports: its status, which workflows matched, and what each did |

### Engine

The engine runs messages through the program it was built with.

```rust
# use dataflow_rs::Message;
# use serde_json::Value;
# async fn _demo(definitions: Vec<Value>, mut message: Message)
#     -> Result<(), Vec<dataflow_rs::Issue>> {
use dataflow_rs::Engine;

// Every definition is parsed, compiled and linked here. `build` fails with
// every issue of every rejected definition. `.register("name", function)`
// adds a host function before the definitions that call it compile.
let mut builder = Engine::builder();
for definition in &definitions {
    builder = builder.workflow_json(definition);
}
let engine = builder.build()?;

// Run a message. The `Run` says how it ended and what each workflow did.
let run = engine.run(&mut message).await;
println!("{} workflow(s) matched", run.matched());
# Ok(()) }
```

### Workflow

A list of tasks that run in order. A workflow has:

- **Priority** - Determines execution order across workflows (lower runs first)
- **Condition** - JSONLogic evaluated against the full context (`data`, `metadata`, `temp_data`); absent means the workflow always runs
- **Failure policy** - `on_failure` says whether a failure that reaches the workflow stops the run (`fail`, the default) or lets the next workflow run (`continue`)

```json
{
    "id": "premium_order",
    "name": "Premium Order Processing",
    "priority": 1,
    "condition": { ">=": [{"var": "data.order.total"}, 1000] },
    "tasks": [...]
}
```

### Task

An individual processing unit within a workflow. A task:

- Calls one function: a built-in (`map`, `validation`, `filter`, `log`, `parse_json`, `parse_xml`, `publish_json`, `publish_xml`) or a host function registered under its name
- Can have a condition for conditional execution
- Says what its outcome does: `on_failure` for a failure (a `5xx` or an `Err`), `on_rejection` for a rejection (a `4xx`)

```json
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

The data structure that flows through workflows. It carries:

- **id** - A UUID v7 unless you set one
- **payload** - The body as the engine received it. Read-only, and **not** part
  of the JSONLogic evaluation context; a `parse_json` task copies it into
  `data`
- **context.data** - Main data payload
- **context.metadata** - Message metadata; the engine stamps `processed_at`,
  `engine_version` and `progress` here
- **context.temp_data** - Temporary processing data
- **audit_trail** - One entry per task that ran, with the writes it made
- **errors** - Every error recorded during processing

```rust
use dataflow_rs::Message;
use serde_json::json;

// `from_json` sets the *payload*. The context starts as three empty roots;
// a `parse_json` task is what lands the payload in `data`.
let message = Message::from_json(&json!({
    "name": "John",
    "email": "john@example.com"
}));

// After a run, read the context, the audit trail and the errors.
println!("data: {:?}", message.context().data());
println!("audit entries: {}", message.audit().len());
println!("errors: {}", message.errors().len());
# assert!(message.audit().is_empty() && message.errors().is_empty());
```

### Run

`engine.run(&mut message).await` returns a `Run` on every outcome, so there is
no `Err` to match before you can read what happened:

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

1. **Input** - A message is created with a payload
2. **Workflow Selection** - The engine evaluates each active workflow's condition, in priority order
3. **Task Execution** - Tasks run in order within each matching workflow
4. **Output** - The message holds the transformed context, its audit trail and its errors; the `Run` says how it ended

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

- **Conditions** - Control when workflows and tasks execute (can access any context field)
- **Data Access** - Read values from the message context
- **Transformations** - Transform and combine data

Common operations:

```json
// Access data
{"var": "data.name"}

// String concatenation
{"cat": ["Hello, ", {"var": "data.name"}]}

// Conditionals
{"if": [{"var": "data.premium"}, "VIP", "Standard"]}

// Comparisons
{">=": [{"var": "data.order.total"}, 1000]}
```

## Next Steps

- [Engine](../core-concepts/engine.md) - The engine in detail
- [JSONLogic](../advanced/jsonlogic.md) - Advanced JSONLogic usage
- [Custom Functions](../advanced/custom-functions.md) - Extend with host functions
