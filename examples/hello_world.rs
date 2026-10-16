//! # Hello World
//!
//! The smallest possible dataflow-rs program. `parse_json` copies the
//! payload into `data.input`, then `map` builds a greeting from it, and the
//! greeting is printed: `"Hello, World!"`.
//!
//! Run with: `cargo run --example hello_world`

use dataflow_rs::{Engine, Message};
use serde_json::{Value, json};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workflow = json!({
        "id": "hello",
        "name": "Hello World",
        "tasks": [
            {
                "id": "load",
                "name": "Load payload into data.input",
                "function": {
                    "name": "parse_json",
                    "input": { "source": "payload", "target": "input" }
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
    });

    // The builder parses and compiles the definition, every expression
    // included. A definition it rejects fails the build with its issues, so
    // nothing malformed waits for the first message to fail.
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

    // The payload is not part of the evaluation context, which is only
    // `data`, `metadata` and `temp_data`: the `parse_json` task moves it
    // into `data` first.
    let mut message = Message::from_json(&json!({"name": "World"}));

    // A run always returns a `Run`, whatever happened. `into_result` turns a
    // run the engine stopped early into an `Err`.
    engine.run(&mut message).await.into_result()?;

    let data = Value::from(message.context().data());
    println!("{}", data["greeting"]);
    Ok(())
}
