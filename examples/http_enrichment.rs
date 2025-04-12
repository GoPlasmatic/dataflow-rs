use dataflow_rs::{
    Engine, Workflow, FunctionHandler, 
    engine::message::{Message, Change}
};
use datalogic_rs::{arena::DataArena, DataLogic, DataValue, FromJson};
use reqwest::Client;
use tokio;
use serde_json::{json, Value};

// Define a simplified HTTP task for fetching cat facts
struct CatFactTask {
    client: Client,
}

impl CatFactTask {
    fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl FunctionHandler for CatFactTask {
    fn execute<'a>(&self, message: &mut Message<'a>, _input: &DataValue, arena: &'a DataArena) -> Result<Vec<Change<'a>>, String> {
        // Create a runtime for async HTTP requests
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| format!("Failed to create runtime: {}", e))?;
        
        // For this simple example, we use a fixed URL
        let url = "https://catfact.ninja/fact";
        
        println!("Fetching cat fact from: {}", url);
        
        // Execute the HTTP request
        let response_data = runtime.block_on(async {
            // Make a simple GET request
            let response = self.client.get(url)
                .send()
                .await
                .map_err(|e| format!("HTTP request failed: {}", e))?;
                
            // Parse the response as JSON
            let json = response.json::<Value>()
                .await
                .map_err(|e| format!("Failed to parse response as JSON: {}", e))?;
                
            Ok::<Value, String>(json)
        }).map_err(|e| e.to_string())?;
        
        // Store the cat fact in message.data
        let mut data_object = json!({});
        data_object["cat_fact"] = response_data.clone();
        message.data = DataValue::from_json(&data_object, arena);
        
        // Record the change using the values in arena's lifetime
        let changes = vec![
            Change {
                path: "data.cat_fact".to_string(),
                old_value: DataValue::null(),
                new_value: DataValue::from_json(&response_data, arena),
            }
        ];
        
        // Return the changes vector
        Ok(changes)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize the DataLogic engine
    let data_logic = Box::leak(Box::new(DataLogic::default()));
    
    // Create the workflow engine
    let mut engine = Engine::new(data_logic);
    
    // Register the cat fact task
    engine.register_function("cat_fact".to_string(), Box::new(CatFactTask::new()));
    
    // Define a simple workflow for fetching cat facts
    let workflow_json = r#"
    {
        "id": "cat_fact_workflow",
        "name": "Cat Fact Fetcher",
        "description": "Fetches random cat facts",
        "condition": { "==": [true, true] },
        "tasks": [
            {
                "id": "get_cat_fact",
                "name": "Get Cat Fact",
                "function": "cat_fact",
                "condition": { "==": [true, true] },
                "input": {}
            }
        ]
    }
    "#;
    
    // Parse and prepare the workflow
    let mut workflow = Workflow::from_json(workflow_json)?;
    workflow.prepare(data_logic);
    
    // Add the workflow to the engine
    engine.add_workflow(&workflow);
    
    // Create a message to process
    let mut message = Message {
        id: "msg_001".to_string(),
        data: DataValue::from_json(&json!({}), data_logic.arena()),
        payload: DataValue::from_json(&json!({}), data_logic.arena()),
        metadata: DataValue::from_json(&json!({}), data_logic.arena()),
        temp_data: DataValue::from_json(&json!({}), data_logic.arena()), 
        audit_trail: Vec::new(),
    };
    
    // Process the message through the workflow
    engine.process_message(&mut message);
    
    println!("Message processed: {:?}", message);
    
    Ok(())
} 