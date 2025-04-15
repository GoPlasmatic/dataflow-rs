use dataflow_rs::engine::functions::FunctionHandler;
use dataflow_rs::engine::message::{Change, Message};
use dataflow_rs::engine::task::Function;
use dataflow_rs::{Engine, Result, Task, Workflow};
use serde_json::Value;

// A simple task implementation
struct LoggingTask;

impl FunctionHandler for LoggingTask {
    fn execute(&self, message: &mut Message, _input: &Value) -> Result<(usize, Vec<Change>)> {
        println!("Executed task for message: {}", &message.id);
        Ok((200, vec![]))
    }
}

#[test]
fn test_task_execution() {
    // This test only tests the task implementation
    let task = LoggingTask;

    // Create a dummy message
    let mut message = Message::new(&serde_json::Value::Null);

    // Execute the task directly
    let input = serde_json::Value::Null;
    let result = task.execute(&mut message, &input);

    // Verify the execution was successful
    assert!(result.is_ok(), "Task execution should succeed");
}

#[test]
fn test_workflow_execution() {
    let mut engine = Engine::new();

    engine.register_task_function("log".to_string(), Box::new(LoggingTask));

    // Create a dummy message
    let mut message = Message::new(&serde_json::Value::Null);

    // Add a workflow to the engine
    let workflow = Workflow {
        id: "test_workflow".to_string(),
        name: "Test Workflow".to_string(),
        description: Some("A test workflow".to_string()),
        tasks: vec![Task {
            id: "log_task".to_string(),
            name: "Log Task".to_string(),
            description: Some("A test task".to_string()),
            condition: Some(serde_json::Value::Bool(true)),
            function: Function {
                name: "log".to_string(),
                input: serde_json::Value::Null,
            },
        }],
        condition: Some(serde_json::Value::Bool(true)),
    };
    engine.add_workflow(&workflow);

    // Process the message
    let result = engine.process_message(&mut message);

    assert!(result.is_ok(), "Workflow execution should succeed");
    println!("Message: {:?}", message);
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
