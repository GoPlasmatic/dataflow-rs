//! # Error Handling
//!
//! A run reports on **two channels**, and a host checks both:
//!
//! - `Run::status()` says how the run ended: `Completed`,
//!   `CompletedWithErrors`, or `Stopped(error)` when a task's failure ended
//!   it early. (`TimedOut` and `Cancelled` are the run's own limits.)
//! - `message.errors()` holds every error recorded along the way, the ones
//!   a policy stepped past included.
//!
//! What a task's outcome does is its two policies. `on_rejection` handles a
//! 4xx, such as a failed `validation`; its default, `continue`, records the
//! failure and runs the next task. `on_failure` handles a 5xx or an `Err`;
//! its default, `fail`, stops the run. The same workflow runs under three
//! spellings, and each time the status, the recorded errors and the
//! greeting are printed.
//!
//! Run with: `cargo run --example error_handling`

use dataflow_rs::run::RunStatus;
use dataflow_rs::{Ctx, Engine, Error, Function, Message, Outcome, Result, Run};
use serde_json::{Value, json};

/// A host function standing in for a call to a service that is down: every
/// call fails with the service's own code. A retryable error says a retry
/// may succeed, which the run reports.
struct Notify;

impl Function for Notify {
    type Config = ();

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        Err(
            Error::service("UPSTREAM_DOWN", "the notification service did not answer")
                .with_retryable(true),
        )
    }
}

/// The workflow, with the policies of its two fallible tasks spelled out:
///
/// 1. `load` parses the payload into `data.input`
/// 2. `validate_optional` fails, since the payload has no email; a failed
///    rule returns 400, which `on_rejection` handles
/// 3. `greet` builds the greeting from the payload
/// 4. `notify` calls the service that is down, an `Err`, which `on_failure`
///    handles
fn workflow(on_rejection: &str, on_failure: &str) -> Value {
    json!({
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
                "name": "Optional validation",
                "on_rejection": on_rejection,
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
            },
            {
                "id": "notify",
                "name": "Notify the service",
                "on_failure": on_failure,
                "function": { "name": "notify", "input": {} }
            }
        ]
    })
}

/// Channel 1: how the run ended. `RunStatus` can grow, so the match keeps a
/// catch-all arm.
fn describe(run: &Run) -> String {
    match run.status() {
        RunStatus::Completed => "ran to completion, no errors".to_owned(),
        RunStatus::CompletedWithErrors => "ran to completion, with errors recorded".to_owned(),
        RunStatus::Stopped(e) => format!(
            "stopped early: {e} [{}], retryable: {}",
            e.code(),
            run.retryable()
        ),
        RunStatus::TimedOut => "timed out".to_owned(),
        RunStatus::Cancelled => "cancelled".to_owned(),
        _ => "ended another way".to_owned(),
    }
}

/// Builds the workflow under the two policies, runs the email-less payload
/// through it, and prints both channels and the greeting.
async fn scenario(
    title: &str,
    on_rejection: &str,
    on_failure: &str,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    println!("{title}");
    println!("{}", "-".repeat(title.len()));
    println!("  validate_optional: on_rejection = {on_rejection}");
    println!("  notify:            on_failure   = {on_failure}");

    let engine = Engine::builder()
        .workflow_json(&workflow(on_rejection, on_failure))
        .register("notify", Notify)
        .build()
        .map_err(|issues| {
            issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        })?;

    // The payload deliberately omits `email`, so the validation fails.
    let mut message = Message::from_json(&json!({"name": "World"}));

    // Channel 1: the `Run`. Every run returns one; `status()` says how it
    // ended, and the workflow's record says how the workflow did.
    let run = engine.run(&mut message).await;
    println!("  run:      {}", describe(&run));
    for record in run.workflows() {
        match record.exit {
            Some(exit) => println!("  workflow: {} exited {exit:?}", record.id),
            None => println!("  workflow: {} did not run", record.id),
        }
    }

    // Channel 2: the errors recorded on the message. Always check this, even
    // when the run completed: a policy that continued past a failure left
    // its record here. A validation rule's record carries no task id, as
    // in 3.x.
    if message.errors().is_empty() {
        println!("  errors:   none recorded");
    } else {
        println!("  errors:");
        for err in message.errors() {
            println!(
                "    [{}/{}] {} [{}]",
                err.workflow_id.as_deref().unwrap_or("-"),
                err.task_id.as_deref().unwrap_or("-"),
                err.message,
                err.code,
            );
        }
    }

    // Whether the `greet` task was reached.
    let data = Value::from(message.context().data());
    match data["greeting"].as_str() {
        Some(greeting) => println!("  greeting: {greeting}"),
        None => println!("  greeting: not built, the workflow ended before `greet`"),
    }
    println!();
    Ok(())
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Both policies step past the failures: the run completes, every error
    // is on the message, and the greeting is built.
    scenario(
        "Scenario 1: continue past the rejection and the failure",
        "continue",
        "continue",
    )
    .await?;

    // `on_rejection: halt` ends the workflow at the failed validation, on
    // purpose: `greet` and `notify` never run, the run completes with the
    // validation's record, and nothing else is recorded.
    scenario(
        "Scenario 2: halt the workflow on the rejection",
        "halt",
        "continue",
    )
    .await?;

    // `on_failure: fail`, the default, stops the run at `notify`: the status
    // carries the error, `greet` had already run, and the record is on the
    // message too.
    scenario(
        "Scenario 3: fail the run on the failure (the default)",
        "continue",
        "fail",
    )
    .await?;

    Ok(())
}
