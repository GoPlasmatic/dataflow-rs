# Quick Start

Build and run your first workflow in a few minutes.

> **Prerequisite reading:** Workflow conditions and mappings use
> [JSONLogic](../advanced/jsonlogic.md). If you are new to JSONLogic, skim the
> *Data Access*, *Comparison Operators*, and *Conditionals* sections first—they
> cover everything used in the examples below.

## Create a Simple Workflow

Workflows are defined in JSON. Each workflow contains an ordered list of tasks that process a message.

```rust
use dataflow_rs::{Engine, Message};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define a workflow that copies the incoming payload into `data.input`
    // and then constructs a greeting. Because `payload` is outside the
    // JSONLogic evaluation context, starting with `parse_json` is the
    // standard pattern.
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

    // The builder parses, compiles, and links every workflow definition.
    // If any definition has an error (such as a typo or unknown function),
    // `build()` fails upfront with detailed issues rather than failing at runtime.
    let engine = match Engine::builder().workflow_json(&workflow).build() {
        Ok(engine) => engine,
        Err(issues) => {
            for issue in &issues {
                eprintln!("{issue}");
            }
            return Err("the workflow was rejected".into());
        }
    };

    // Create a message from a JSON payload. When the workflow runs,
    // `parse_json` copies this payload into `data.input`.
    let mut message = Message::from_json(&json!({"name": "World"}));

    // Run the message through the engine. `run` always returns a `Run` report.
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

Running this program prints:

```text
status: Completed
data: {"input":{"name":"World"},"greeting":"Hello, World!"}
```

## Try It Interactively

> Open the [Full Debugger UI](/dataflow-rs/debugger/) for step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"greeting","name":"Greeting","tasks":[{"id":"load","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"greet","name":"Create Greeting","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"data.input.name"},"!"]}}]}}}]}]' data-payload='{"name":"World"}'>
</div>

## How It Works

1. **Workflow Definition** — A JSON object with a unique `id` and a list of `tasks` to execute.
2. **Engine Creation** — `Engine::builder()` parses and validates each definition, compiles all JSONLogic expressions and function configs, and links them into an immutable program.
3. **Message Creation** — `Message::from_json` places your input in `payload`, while the evaluation context (`data`, `metadata`, `temp_data`) starts empty.
4. **Execution** — `engine.run(&mut message).await` evaluates each active workflow's condition in priority order and runs the tasks of matching workflows.
5. **Inspecting Results** — `Run` reports the final `RunStatus` and per-workflow execution records, while `Message` holds the updated context, audit trail, and any recorded errors.

## Add Validation

Next, add a `validation` task before the `map` task to ensure `data.input.name` is present:

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

When a `validation` rule fails, it records a `VALIDATION_ERROR` on `message.errors()` and returns status `400` (a **rejection**). Because a task's default `on_rejection` policy is `"continue"`, omitting `"on_rejection": "halt"` would allow `greet` to run even when validation fails. Setting `"on_rejection": "halt"` stops the current workflow immediately upon rejection while still allowing later workflows to run.

Here is what happens when you run that workflow with an empty payload:

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

// The run finished, and recorded the validation error on the message.
assert!(matches!(run.status(), RunStatus::CompletedWithErrors));
let record = &message.errors()[0];
assert_eq!(record.code, "VALIDATION_ERROR");
assert_eq!(record.message, "Name is required");

// The workflow halted at `validate_input`, so `greet` never ran.
let workflow = run.workflows().next().expect("one workflow was considered");
assert_eq!(workflow.exit, Some(WorkflowExit::Halted));
assert!(message.context().get(&"data.greeting".parse().unwrap()).is_none());
# }
```

> **Why is `parse_json` needed first?** `payload` is not part of the JSONLogic evaluation context (`data`, `metadata`, `temp_data`). Without `parse_json`, `{"var": "data.input.name"}` evaluates to `null` (falsy) and validation fails on every message. In addition, `Compiler::check` flags any `{"var": "payload..."}` expression as a `PAYLOAD_VAR` defect.

## Next Steps

- [Basic Concepts](./basic-concepts.md) — Understand the core architecture
- [JSONLogic Reference](../advanced/jsonlogic.md) — Explore supported JSONLogic operators
- [Map Function](../built-in-functions/map.md) — Learn how to transform data
- [Validation](../built-in-functions/validation.md) — Learn how to validate data
