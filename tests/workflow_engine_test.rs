use async_trait::async_trait;
use dataflow_rs::engine::functions::{
    AsyncFunctionHandler, FunctionConfig, FunctionHandler, SyncFunctionWrapper,
};
use dataflow_rs::engine::message::{Change, Message};
use dataflow_rs::{Engine, Result, Task, Workflow};
use datalogic_rs::DataLogic;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

// A simple sync task implementation for backward compatibility testing
#[derive(Debug)]
struct LoggingTask;

impl FunctionHandler for LoggingTask {
    fn execute(
        &self,
        message: &mut Message,
        _config: &FunctionConfig,
        _datalogic: &DataLogic,
    ) -> Result<(usize, Vec<Change>)> {
        println!("Executed task for message: {}", &message.id);
        Ok((200, vec![]))
    }
}

// An async task implementation
struct AsyncLoggingTask;

#[async_trait]
impl AsyncFunctionHandler for AsyncLoggingTask {
    async fn execute(
        &self,
        message: &mut Message,
        _config: &FunctionConfig,
        _datalogic: Arc<DataLogic>,
    ) -> Result<(usize, Vec<Change>)> {
        println!("Executed async task for message: {}", &message.id);
        // Simulate async work
        tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
        Ok((200, vec![]))
    }
}

#[test]
fn test_sync_task_execution() {
    // This test only tests the task implementation
    let task = LoggingTask;

    // Create a dummy message
    let mut message = Message::from_value(&json!({}));

    // Execute the task directly
    let config = FunctionConfig::Custom {
        name: "log".to_string(),
        input: json!({}),
    };
    let datalogic = DataLogic::with_preserve_structure();
    let result = task.execute(&mut message, &config, &datalogic);

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
            condition: json!(true),
            condition_index: None,
            continue_on_error: false,
            function: FunctionConfig::Custom {
                name: "log".to_string(),
                input: json!({}),
            },
        }],
        condition: json!(true),
        condition_index: None,
        continue_on_error: false,
    };

    // Create custom functions using AsyncFunctionHandler
    let mut custom_functions: HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>> =
        HashMap::new();

    // Wrap sync handler for async compatibility
    custom_functions.insert(
        "log".to_string(),
        Box::new(SyncFunctionWrapper::new(
            Box::new(LoggingTask) as Box<dyn FunctionHandler + Send + Sync>
        )),
    );

    // Create engine with the workflow and custom function
    let engine = Engine::new(vec![workflow], Some(custom_functions));

    // Create a dummy message
    let mut message = Message::from_value(&json!({}));

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
        message.audit_trail[0].task_id.as_ref(),
        "log_task",
        "Audit trail should contain the executed task"
    );
}

#[tokio::test]
async fn test_async_workflow_execution() {
    // Create a workflow with async task
    let workflow = Workflow {
        id: "async_workflow".to_string(),
        name: "Async Test Workflow".to_string(),
        priority: 0,
        description: Some("An async test workflow".to_string()),
        tasks: vec![Task {
            id: "async_log_task".to_string(),
            name: "Async Log Task".to_string(),
            description: Some("An async test task".to_string()),
            condition: json!(true),
            condition_index: None,
            continue_on_error: false,
            function: FunctionConfig::Custom {
                name: "async_log".to_string(),
                input: json!({}),
            },
        }],
        condition: json!(true),
        condition_index: None,
        continue_on_error: false,
    };

    // Create custom async functions
    let mut custom_functions: HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>> =
        HashMap::new();
    custom_functions.insert("async_log".to_string(), Box::new(AsyncLoggingTask));

    // Create engine with the workflow and custom function
    let engine = Engine::new(vec![workflow], Some(custom_functions));

    // Create a dummy message
    let mut message = Message::from_value(&json!({}));

    // Process the message
    let result = engine.process_message(&mut message).await;

    assert!(result.is_ok(), "Async workflow execution should succeed");

    // Verify the message was processed correctly
    assert_eq!(
        message.audit_trail.len(),
        1,
        "Message should have one audit trail entry"
    );
    assert_eq!(
        message.audit_trail[0].task_id.as_ref(),
        "async_log_task",
        "Audit trail should contain the executed async task"
    );
}
