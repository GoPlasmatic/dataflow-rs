use async_trait::async_trait;
use dataflow_rs::engine::functions::AsyncFunctionHandler;
use dataflow_rs::engine::message::{Change, Message};
use dataflow_rs::engine::task::Function;
use dataflow_rs::{Engine, Result, Task, Workflow};
use serde_json::{json, Value};

// A simple task implementation
#[derive(Debug)]
struct LoggingTask;

#[async_trait]
impl AsyncFunctionHandler for LoggingTask {
    async fn execute(&self, message: &mut Message, _input: &Value) -> Result<(usize, Vec<Change>)> {
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
    let input = json!({});
    let result = task.execute(&mut message, &input).await;

    // Verify the execution was successful
    assert!(result.is_ok(), "Task execution should succeed");
}

#[tokio::test]
async fn test_workflow_execution() {
    let mut engine = Engine::new();

    engine.register_task_function("log".to_string(), Box::new(LoggingTask));

    // Create a dummy message
    let mut message = Message::new(&json!({}));

    // Add a workflow to the engine
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
            function: Function {
                name: "log".to_string(),
                input: json!({}),
            },
        }],
        condition: Some(json!(true)),
    };
    engine.add_workflow(&workflow);

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
