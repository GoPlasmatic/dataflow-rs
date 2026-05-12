//! # Error Handling
//!
//! Demonstrates dataflow-rs's **dual error channel**:
//!
//! - `Result::Err` from `process_message` signals early-stop (a task failed
//!   without `continue_on_error`, or an engine-level error occurred).
//! - `message.errors()` **always** contains every error encountered,
//!   including ones swallowed by `continue_on_error = true` tasks.
//!
//! New users often only check the `Result` and miss errors recorded on the
//! message. This example shows why you should check both.
//!
//! Run with: `cargo run --example error_handling`

use dataflow_rs::prelude::*;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    // Workflow with three tasks:
    //   1. `load` — parse payload into data.input
    //   2. `validate_optional` — fails validation, but `continue_on_error: true`
    //      means the workflow keeps going; the error lands on `message.errors()`
    //   3. `greet` — succeeds, uses the loaded payload
    //
    // The whole run returns `Ok(())` because no task stopped the workflow,
    // but the validation error is still visible via `message.errors()`.
    let workflow = Workflow::from_json(
        r#"{
            "id": "demo",
            "name": "Error-handling demo",
            "tasks": [
                {
                    "id": "load",
                    "name": "Load payload",
                    "function": {
                        "name": "parse_json",
                        "input": { "source": "payload", "target": "input" }
                    }
                },
                {
                    "id": "validate_optional",
                    "name": "Optional validation (allowed to fail)",
                    "continue_on_error": true,
                    "function": {
                        "name": "validation",
                        "input": {
                            "rules": [
                                {
                                    "logic": { "!!": {"var": "data.input.email"} },
                                    "message": "email is required"
                                }
                            ]
                        }
                    }
                },
                {
                    "id": "greet",
                    "name": "Build greeting",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "data.greeting",
                                    "logic": { "cat": ["Hello, ", {"var": "data.input.name"}, "!"] }
                                }
                            ]
                        }
                    }
                }
            ]
        }"#,
    )?;

    let engine = Engine::builder().with_workflow(workflow).build()?;

    // Payload deliberately omits `email` — the optional validation will fail
    // and record an entry on `message.errors()`, but won't halt the workflow.
    let mut message = Message::from_value(&json!({"name": "World"}));

    // Channel 1: the `Result`. Will be `Ok(())` here because the failing
    // validation had `continue_on_error: true`.
    match engine.process_message(&mut message).await {
        Ok(()) => println!("engine: workflow ran to completion"),
        Err(e) => println!("engine: workflow halted early: {e}"),
    }

    // Channel 2: errors recorded on the message. Always check this, even
    // when the `Result` is `Ok`.
    if message.has_errors() {
        println!("recorded errors:");
        for err in message.errors() {
            println!(
                "  [{workflow}/{task}] {msg}",
                workflow = err.workflow_id.as_deref().unwrap_or("-"),
                task = err.task_id.as_deref().unwrap_or("-"),
                msg = err.message,
            );
        }
    } else {
        println!("no errors recorded");
    }

    // The greeting task still ran:
    println!("greeting: {}", message.data()["greeting"]);

    Ok(())
}
