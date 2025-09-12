//! # Complete Workflow Example
//!
//! This example demonstrates a complete workflow with data transformation and validation
//! using the async dataflow-rs engine.
//!
//! Run with: `cargo run --example complete_workflow`

use dataflow_rs::{Engine, Workflow, engine::message::Message};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // Define a workflow that:
    // 1. Prepares sample user data
    // 2. Enriches the message with transformed data
    // 3. Validates the enriched data
    let workflow_json = r#"
    {
        "id": "complete_workflow",
        "name": "Complete Workflow Example",
        "priority": 0,
        "description": "Demonstrates enrich -> validate flow",
        "tasks": [
            {
                "id": "initialize_user",
                "name": "Initialize User Structure",
                "description": "Create empty user object in data",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "user",
                                "logic": {}
                            }
                        ]
                    }
                }
            },
            {
                "id": "transform_data",
                "name": "Transform Data",
                "description": "Map API response to our data model",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "user.id", 
                                "logic": { "var": "payload.body.id" }
                            },
                            {
                                "path": "user.name", 
                                "logic": { "var": "payload.body.name" }
                            },
                            {
                                "path": "user.email", 
                                "logic": { "var": "payload.body.email" }
                            },
                            {
                                "path": "user.address", 
                                "logic": {
                                    "cat": [
                                        { "var": "payload.body.address.street" },
                                        ", ",
                                        { "var": "payload.body.address.city" }
                                    ]
                                }
                            },
                            {
                                "path": "user.company", 
                                "logic": { "var": "payload.body.company.name" }
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
                                "path": "user.id",
                                "logic": { "!!": { "var": "data.user.id" } },
                                "message": "User ID is required"
                            },
                            {
                                "path": "user.name",
                                "logic": { "!!": { "var": "data.user.name" } },
                                "message": "User name is required"
                            },
                            {
                                "path": "user.email",
                                "logic": { "!!": { "var": "data.user.email" } },
                                "message": "User email is required"
                            },
                            {
                                "path": "user.email",
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
    }
    "#;

    // Parse the workflow
    let workflow = Workflow::from_json(workflow_json)?;

    // Create the workflow engine with the workflow (built-in functions are auto-registered)
    let engine = Engine::new(vec![workflow], None);

    // Create a message to process with sample user data
    let mut message = Message::from_value(&json!({
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

    // Process the message through the workflow
    println!("Processing message through workflow...");

    match engine.process_message(&mut message).await {
        Ok(_) => {
            println!("✅ Workflow completed successfully!");
        }
        Err(e) => {
            eprintln!("❌ Error executing workflow: {e:?}");
            if !message.errors.is_empty() {
                println!("\nErrors recorded in message:");
                for err in &message.errors {
                    println!(
                        "- Workflow: {:?}, Task: {:?}, Error: {}",
                        err.workflow_id, err.task_id, err.message
                    );
                }
            }
        }
    }

    println!(
        "\nFull message structure:\n{}",
        serde_json::to_string_pretty(&message)?
    );

    Ok(())
}
