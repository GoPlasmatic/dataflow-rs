//! # Hello World
//!
//! The smallest possible dataflow-rs program. `parse_json` copies the
//! payload into `data.input`, then `map` builds a greeting from it.
//! Run with: `cargo run --example hello_world`

use dataflow_rs::prelude::*;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    let workflow = Workflow::from_json(
        r#"{
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
        }"#,
    )?;

    let engine = Engine::builder().with_workflow(workflow).build()?;
    let mut message = Message::from_value(&json!({"name": "World"}));
    engine.process_message(&mut message).await?;

    println!("{}", message.data()["greeting"]);
    Ok(())
}
