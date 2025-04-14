use dataflow_rs::engine::functions::source::cron::CronSourceFunctionHandler;
use dataflow_rs::engine::functions::source::http::HttpSourceFunctionHandler;
use dataflow_rs::engine::functions::source::SourceFunctionHandler;
use dataflow_rs::engine::functions::task::TaskFunctionHandler;
use dataflow_rs::engine::message::{Change, Message};
use dataflow_rs::{Engine, Workflow};
use datalogic_rs::arena::DataArena;
use datalogic_rs::{DataLogic, DataValue, FromJson};
use hyper::Method;
use once_cell::sync::OnceCell;
use serde_json::json;
use std::sync::Arc;
use std::thread_local;
use tokio;

// A simple task that adds a greeting to the message
#[derive(Clone)]
struct GreetingTask;

// Explicitly implement Send and Sync for our task handler
unsafe impl Send for GreetingTask {}
unsafe impl Sync for GreetingTask {}

impl TaskFunctionHandler for GreetingTask {
    fn execute<'a>(
        &self,
        message: &mut Message<'a>,
        _input: &DataValue,
        arena: &'a datalogic_rs::arena::DataArena,
    ) -> Result<Vec<Change<'a>>, String> {
        // Extract name from payload if available
        let name = message
            .payload
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Guest");

        // Create greeting
        let greeting = format!("Hello, {}!", name);

        // Set message data
        let data_obj = json!({"greeting": greeting});
        message.data = DataValue::from_json(&data_obj, arena);

        // Create change record
        let changes = vec![Change {
            path: "data.greeting".to_string(),
            old_value: DataValue::null(),
            new_value: DataValue::from_json(&json!(greeting), arena),
        }];

        Ok(changes)
    }
}

// Thread-local storage for DataLogic and configuration
thread_local! {
    static WORKFLOW_DEF: OnceCell<String> = OnceCell::new();
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define the workflow
    let workflow_json = r#"
    {
        "id": "greeting_workflow",
        "name": "Greeting Generator",
        "description": "Generates a greeting based on the payload name",
        "condition": { "==": [true, true] },
        "tasks": [
            {
                "id": "generate_greeting",
                "name": "Generate Greeting",
                "function": {
                    "name": "greet",
                    "input": {}
                },
                "condition": { "==": [true, true] },
                "input": {}
            }
        ]
    }
    "#
    .to_string();

    // Initialize thread-local storage
    WORKFLOW_DEF.with(|cell| {
        let _ = cell.set(workflow_json.clone());
    });

    // Create a message processor that processes each message in its own thread-local context
    let message_processor = Arc::new(move |message: Message| {
        println!("Received message: {}", message.id);

        // Use thread_local for safety - each thread gets its own DataLogic instance
        WORKFLOW_DEF.with(|cell| {
            if let Some(wf_json) = cell.get() {
                // Create a new DataLogic instance for this message processing
                let thread_local_dl = Box::leak(Box::new(DataLogic::default()));
                let mut engine = Engine::new(thread_local_dl);

                // Register task handler
                engine.register_task_function("greet".to_string(), Box::new(GreetingTask));

                // Load workflow from JSON
                if let Ok(mut workflow) = Workflow::from_json(wf_json) {
                    workflow.prepare(thread_local_dl);
                    engine.add_workflow(&workflow);

                    // Process a cloned message (with static lifetime)
                    let mut msg_copy = Message {
                        id: message.id.clone(),
                        data: DataValue::from_json(&json!({}), thread_local_dl.arena()),
                        payload: DataValue::from_json(&json!({}), thread_local_dl.arena()),
                        metadata: DataValue::from_json(&json!({}), thread_local_dl.arena()),
                        temp_data: DataValue::from_json(&json!({}), thread_local_dl.arena()),
                        audit_trail: Vec::new(),
                    };

                    // Copy over payload fields individually if needed
                    if let Some(name) = message.payload.get("name") {
                        if let Some(name_str) = name.as_str() {
                            // Create a new payload with just the name
                            let payload_obj = json!({"name": name_str});
                            msg_copy.payload =
                                DataValue::from_json(&payload_obj, thread_local_dl.arena());
                        }
                    }

                    // Process the message
                    engine.process_message(&mut msg_copy);

                    // Output the result
                    if let Some(greeting) = msg_copy.data.get("greeting") {
                        println!(
                            "Processed message: {} with greeting: {}",
                            msg_copy.id, greeting
                        );
                    } else {
                        println!("Processed message: {} (no greeting found)", msg_copy.id);
                    }
                }
            }
        });
    });

    // Create a function to initialize DataArena for each thread
    let init_arena = Arc::new(|| {
        // Create a new data arena for each thread
        DataArena::new()
    });

    // Create HTTP source function
    let http_source = HttpSourceFunctionHandler::new(
        "127.0.0.1:8080".to_string(),
        "/webhook".to_string(),
        init_arena.clone(),
    )
    .with_methods(vec![Method::GET, Method::POST]);

    // Create Cron source function that generates a message every 10 seconds
    let cron_source = CronSourceFunctionHandler::new(
        10, // every 10 seconds for demonstration purposes
        init_arena.clone(),
        Box::new(|| {
            json!({
                "name": "Timer",
                "timestamp": chrono::Utc::now().to_rfc3339()
            })
        }),
    );

    // Start both source functions concurrently
    println!("Starting source functions...");
    println!("HTTP listening on http://127.0.0.1:8080/webhook");
    println!("Cron will generate a message every 10 seconds");
    println!("Press Ctrl+C to exit");

    tokio::select! {
        result = http_source.start(message_processor.clone()) => {
            if let Err(e) = result {
                eprintln!("HTTP source error: {}", e);
            }
        }
        result = cron_source.start(message_processor.clone()) => {
            if let Err(e) = result {
                eprintln!("Cron source error: {}", e);
            }
        }
    }

    Ok(())
}
