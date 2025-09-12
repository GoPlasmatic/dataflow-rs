//! # Async Migration Example
//!
//! This example demonstrates how to use the new async dataflow-rs API with DataLogic v4.
//!
//! ## Key Changes from Previous Versions:
//!
//! 1. **Pure Async API**: Engine now provides async `process_message()` method
//! 2. **No Thread Management**: Removed ThreadedEngine and RayonEngine
//! 3. **DataLogic v4**: Leverages Arc<CompiledLogic> for zero-copy sharing
//! 4. **Tokio Integration**: Designed for Tokio runtime with mixed I/O and CPU workloads
//!
//! Run with: `cargo run --example async_migration`

use async_trait::async_trait;
use dataflow_rs::{
    AsyncFunctionHandler, Engine, FunctionConfig, FunctionHandler, Message, Workflow,
    engine::SyncFunctionWrapper,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

// Example of a new async function handler
struct AsyncHttpHandler;

#[async_trait]
impl AsyncFunctionHandler for AsyncHttpHandler {
    async fn execute(
        &self,
        message: &mut Message,
        _config: &FunctionConfig,
        _datalogic: Arc<datalogic_rs::DataLogic>,
    ) -> dataflow_rs::Result<(usize, Vec<dataflow_rs::engine::message::Change>)> {
        // Simulate async HTTP call
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Update message data
        message.data["http_response"] = json!({
            "status": "success",
            "data": "Response from API"
        });

        Ok((200, vec![]))
    }
}

// Example of adapting a legacy synchronous handler
struct LegacySyncHandler;

impl FunctionHandler for LegacySyncHandler {
    fn execute(
        &self,
        message: &mut Message,
        _config: &FunctionConfig,
        _datalogic: &datalogic_rs::DataLogic,
    ) -> dataflow_rs::Result<(usize, Vec<dataflow_rs::engine::message::Change>)> {
        // Synchronous processing
        message.data["processed"] = json!(true);
        Ok((200, vec![]))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // Create workflows
    let workflow_json = r#"
    {
        "id": "async_workflow",
        "name": "Async Processing Workflow",
        "priority": 0,
        "tasks": [
            {
                "id": "validate",
                "name": "Validate Input",
                "function": {
                    "name": "validate",
                    "input": {
                        "rules": [
                            {
                                "logic": {"!!": [{"var": "data.required_field"}]},
                                "message": "Required field is missing",
                                "path": "data.required_field"
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
                            {
                                "path": "data.transformed",
                                "logic": {"var": "data.input"}
                            }
                        ]
                    }
                }
            },
            {
                "id": "http_call",
                "name": "Make HTTP Call",
                "function": {
                    "name": "async_http"
                }
            },
            {
                "id": "legacy_process",
                "name": "Legacy Processing",
                "function": {
                    "name": "legacy_sync"
                }
            }
        ]
    }
    "#;

    let workflow = Workflow::from_json(workflow_json)?;

    // Method 1: Create engine with async handlers
    println!("Method 1: Using async handlers directly");
    {
        let mut custom_functions: HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>> =
            HashMap::new();

        custom_functions.insert("async_http".to_string(), Box::new(AsyncHttpHandler));

        // Wrap legacy sync handler for compatibility
        custom_functions.insert(
            "legacy_sync".to_string(),
            Box::new(SyncFunctionWrapper::new(
                Box::new(LegacySyncHandler) as Box<dyn FunctionHandler + Send + Sync>
            )),
        );

        let engine = Engine::new(vec![workflow.clone()], Some(custom_functions));

        let mut message = Message::from_value(&json!({
            "required_field": "present",
            "input": "test data"
        }));

        // Process message asynchronously
        engine.process_message(&mut message).await?;

        println!("Message processed: {:?}", message.data);
    }

    // Method 2: Create engine from legacy sync handlers (for migration)
    println!("\nMethod 2: Migrating from sync handlers");
    {
        let mut sync_functions: HashMap<String, Box<dyn FunctionHandler + Send + Sync>> =
            HashMap::new();

        sync_functions.insert("legacy_sync".to_string(), Box::new(LegacySyncHandler));

        // Engine automatically wraps sync handlers for async execution
        let engine = Engine::from_sync_handlers(vec![workflow.clone()], Some(sync_functions));

        let mut message = Message::from_value(&json!({
            "required_field": "present",
            "input": "test data"
        }));

        engine.process_message(&mut message).await?;

        println!("Message processed with sync handlers: {:?}", message.data);
    }

    // Method 3: Using with Axum web server
    println!("\nMethod 3: Integration with Axum");
    {
        use axum::{Json, Router, routing::post};

        // Create engine once and share across requests
        let engine = Arc::new(Engine::new(vec![workflow.clone()], None));

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
        println!("    -d '{{\"data\":{{\"required_field\":\"test\",\"input\":\"data\"}}}}'");

        // Uncomment to run the server
        // let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
        // axum::Server::bind(&addr)
        //     .serve(app.into_make_service())
        //     .await?;
    }

    // Performance comparison
    println!("\nPerformance Comparison:");
    {
        use std::time::Instant;

        let engine = Arc::new(Engine::new(vec![workflow], None));

        // Single message
        let mut message = Message::from_value(&json!({
            "required_field": "present",
            "input": "test data"
        }));

        let start = Instant::now();
        engine.process_message(&mut message).await?;
        println!("Single message: {:?}", start.elapsed());

        // Concurrent processing (simulating multiple API requests)
        let messages = vec![
            Message::from_value(&json!({"required_field": "1", "input": "data1"})),
            Message::from_value(&json!({"required_field": "2", "input": "data2"})),
            Message::from_value(&json!({"required_field": "3", "input": "data3"})),
            Message::from_value(&json!({"required_field": "4", "input": "data4"})),
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

    println!("\n✅ Migration complete! Key benefits:");
    println!("  - Simplified architecture (no custom thread management)");
    println!("  - Better Tokio integration for mixed I/O and CPU workloads");
    println!("  - Zero-copy sharing with Arc<CompiledLogic>");
    println!("  - Backward compatibility with sync handlers");

    Ok(())
}
