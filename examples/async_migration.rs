//! # Async Migration Example
//!
//! Demonstrates the v3 typed-handler shape:
//!
//! 1. **Typed Input via `AsyncFunctionHandler::Input`** — config is
//!    deserialized once at startup, not per message.
//! 2. **`TaskContext` for mutation** — `ctx.set("data.x", v)` records audit
//!    changes automatically; no hand-built `Change` entries.
//! 3. **`TaskOutcome` return** — explicit `Success` / `Status(u16)` /
//!    `Skip` / `Halt` instead of a magic-number `usize`.
//!
//! Run with: `cargo run --example async_migration`

use async_trait::async_trait;
use dataflow_rs::{
    AsyncFunctionHandler, BoxedFunctionHandler, Engine, Message, Result, TaskContext, TaskOutcome,
    Workflow,
};
use datavalue::OwnedDataValue;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Deserialize, Default)]
struct EmptyInput {}

// Async HTTP handler — typed Input is empty; the handler is parameterized by
// the message context, not by config.
struct AsyncHttpHandler;

#[async_trait]
impl AsyncFunctionHandler for AsyncHttpHandler {
    type Input = EmptyInput;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &EmptyInput) -> Result<TaskOutcome> {
        // Simulate async HTTP call
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        ctx.set(
            "data.http_response",
            OwnedDataValue::from(&json!({
                "status": "success",
                "data": "Response from API"
            })),
        );
        Ok(TaskOutcome::Success)
    }
}

// Simple async handler — sets a flag.
struct SimpleAsyncHandler;

#[async_trait]
impl AsyncFunctionHandler for SimpleAsyncHandler {
    type Input = EmptyInput;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &EmptyInput) -> Result<TaskOutcome> {
        ctx.set("data.processed", OwnedDataValue::Bool(true));
        Ok(TaskOutcome::Success)
    }
}

fn make_handlers() -> HashMap<String, BoxedFunctionHandler> {
    let mut h: HashMap<String, BoxedFunctionHandler> = HashMap::new();
    h.insert("async_http".to_string(), Box::new(AsyncHttpHandler));
    h.insert("simple_async".to_string(), Box::new(SimpleAsyncHandler));
    h
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let workflow_json = r#"
    {
        "id": "async_workflow",
        "name": "Async Processing Workflow",
        "priority": 0,
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
                "function": { "name": "async_http", "input": {} }
            },
            {
                "id": "simple_process",
                "name": "Simple Processing",
                "function": { "name": "simple_async", "input": {} }
            }
        ]
    }
    "#;

    let workflow = Workflow::from_json(workflow_json)?;

    println!("Method 1: Using async handlers directly");
    {
        let engine = Engine::new(vec![workflow.clone()], Some(make_handlers()))?;
        let mut message = Message::from_value(&json!({
            "required_field": "present",
            "value": "test data"
        }));
        engine.process_message(&mut message).await?;
        println!("Message processed: {:?}", message.context["data"]);
    }

    println!("\nMethod 2: CPU-bound async handlers");
    {
        let engine = Engine::new(vec![workflow.clone()], Some(make_handlers()))?;
        let mut message = Message::from_value(&json!({
            "required_field": "present",
            "value": "test data"
        }));
        engine.process_message(&mut message).await?;
        println!(
            "Message processed with async handlers: {:?}",
            message.context["data"]
        );
    }

    println!("\nMethod 3: Integration with Axum");
    {
        use axum::{Json, Router, routing::post};

        let engine = Arc::new(Engine::new(vec![workflow.clone()], Some(make_handlers()))?);

        async fn process_handler(
            engine: Arc<Engine>,
            Json(mut message): Json<Message>,
        ) -> Json<Message> {
            match engine.process_message(&mut message).await {
                Ok(_) => Json(message),
                Err(e) => {
                    eprintln!("Processing error: {:?}", e);
                    message.errors.push(dataflow_rs::ErrorInfo::simple(
                        "PROCESSING_ERROR".to_string(),
                        format!("Failed to process: {}", e),
                        None,
                    ));
                    Json(message)
                }
            }
        }

        let _app: Router = Router::new().route(
            "/process",
            post({
                let engine = Arc::clone(&engine);
                move |body| process_handler(engine, body)
            }),
        );

        println!("Starting Axum server on http://localhost:3000");
        println!("Example request:");
        println!("  curl -X POST http://localhost:3000/process \\");
        println!("    -H 'Content-Type: application/json' \\");
        println!("    -d '{{\"required_field\":\"test\",\"value\":\"data\"}}'");
    }

    println!("\nPerformance Comparison:");
    {
        use std::time::Instant;

        let engine = Arc::new(Engine::new(vec![workflow], Some(make_handlers()))?);

        let mut message = Message::from_value(&json!({
            "required_field": "present",
            "value": "test data"
        }));

        let start = Instant::now();
        engine.process_message(&mut message).await?;
        println!("Single message: {:?}", start.elapsed());

        let messages = vec![
            Message::from_value(&json!({"required_field": "1", "value": "data1"})),
            Message::from_value(&json!({"required_field": "2", "value": "data2"})),
            Message::from_value(&json!({"required_field": "3", "value": "data3"})),
            Message::from_value(&json!({"required_field": "4", "value": "data4"})),
        ];

        let start = Instant::now();
        let mut handles = vec![];

        for mut msg in messages {
            let engine = Arc::clone(&engine);
            handles.push(tokio::spawn(async move {
                engine.process_message(&mut msg).await.unwrap();
                msg
            }));
        }

        let results = futures::future::join_all(handles).await;
        println!("Concurrent processing (4 messages): {:?}", start.elapsed());
        println!("All messages processed: {} succeeded", results.len());
    }

    println!("\nMigration complete! Key benefits:");
    println!("  - Typed Input deserialized once at engine init (not per message)");
    println!("  - TaskContext records audit-trail changes automatically");
    println!("  - TaskOutcome enum replaces magic-number status tuples");
    println!("  - Zero-copy sharing with Arc<Logic>");

    Ok(())
}
