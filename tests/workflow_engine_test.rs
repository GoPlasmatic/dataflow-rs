use dataflow_rs::engine::functions::task::TaskFunctionHandler;
use dataflow_rs::engine::functions::Function;
use dataflow_rs::engine::message::{Change, Message};
use dataflow_rs::{Engine, Task, Workflow};
use datalogic_rs::arena::DataArena;
use datalogic_rs::{DataLogic, DataValue};

// A simple task implementation
struct LoggingTask;

impl TaskFunctionHandler for LoggingTask {
    fn execute<'a>(
        &self,
        message: &mut Message<'a>,
        _input: &DataValue,
        _arena: &'a DataArena,
    ) -> Result<Vec<Change<'a>>, String> {
        println!("Executed task for message: {}", &message.id);
        Ok(vec![])
    }
}

#[test]
fn test_task_execution() {
    // This test only tests the task implementation
    let task = LoggingTask;

    // Create a dummy message
    let mut message = Message {
        id: "test123".to_string(),
        data: DataValue::Null,
        payload: DataValue::Null,
        metadata: DataValue::Null,
        temp_data: DataValue::Null,
        audit_trail: Vec::new(),
    };

    // Execute the task directly
    let input = DataValue::Null;
    let arena = DataArena::new();
    let result = task.execute(&mut message, &input, &arena);

    // Verify the execution was successful
    assert!(result.is_ok(), "Task execution should succeed");
}

#[test]
fn test_workflow_execution() {
    let data_logic: &'static DataLogic = Box::leak(Box::new(DataLogic::new()));
    let mut engine = Engine::new(data_logic);

    engine.register_task_function("log".to_string(), Box::new(LoggingTask));

    // Create a dummy message
    let mut message = Message {
        id: "test123".to_string(),
        data: DataValue::Null,
        payload: DataValue::Null,
        metadata: DataValue::Null,
        temp_data: DataValue::Null,
        audit_trail: Vec::new(),
    };

    // Add a workflow to the engine
    let mut workflow = Workflow {
        id: "test_workflow".to_string(),
        name: "Test Workflow".to_string(),
        description: Some("A test workflow".to_string()),
        task_logics: vec![],
        tasks: vec![Task {
            id: "log_task".to_string(),
            name: "Log Task".to_string(),
            description: Some("A test task".to_string()),
            condition: Some(serde_json::Value::Bool(true)),
            function: Function::new("log".to_string(), serde_json::Value::Null),
            input: serde_json::Value::Null,
        }],
        condition: Some(serde_json::Value::Bool(true)),
    };
    workflow.prepare(data_logic);
    engine.add_workflow(&workflow);

    // Process the message
    engine.process_message(&mut message);

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
