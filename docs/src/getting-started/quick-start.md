# Quick Start

Build your first workflow in minutes.

> **Prerequisite reading:** conditions and mappings are written in
> [JSONLogic](../advanced/jsonlogic.md). If you've never used it, skim the
> *Data Access*, *Comparison Operators*, and *Conditionals* sections first;
> they cover everything in the examples below.

## Create a Simple Workflow

You define workflows in JSON. A workflow is a list of tasks that run in order
over a message.

```rust
use dataflow_rs::{Engine, Message};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A workflow that loads the payload into `data.input` and then
    // transforms it. Letting `parse_json` seed `data` is the idiomatic
    // pattern: the payload itself is not part of the evaluation context.
    let workflow = json!({
        "id": "greeting",
        "name": "Greeting",
        "tasks": [
            {
                "id": "load",
                "function": {
                    "name": "parse_json",
                    "input": {"source": "payload", "target": "input"}
                }
            },
            {
                "id": "greet",
                "function": {
                    "name": "map",
                    "input": {"mappings": [
                        {
                            "path": "data.greeting",
                            "logic": {"cat": ["Hello, ", {"var": "data.input.name"}, "!"]}
                        }
                    ]}
                }
            }
        ]
    });

    // The builder parses, compiles and links every definition. It fails
    // with every issue of every rejected definition, so a typo or an
    // unknown function surfaces here, never on the first message.
    let engine = match Engine::builder().workflow_json(&workflow).build() {
        Ok(engine) => engine,
        Err(issues) => {
            for issue in &issues {
                eprintln!("{issue}");
            }
            return Err("the workflow was rejected".into());
        }
    };

    // A message from a JSON payload. `parse_json` copies it into
    // `data.input` when the workflow runs.
    let mut message = Message::from_json(&json!({"name": "World"}));

    // Run the message. A `Run` comes back on every outcome.
    let run = engine.run(&mut message).await;

    println!("status: {:?}", run.status());
    println!("data: {}", serde_json::to_string(message.context().data())?);
    # assert!(matches!(run.status(), dataflow_rs::run::RunStatus::Completed));
    # assert_eq!(
    #     serde_json::to_string(message.context().data())?,
    #     r#"{"input":{"name":"World"},"greeting":"Hello, World!"}"#
    # );
    Ok(())
}
```

Running it prints:

```text
status: Completed
data: {"input":{"name":"World"},"greeting":"Hello, World!"}
```

## Try It Interactively

> Open the [Full Debugger UI](/dataflow-rs/debugger/) for step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"greeting","name":"Greeting","tasks":[{"id":"load","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"greet","name":"Create Greeting","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"data.input.name"},"!"]}}]}}}]}]' data-payload='{"name":"World"}'>
</div>

## Understanding the Code

1. **Workflow Definition** - A JSON object with an `id` and the `tasks` to run
2. **Engine Creation** - `Engine::builder()` parses and lints each definition, compiles every expression and function config, and links them into a program; a rejected definition fails `build()` with its issues
3. **Message Creation** - The input is the message's `payload`; its context (`data`, `metadata`, `temp_data`) starts empty
4. **Running** - `engine.run(&mut message).await` evaluates each workflow's condition and runs the tasks of the ones that match
5. **Result** - The `Run` says how it ended and which workflows ran; the message holds the transformed context, an audit entry per task and any errors

## Add Validation

Extend your workflow with data validation:

```json
{
    "id": "validated_greeting",
    "name": "Validated Greeting",
    "tasks": [
        {
            "id": "load",
            "function": {
                "name": "parse_json",
                "input": {"source": "payload", "target": "input"}
            }
        },
        {
            "id": "validate_input",
            "on_rejection": "halt",
            "function": {
                "name": "validation",
                "input": {
                    "rules": [
                        {
                            "logic": {"!!": {"var": "data.input.name"}},
                            "message": "Name is required"
                        }
                    ]
                }
            }
        },
        {
            "id": "greet",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {
                            "path": "data.greeting",
                            "logic": {"cat": ["Hello, ", {"var": "data.input.name"}, "!"]}
                        }
                    ]
                }
            }
        }
    ]
}
```

A failing `validation` records a `VALIDATION_ERROR` with the rule's message and
returns status `400`, a rejection. A task's default `on_rejection` is
`continue`, so without the policy `greet` would still run and greet a message
with no name. `on_rejection: "halt"` ends the workflow at the failed
validation; later workflows still run.

Run the same definition over a payload with no name and the run tells you
what happened:

```rust
# use dataflow_rs::run::{RunStatus, WorkflowExit};
# use dataflow_rs::{Engine, Message};
# use serde_json::json;
# #[tokio::main]
# async fn main() {
# let workflow = json!({
#     "id": "validated_greeting",
#     "tasks": [
#         {"id": "load", "function": {"name": "parse_json",
#          "input": {"source": "payload", "target": "input"}}},
#         {"id": "validate_input", "on_rejection": "halt", "function": {"name": "validation",
#          "input": {"rules": [{"logic": {"!!": {"var": "data.input.name"}},
#                               "message": "Name is required"}]}}},
#         {"id": "greet", "function": {"name": "map", "input": {"mappings": [
#             {"path": "data.greeting",
#              "logic": {"cat": ["Hello, ", {"var": "data.input.name"}, "!"]}}]}}}
#     ]
# });
# let engine = Engine::builder().workflow_json(&workflow).build().expect("the workflow builds");
let mut message = Message::from_json(&json!({}));
let run = engine.run(&mut message).await;

// The run completed, with a record on the message.
assert!(matches!(run.status(), RunStatus::CompletedWithErrors));
let record = &message.errors()[0];
assert_eq!(record.code, "VALIDATION_ERROR");
assert_eq!(record.message, "Name is required");

// The workflow halted at the validation: nothing was greeted.
let workflow = run.workflows().next().expect("one workflow was considered");
assert_eq!(workflow.exit, Some(WorkflowExit::Halted));
assert!(message.context().get(&"data.greeting".parse().unwrap()).is_none());
# }
```

The `load` task is required because `payload` is outside the JSONLogic
evaluation context. Without `parse_json`, `{"var": "data.name"}` resolves to
`null` (which is falsy) and the validation rule fails on every message.
`parse_json` copies the payload to `data.input`, so tasks read `data.input.…`.
`Compiler::check` flags any literal `var` rooted at `payload` as a
`PAYLOAD_VAR` defect.

## Next Steps

- [Basic Concepts](./basic-concepts.md) - Understand the core architecture
- [JSONLogic Reference](../advanced/jsonlogic.md) - Complete operator reference
- [Map Function](../built-in-functions/map.md) - Learn about data transformation
- [Validation](../built-in-functions/validation.md) - Learn about data validation
