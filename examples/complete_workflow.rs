//! # Complete Workflow Example
//!
//! A complete workflow with data transformation and validation, in the
//! recommended shape:
//!
//! 1. `parse_json` loads the payload into the `data` context
//! 2. `map` transforms it into the structure the host wants
//! 3. `validation` checks the transformed data
//!
//! A well-formed user record goes through first: the run's status, the
//! transformed data and the audit trail are printed. Then a record with no
//! email goes through the same engine, and the validation errors it records
//! are printed beside the status.
//!
//! Run with: `cargo run --example complete_workflow`

use dataflow_rs::run::RunStatus;
use dataflow_rs::{Engine, Message, Run};
use serde_json::{Value, json};

/// A line for how a run ended. `RunStatus` can grow, so the match keeps a
/// catch-all arm.
fn describe(run: &Run) -> String {
    match run.status() {
        RunStatus::Completed => "completed".to_owned(),
        RunStatus::CompletedWithErrors => "completed, with errors recorded".to_owned(),
        RunStatus::Stopped(e) => format!("stopped early: {e} [{}]", e.code()),
        RunStatus::TimedOut => "timed out".to_owned(),
        RunStatus::Cancelled => "cancelled".to_owned(),
        _ => "ended another way".to_owned(),
    }
}

/// Runs `message` and prints the walk-through: the status, the data the
/// transformation left, the audit trail and any recorded error.
async fn process(engine: &Engine, mut message: Message) -> Result<(), Box<dyn std::error::Error>> {
    println!("Processing message through workflow...");
    let run = engine.run(&mut message).await;
    println!("Workflow {}", describe(&run));

    println!("\nTransformed user data:");
    let data = Value::from(message.context().data());
    println!("{}", serde_json::to_string_pretty(&data["user"])?);

    // Every task that ran leaves an audit entry with its status and, under
    // the default `capture_changes`, the writes it made.
    println!("\nAudit trail:");
    for entry in message.audit() {
        println!(
            "  - Task: {} (Status: {}, Changes: {})",
            entry.task_id,
            entry.status,
            entry.changes.len()
        );
    }

    // A failed validation rule records its message here and returns 400,
    // which the default `on_rejection: continue` steps past: the run
    // completes, with errors.
    if !message.errors().is_empty() {
        println!("\nValidation errors:");
        for err in message.errors() {
            println!("  - {} [{}]", err.message, err.code);
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // Define a workflow that:
    // 1. Parses the payload into the data context (the recommended first step)
    // 2. Transforms the data to our model
    // 3. Validates the transformed data
    let workflow = json!({
        "id": "complete_workflow",
        "name": "Complete Workflow Example",
        "priority": 0,
        "description": "Demonstrates parse -> transform -> validate flow",
        "tasks": [
            {
                "id": "load_payload",
                "name": "Load Payload",
                "description": "Parse JSON payload into data context",
                "function": {
                    "name": "parse_json",
                    "input": {
                        "source": "payload",
                        "target": "input"
                    }
                }
            },
            {
                "id": "transform_data",
                "name": "Transform Data",
                "description": "Map input data to our user model",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "data.user.id",
                                "logic": { "var": "data.input.body.id" }
                            },
                            {
                                "path": "data.user.name",
                                "logic": { "var": "data.input.body.name" }
                            },
                            {
                                "path": "data.user.email",
                                "logic": { "var": "data.input.body.email" }
                            },
                            {
                                "path": "data.user.address",
                                "logic": {
                                    "cat": [
                                        { "var": "data.input.body.address.street" },
                                        ", ",
                                        { "var": "data.input.body.address.city" }
                                    ]
                                }
                            },
                            {
                                "path": "data.user.company",
                                "logic": { "var": "data.input.body.company.name" }
                            }
                        ]
                    }
                }
            },
            {
                "id": "validate_user_data",
                "name": "Validate User Data",
                "description": "Ensure the user data meets our requirements",
                "function": {
                    "name": "validation",
                    "input": {
                        "rules": [
                            {
                                "logic": { "!!": { "var": "data.user.id" } },
                                "message": "User ID is required"
                            },
                            {
                                "logic": { "!!": { "var": "data.user.name" } },
                                "message": "User name is required"
                            },
                            {
                                "logic": { "!!": { "var": "data.user.email" } },
                                "message": "User email is required"
                            },
                            {
                                "logic": {
                                    "in": [
                                        "@",
                                        { "var": "data.user.email" }
                                    ]
                                },
                                "message": "Email must be valid format"
                            }
                        ]
                    }
                }
            }
        ]
    });

    // The built-in functions are always registered; the builder compiles the
    // definition and fails with its issues if it is malformed.
    let engine = Engine::builder()
        .workflow_json(&workflow)
        .build()
        .map_err(|issues| {
            issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        })?;

    // A message with sample user data: the three rules pass.
    let message = Message::from_json(&json!({
        "body": {
            "id": 1,
            "name": "John Doe",
            "email": "john.doe@example.com",
            "address": {
                "street": "123 Main St",
                "city": "New York"
            },
            "company": {
                "name": "Acme Corp"
            }
        }
    }));
    process(&engine, message).await?;

    // The same engine over a record with no email: two rules fail, the
    // errors are recorded, and the run still completes.
    println!("\n{}\n", "=".repeat(60));
    let message = Message::from_json(&json!({
        "body": {
            "id": 2,
            "name": "Jane Roe",
            "address": {
                "street": "9 Side St",
                "city": "Boston"
            },
            "company": {
                "name": "Roe & Co"
            }
        }
    }));
    process(&engine, message).await?;

    Ok(())
}
