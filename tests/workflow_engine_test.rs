use async_trait::async_trait;
use dataflow_rs::engine::functions::{AsyncFunctionHandler, FunctionConfig};
use dataflow_rs::engine::message::{Change, Message};
use dataflow_rs::{Engine, Result, Task, Workflow};
use std::collections::HashMap;
use serde_json::json;

// A simple task implementation
#[derive(Debug)]
struct LoggingTask;

#[async_trait]
impl AsyncFunctionHandler for LoggingTask {
    async fn execute(
        &self,
        message: &mut Message,
        _config: &FunctionConfig,
    ) -> Result<(usize, Vec<Change>)> {
        println!("Executed task for message: {}", &message.id);
        Ok((200, vec![]))
    }
}

#[tokio::test]
async fn test_task_execution() {
    // This test only tests the task implementation
    let task = LoggingTask;

    // Create a dummy message
    let mut message = Message::new(&json!({}));

    // Execute the task directly
    let config = FunctionConfig::Raw(json!({}));
    let result = task.execute(&mut message, &config).await;

    // Verify the execution was successful
    assert!(result.is_ok(), "Task execution should succeed");
}

#[tokio::test]
async fn test_workflow_execution() {
    // Create a workflow
    let workflow = Workflow {
        id: "test_workflow".to_string(),
        name: "Test Workflow".to_string(),
        priority: 0,
        description: Some("A test workflow".to_string()),
        tasks: vec![Task {
            id: "log_task".to_string(),
            name: "Log Task".to_string(),
            description: Some("A test task".to_string()),
            condition: Some(json!(true)),
            function_name: "log".to_string(),
            function_config: FunctionConfig::Raw(json!({})),
        }],
        condition: Some(json!(true)),
    };
    
    // Create custom functions
    let mut custom_functions = HashMap::new();
    custom_functions.insert(
        "log".to_string(), 
        Box::new(LoggingTask) as Box<dyn AsyncFunctionHandler + Send + Sync>
    );
    
    // Create engine with the workflow and custom function
    let engine = Engine::new(vec![workflow], Some(custom_functions), None, None, None);

    // Create a dummy message
    let mut message = Message::new(&json!({}));

    // Process the message
    let result = engine.process_message(&mut message).await;

    match &result {
        Ok(_) => println!("Workflow executed successfully"),
        Err(e) => println!("Workflow execution failed: {e:?}"),
    }

    assert!(result.is_ok(), "Workflow execution should succeed");

    // Verify the message was processed correctly
    assert_eq!(
        message.audit_trail.len(),
        1,
        "Message should have one audit trail entry"
    );
    assert_eq!(
        message.audit_trail[0].task_id, "log_task",
        "Audit trail should contain the executed task"
    );
}
