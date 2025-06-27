use dataflow_rs::{engine::message::Message, Engine, Workflow};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create the workflow engine (built-in functions are auto-registered)
    let mut engine = Engine::new();

    // Define a workflow that:
    // 1. Fetches data from a public API
    // 2. Enriches the message with transformed data
    // 3. Validates the enriched data
    let workflow_json = r#"
    {
        "id": "complete_workflow",
        "name": "Complete Workflow Example",
        "priority": 0,
        "description": "Demonstrates fetch -> enrich -> validate flow",
        "condition": { "==": [true, true] },
        "tasks": [
            {
                "id": "fetch_user_data",
                "name": "Fetch User Data",
                "description": "Get user data from a public API",
                "function": {
                    "name": "http",
                    "input": {
                        "url": "https://jsonplaceholder.typicode.com/users/1",
                        "method": "GET",
                        "headers": {
                            "Accept": "application/json"
                        }
                    }
                }
            },
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

    // Parse and add the workflow to the engine
    let workflow = Workflow::from_json(workflow_json)?;
    engine.add_workflow(&workflow);

    // Create a message to process with properly initialized data structure
    let mut message = Message::new(&json!({}));

    // Process the message through the workflow asynchronously
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
