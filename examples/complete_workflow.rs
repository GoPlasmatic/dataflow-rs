use dataflow_rs::{engine::message::Message, Engine, Workflow};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
                        "rule": {
                            "and": [
                                { "!": { "var": "data.user.id" } },
                                { "!": { "var": "data.user.name" } },
                                { "!": { "var": "data.user.email" } },
                                {
                                    "in": [
                                        "@",
                                        { "var": "data.user.email" }
                                    ]
                                }
                            ]
                        }
                    }
                }
            }
        ]
    }
    "#;

    // Parse and add the workflow to the engine
    let workflow = Workflow::from_json(workflow_json)?;
    engine.add_workflow(&workflow);

    // Create a message to process
    let mut message = Message::new(&json!({}));

    // Process the message through the workflow
    println!("Processing message through workflow...");
    engine.process_message(&mut message);

    println!(
        "\nFull message structure:\n{}",
        serde_json::to_string_pretty(&message)?
    );

    Ok(())
}
