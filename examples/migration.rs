//! # Migration
//!
//! A 3.x host moving to 4.0, in two parts.
//!
//! 1. **The definition.** A 3.x workflow with `continue_on_error`,
//!    `halt_on` and `validate` is read through `compat::migrate_v3`, which
//!    rewrites each 3.x spelling into 4.0's. Every rewrite is printed with
//!    its path, then the definition in its 4.0 spelling, and the migrated
//!    definition is built and run. `Engine::builder` reads the 3.x spelling
//!    too, through the same reader; `migrate_v3` is for rewriting the files
//!    once.
//! 2. **The handler.** A 3.x `AsyncFunctionHandler` written as a 4.0
//!    `Function` with a derived config, the 3.x version kept beside it as a
//!    comment, run once through `function::testing::call`.
//!
//! Run with: `cargo run --example migration`

use dataflow_rs::compat::migrate_v3;
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::Output;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::function::testing::{self, Write};
use dataflow_rs::{Ctx, Engine, Expr, Function, Message, Outcome, Result};
use serde_json::{Value, json};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Part 2's handler, before and after
// ---------------------------------------------------------------------------

// 3.x: the handler as `examples/async_migration.rs` had it. The config was
// an empty serde struct, the destination was a string spelled in the
// handler, `ctx.set` could not fail, and the trait needed `async_trait`.
//
//     use async_trait::async_trait;
//     use dataflow_rs::{AsyncFunctionHandler, Result, TaskContext, TaskOutcome};
//     use datavalue::OwnedDataValue;
//     use serde::Deserialize;
//     use serde_json::json;
//
//     #[derive(Debug, Deserialize, Default)]
//     struct EmptyInput {}
//
//     struct AsyncHttpHandler;
//
//     #[async_trait]
//     impl AsyncFunctionHandler for AsyncHttpHandler {
//         type Input = EmptyInput;
//
//         async fn execute(
//             &self,
//             ctx: &mut TaskContext<'_>,
//             _input: &EmptyInput,
//         ) -> Result<TaskOutcome> {
//             // Simulate async HTTP call
//             tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
//
//             ctx.set(
//                 "data.http_response",
//                 OwnedDataValue::from(&json!({
//                     "status": "success",
//                     "data": "Response from API"
//                 })),
//             );
//             Ok(TaskOutcome::Success)
//         }
//     }

// 4.0: the same handler as a `Function`. What moved:
//
// - `type Input`, a serde struct, is `type Config`, a struct with
//   `#[derive(FunctionConfig)]`: each field is compiled at build, a literal
//   folded once, an expression evaluated per call.
// - `TaskContext` is `Ctx`; `TaskOutcome::Success` is `Outcome::Ok`.
// - The destination is an `Output` field of the config, so the task chooses
//   it, and the write goes through `ctx.emit_default`, which can fail (a
//   root takes only an object) and so takes a `?`.
// - No `#[async_trait]`: `call` is an `async fn` on the trait itself.

/// `async_http`'s input.
#[derive(FunctionConfig)]
struct HttpConfig {
    /// The URL to call.
    url: Expr<String>,
    /// How long to wait for the answer, in milliseconds.
    #[config(default = 5000)]
    timeout_ms: Expr<u64>,
    /// Where the response is written.
    #[output(default = "data.http_response")]
    output: Output,
}

/// Calls an HTTP service, simulated.
struct HttpCall;

impl Function for HttpCall {
    type Config = HttpConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &HttpConfig) -> Result<Outcome> {
        let url = ctx.eval(&cfg.url)?;
        let timeout_ms = ctx.eval(&cfg.timeout_ms)?;

        // A real client awaits its response here, under the timeout.
        tokio::time::sleep(Duration::from_millis(10)).await;

        let response = json!({
            "status": "success",
            "url": &*url,
            "timeout_ms": timeout_ms,
            "data": "Response from API"
        });
        ctx.emit_default(OwnedDataValue::from(&response))?;
        Ok(Outcome::Ok)
    }
}

/// The 3.x example's second handler: no input, one flag written. A function
/// with nothing to configure takes `()` as its config, which accepts `{}`
/// or no `input` at all.
struct SimpleAsync;

impl Function for SimpleAsync {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        // A write at a path the function chooses: the path is parsed, and
        // the write recorded, as an `Output`'s would be.
        ctx.set(&"data.processed".parse()?, OwnedDataValue::Bool(true))?;
        Ok(Outcome::Ok)
    }
}

// ---------------------------------------------------------------------------
// Part 1's definition
// ---------------------------------------------------------------------------

/// The 3.x example's workflow, with the 3.x policy spellings on it:
/// `continue_on_error` on the workflow and on a task, `halt_on` on the
/// validation, and the validation called by its 3.x alias, `validate`.
fn v3_workflow() -> Value {
    json!({
        "id": "async_workflow",
        "name": "Async Processing Workflow",
        "priority": 0,
        "continue_on_error": true,
        "tasks": [
            {
                "id": "load_payload",
                "name": "Load Payload",
                "function": {
                    "name": "parse_json",
                    "input": { "source": "payload", "target": "input" }
                }
            },
            {
                "id": "validate",
                "name": "Validate Input",
                "halt_on": "failure",
                "function": {
                    "name": "validate",
                    "input": {
                        "rules": [
                            {
                                "logic": {"!!": [{"var": "data.input.required_field"}]},
                                "message": "Required field is missing"
                            }
                        ]
                    }
                }
            },
            {
                "id": "transform",
                "name": "Transform Data",
                "continue_on_error": true,
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            { "path": "data.transformed", "logic": {"var": "data.input.value"} }
                        ]
                    }
                }
            },
            {
                "id": "http_call",
                "name": "Make HTTP Call",
                "function": {
                    "name": "async_http",
                    "input": { "url": "https://api.example.com/items" }
                }
            },
            {
                "id": "simple_process",
                "name": "Simple Processing",
                "function": { "name": "simple_async", "input": {} }
            }
        ]
    })
}

fn issues_text(issues: &[dataflow_rs::Issue]) -> String {
    issues
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    println!("Part 1: the definition");
    println!("======================\n");

    // `migrate_v3` parses with the compat reader on and writes the
    // definition back in the 4.0 spelling. It is `Err` only when the
    // definition is rejected outright; the rewrites it made, and every
    // advisory, come back beside the JSON.
    let migrated = migrate_v3(&v3_workflow()).map_err(|issues| issues_text(&issues))?;

    println!("Rewrites, in document order:");
    for rewrite in &migrated.rewrites {
        let path = rewrite.path.to_string();
        println!("  {path:<32} {}  ->  {}", rewrite.v3, rewrite.v4);
    }

    println!("\nIssues the reader reported (none rejected):");
    for issue in &migrated.issues {
        println!("  {} {issue}", issue.severity);
    }

    // Only the 4.0 keys are written, and a key holding its default is left
    // out.
    println!("\nThe definition in its 4.0 spelling:");
    println!("{}", serde_json::to_string_pretty(&migrated.json)?);

    // The migrated definition builds as any 4.0 definition does, with the
    // two host functions the workflow names.
    let engine = Engine::builder()
        .workflow_json(&migrated.json)
        .register("async_http", HttpCall)
        .register("simple_async", SimpleAsync)
        .build()
        .map_err(|issues| issues_text(&issues))?;

    let mut message = Message::from_json(&json!({
        "required_field": "present",
        "value": "test data"
    }));
    engine.run(&mut message).await.into_result()?;

    println!("\nRun over the migrated definition, data after the run:");
    let data = Value::from(message.context().data());
    println!("{}", serde_json::to_string_pretty(&data)?);
    println!("Audit trail:");
    for entry in message.audit() {
        println!("  {} -> {}", entry.task_id, entry.status);
    }

    println!("\nPart 2: the handler");
    println!("===================\n");

    // `testing::call` runs one call of a function through the real config
    // compile and the real `Ctx`, with no engine or workflow assembled by
    // hand. What comes back is the outcome, the writes and their `Change`s,
    // and the message after the call's writes landed.
    let called = testing::call(
        HttpCall,
        json!({ "url": "https://api.example.com/items", "timeout_ms": 1500 }),
    )
    .await;

    if !called.issues.is_empty() {
        println!("Config issues:\n{}", issues_text(&called.issues));
    }
    println!("Outcome: {:?}", called.outcome?);
    // Each write as the call made it. `Write` can grow, so the match keeps
    // a catch-all arm.
    println!("Writes, in order:");
    for (path, write) in &called.writes {
        match write {
            Write::Set(value) => println!("  set {path} = {}", Value::from(value)),
            Write::Blocked(value) => println!("  blocked {path} = {}", Value::from(value)),
            Write::Removed => println!("  removed {path}"),
            _ => println!("  {path}: {write:?}"),
        }
    }
    println!("Changes recorded for the audit trail:");
    for change in &called.changes {
        println!(
            "  {}: {} -> {}",
            change.path, change.old_value, change.new_value
        );
    }
    println!("Data after the call:");
    let data = Value::from(called.message.context().data());
    println!("{}", serde_json::to_string_pretty(&data)?);

    println!("\nMigration complete.");

    Ok(())
}
