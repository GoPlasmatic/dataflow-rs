use dataflow_rs::{Engine, Workflow, engine::message::Message};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
        "condition": { "==": [true, true] },
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
                                "path": "data",
                                "logic": { "preserve": {"user": {}} }
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
                                "path": "data.user.id", 
                                "logic": { "var": "temp_data.body.id" }
                            },
                            {
                                "path": "data.user.name", 
                                "logic": { "var": "temp_data.body.name" }
                            },
                            {
                                "path": "data.user.email", 
                                "logic": { "var": "temp_data.body.email" }
                            },
                            {
                                "path": "data.user.address", 
                                "logic": {
                                    "cat": [
                                        { "var": "temp_data.body.address.street" },
                                        ", ",
                                        { "var": "temp_data.body.address.city" }
                                    ]
                                }
                            },
                            {
                                "path": "data.user.company", 
                                "logic": { "var": "temp_data.body.company.name" }
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
                    "name": "validate",
                    "input": {
                        "rules": [
                            {
                                "path": "data",
                                "logic": { "!!": { "var": "data.user.id" } },
                                "message": "User ID is required"
                            },
                            {
                                "path": "data",
                                "logic": { "!!": { "var": "data.user.name" } },
                                "message": "User name is required"
                            },
                            {
                                "path": "data",
                                "logic": { "!!": { "var": "data.user.email" } },
                                "message": "User email is required"
                            },
                            {
                                "path": "data",
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

    // Create the workflow engine with the workflow (built-in functions are auto-registered by default)
    let engine = Engine::new(vec![workflow], None, None, None, None);

    // Create a message to process with sample user data
    let mut message = Message::new(&json!({}));

    // Add sample user data to temp_data (simulating what would come from an API)
    message.temp_data = json!({
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
    });

    // Process the message through the workflow
    println!("Processing message through workflow...");

    match engine.process_message(&mut message) {
        Ok(_) => {
            println!("Workflow completed successfully!");
        }
        Err(e) => {
            eprintln!("Error executing workflow: {e:?}");
            if !message.errors.is_empty() {
                println!("\nErrors recorded in message:");
                for err in &message.errors {
                    println!(
                        "- Workflow: {:?}, Task: {:?}, Error: {:?}",
                        err.workflow_id, err.task_id, err.error_message
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
