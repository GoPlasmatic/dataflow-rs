//! # Complete Workflow Example
//!
//! This example demonstrates a complete workflow with data transformation and validation
//! using the async dataflow-rs engine.
//!
//! The workflow follows the recommended pattern:
//! 1. parse_json - Load payload into data context
//! 2. map - Transform data to desired structure
//! 3. validation - Validate the transformed data
//!
//! Run with: `cargo run --example complete_workflow`

use dataflow_rs::{engine::message::Message, Engine, Workflow};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // Define a workflow that:
    // 1. Parses the payload into the data context (recommended first step)
    // 2. Transforms the data to our model
    // 3. Validates the transformed data
    let workflow_json = r#"
    {
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
            println!("Workflow completed successfully!");
        }
        Err(e) => {
            eprintln!("Error executing workflow: {e:?}");
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

    // Show final data structure
    println!("\nTransformed user data:");
    println!(
        "{}",
        serde_json::to_string_pretty(&message.context["data"]["user"])?
    );

    println!("\nAudit trail:");
    for audit in &message.audit_trail {
        println!(
            "  - Task: {} (Status: {}, Changes: {})",
            audit.task_id,
            audit.status,
            audit.changes.len()
        );
    }

    if !message.errors.is_empty() {
        println!("\nValidation errors:");
        for err in &message.errors {
            println!("  - {}", err.message);
        }
    }

    Ok(())
}
