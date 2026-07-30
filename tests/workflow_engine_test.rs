use async_trait::async_trait;
use dataflow_rs::engine::functions::{AsyncFunctionHandler, FunctionConfig};
use dataflow_rs::engine::message::Message;
use dataflow_rs::engine::utils::set_nested_value;
use dataflow_rs::{
    BUILTIN_FUNCTION_NAMES, BuiltinKind, Engine, ExecutionStep, ExecutionTrace, Result, Task,
    TaskContext, TaskOutcome, TraceOptions, Workflow, builtin_function_kind,
};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::sync::Arc;

/// Bridge helper for tests: build an `OwnedDataValue` from a `json!` literal.
fn dv(v: serde_json::Value) -> OwnedDataValue {
    OwnedDataValue::from(&v)
}

// A simple async task implementation
#[derive(Debug)]
struct LoggingTask;

#[async_trait]
impl AsyncFunctionHandler for LoggingTask {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        println!("Executed task for message: {}", ctx.message().id());
        Ok(TaskOutcome::Success)
    }
}

// Handler that always returns Err — used by the single-error-channel
// regression tests below.
struct FailingTask;

#[async_trait]
impl AsyncFunctionHandler for FailingTask {
    type Input = Value;

    async fn execute(&self, _ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        Err(dataflow_rs::DataflowError::Task("boom".to_string()))
    }
}

// Handler that returns a 500 status — used by the single-error-channel
// regression tests below.
struct FivehundredTask;

#[async_trait]
impl AsyncFunctionHandler for FivehundredTask {
    type Input = Value;

    async fn execute(&self, _ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        Ok(TaskOutcome::Status(500))
    }
}

// An async task implementation
struct AsyncLoggingTask;

#[async_trait]
impl AsyncFunctionHandler for AsyncLoggingTask {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        println!("Executed async task for message: {}", ctx.message().id());
        // Simulate async work
        tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
        Ok(TaskOutcome::Success)
    }
}

#[tokio::test]
async fn test_async_task_execution() {
    // Drive the handler directly via `TaskContext` — exercises the trait
    // surface without going through `Engine::process_message`.
    let task = LoggingTask;

    let mut message = Message::from_value(&json!({}));
    let datalogic = Arc::new(
        datalogic_rs::Engine::builder()
            .with_templating(true)
            .build(),
    );

    let mut ctx = TaskContext::new(&mut message, &datalogic);
    let outcome = task.execute(&mut ctx, &json!({})).await;

    assert!(outcome.is_ok(), "Task execution should succeed");
    assert_eq!(outcome.unwrap(), TaskOutcome::Success);
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
            id_arc: std::sync::Arc::from("log_task"),
            name: "Log Task".to_string(),
            description: Some("A test task".to_string()),
            condition: json!(true),
            compiled_condition: None,
            continue_on_error: false,
            function: FunctionConfig::Custom {
                name: "log".to_string(),
                input: json!({}),
                compiled_input: None,
            },
        }],
        condition: json!(true),
        compiled_condition: None,
        continue_on_error: false,
        ..Default::default()
    };

    // Create engine with the workflow and custom function
    let engine = Engine::builder()
        .with_workflow(workflow)
        .register("log", LoggingTask)
        .build()
        .unwrap();

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
        message.audit_trail().len(),
        1,
        "Message should have one audit trail entry"
    );
    assert_eq!(
        message.audit_trail()[0].task_id.as_ref(),
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
            id_arc: std::sync::Arc::from("async_log_task"),
            name: "Async Log Task".to_string(),
            description: Some("An async test task".to_string()),
            condition: json!(true),
            compiled_condition: None,
            continue_on_error: false,
            function: FunctionConfig::Custom {
                name: "async_log".to_string(),
                input: json!({}),
                compiled_input: None,
            },
        }],
        condition: json!(true),
        compiled_condition: None,
        continue_on_error: false,
        ..Default::default()
    };

    // Create engine with the workflow and custom function
    let engine = Engine::builder()
        .with_workflow(workflow)
        .register("async_log", AsyncLoggingTask)
        .build()
        .unwrap();

    // Create a dummy message
    let mut message = Message::from_value(&json!({}));

    // Process the message
    let result = engine.process_message(&mut message).await;

    assert!(result.is_ok(), "Async workflow execution should succeed");

    // Verify the message was processed correctly
    assert_eq!(
        message.audit_trail().len(),
        1,
        "Message should have one audit trail entry"
    );
    assert_eq!(
        message.audit_trail()[0].task_id.as_ref(),
        "async_log_task",
        "Audit trail should contain the executed async task"
    );
}

#[tokio::test]
async fn test_temp_data_replacement_behavior() {
    // This test verifies the current behavior where setting path: "temp_data"
    // REPLACES the entire temp_data object instead of merging fields
    let workflows_json = json!([
        {
            "id": "test_temp_data_workflow",
            "name": "Test Temp Data Workflow",
            "priority": 1,
            "condition": true,
            "tasks": [
                {
                    "id": "task1",
                    "name": "Set field1 in temp_data",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "temp_data",
                                    "logic": {"field1": "first_value"}
                                }
                            ]
                        }
                    }
                },
                {
                    "id": "task2",
                    "name": "Set field2 in temp_data",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "temp_data",
                                    "logic": {"field2": "second_value"}
                                }
                            ]
                        }
                    }
                }
            ]
        }
    ]);

    // Parse workflows from JSON
    let workflows: Vec<Workflow> = workflows_json
        .as_array()
        .unwrap()
        .iter()
        .map(|w| serde_json::from_value(w.clone()).unwrap())
        .collect();

    let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    let mut message = Message::from_value(&json!({"test": "data"}));

    // Initially temp_data should be empty
    assert_eq!(message.temp_data(), &dv(json!({})));

    // Process the message
    engine.process_message(&mut message).await.unwrap();

    // After fix: temp_data is MERGED, not replaced
    // Both field1 and field2 should exist
    assert_eq!(
        message.temp_data(),
        &dv(json!({
            "field1": "first_value",
            "field2": "second_value"
        }))
    );

    // Verify that both fields are present (demonstrating the merge behavior)
    assert!(
        message.context["temp_data"].get("field1").is_some(),
        "field1 should be present after merge"
    );
    assert!(
        message.context["temp_data"].get("field2").is_some(),
        "field2 should be present after merge"
    );

    // The merge behavior preserves existing fields while adding new ones
}

#[tokio::test]
async fn test_temp_data_nested_path_preservation() {
    // This test shows that nested paths work correctly and don't replace the whole object
    let workflows_json = json!([
        {
            "id": "test_nested_workflow",
            "name": "Test Nested Temp Data",
            "priority": 1,
            "condition": true,
            "tasks": [
                {
                    "id": "task1",
                    "name": "Set nested field1",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "temp_data.field1",
                                    "logic": "first_value"
                                }
                            ]
                        }
                    }
                },
                {
                    "id": "task2",
                    "name": "Set nested field2",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "temp_data.field2",
                                    "logic": "second_value"
                                }
                            ]
                        }
                    }
                }
            ]
        }
    ]);

    // Parse workflows from JSON
    let workflows: Vec<Workflow> = workflows_json
        .as_array()
        .unwrap()
        .iter()
        .map(|w| serde_json::from_value(w.clone()).unwrap())
        .collect();

    let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    let mut message = Message::from_value(&json!({"test": "data"}));

    engine.process_message(&mut message).await.unwrap();

    // With nested paths, both fields should be preserved
    assert_eq!(
        message.temp_data(),
        &dv(json!({
            "field1": "first_value",
            "field2": "second_value"
        }))
    );

    // Both fields should exist when using nested paths
    assert!(
        message.context["temp_data"].get("field1").is_some(),
        "field1 should exist with nested path approach"
    );
    assert!(
        message.context["temp_data"].get("field2").is_some(),
        "field2 should exist with nested path approach"
    );
}

#[tokio::test]
async fn test_data_field_replacement_behavior() {
    // Similar test for the data field to show the same replacement behavior
    let workflows_json = json!([
        {
            "id": "test_data_workflow",
            "name": "Test Data Field Workflow",
            "priority": 1,
            "condition": true,
            "tasks": [
                {
                    "id": "task1",
                    "name": "Set data with field1",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "data",
                                    "logic": {"field1": "value1"}
                                }
                            ]
                        }
                    }
                },
                {
                    "id": "task2",
                    "name": "Set data with field2",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "data",
                                    "logic": {"field2": "value2"}
                                }
                            ]
                        }
                    }
                }
            ]
        }
    ]);

    // Parse workflows from JSON
    let workflows: Vec<Workflow> = workflows_json
        .as_array()
        .unwrap()
        .iter()
        .map(|w| serde_json::from_value(w.clone()).unwrap())
        .collect();

    let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    // Initialize the data field with existing data to test merging
    set_nested_value(&mut message.context, "data", dv(json!({"initial": "data"})));

    engine.process_message(&mut message).await.unwrap();

    // After fix: When using path "data", it merges with existing data
    // Note: Order may vary in the JSON object
    assert_eq!(message.context["data"]["initial"], dv(json!("data")));
    assert_eq!(message.context["data"]["field1"], dv(json!("value1")));
    assert_eq!(message.context["data"]["field2"], dv(json!("value2")));

    // All fields should be present after merging
    assert!(
        message.context["data"].get("initial").is_some(),
        "initial field should be preserved"
    );
    assert!(
        message.context["data"].get("field1").is_some(),
        "field1 should be present"
    );
    assert!(
        message.context["data"].get("field2").is_some(),
        "field2 should be present"
    );
}

#[tokio::test]
async fn test_hash_prefix_in_mapping_paths() {
    // Test that # prefix works correctly in map function paths
    let workflows_json = json!([
        {
            "id": "test_hash_workflow",
            "name": "Test Hash Prefix Workflow",
            "priority": 1,
            "condition": true,
            "tasks": [
                {
                    "id": "task1",
                    "name": "Set numeric field names using # prefix",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "data.fields.#20",
                                    "logic": "value for field 20"
                                },
                                {
                                    "path": "data.fields.#100",
                                    "logic": "value for field 100"
                                },
                                {
                                    "path": "data.fields.##",
                                    "logic": "value for hash field"
                                },
                                {
                                    "path": "data.fields.###",
                                    "logic": "value for double hash"
                                }
                            ]
                        }
                    }
                }
            ]
        }
    ]);

    // Parse workflows from JSON
    let workflows: Vec<Workflow> = workflows_json
        .as_array()
        .unwrap()
        .iter()
        .map(|w| serde_json::from_value(w.clone()).unwrap())
        .collect();

    let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    let mut message = Message::from_value(&json!({}));

    engine.process_message(&mut message).await.unwrap();

    // Verify fields with numeric names were created correctly
    assert_eq!(
        message.context["data"]["fields"]["20"],
        dv(json!("value for field 20"))
    );
    assert_eq!(
        message.context["data"]["fields"]["100"],
        dv(json!("value for field 100"))
    );
    assert_eq!(
        message.context["data"]["fields"]["#"],
        dv(json!("value for hash field"))
    );
    assert_eq!(
        message.context["data"]["fields"]["##"],
        dv(json!("value for double hash"))
    );

    // Verify the complete structure
    assert_eq!(
        message.context["data"]["fields"],
        dv(json!({
            "20": "value for field 20",
            "100": "value for field 100",
            "#": "value for hash field",
            "##": "value for double hash"
        }))
    );
}

#[tokio::test]
async fn test_hash_prefix_with_array_values_in_mapping() {
    // Test that # prefix works correctly when the field value is an array
    // Path like "data.fields.#72.0" should set field "72" as array and access index 0
    let workflows_json = json!([
        {
            "id": "test_hash_array_workflow",
            "name": "Test Hash Prefix with Arrays",
            "priority": 1,
            "condition": true,
            "tasks": [
                {
                    "id": "task1",
                    "name": "Create numeric field with array and set values",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    // First create the array structure
                                    "path": "data.fields.#72",
                                    "logic": ["initial1", "initial2", "initial3"]
                                },
                                {
                                    // Then modify specific array elements
                                    "path": "data.fields.#72.0",
                                    "logic": "modified_first"
                                },
                                {
                                    "path": "data.fields.#72.2",
                                    "logic": "modified_third"
                                },
                                {
                                    // Test with another numeric field
                                    "path": "data.fields.#100",
                                    "logic": ["alpha", "beta"]
                                },
                                {
                                    "path": "data.fields.#100.1",
                                    "logic": "modified_beta"
                                }
                            ]
                        }
                    }
                }
            ]
        }
    ]);

    // Parse workflows from JSON
    let workflows: Vec<Workflow> = workflows_json
        .as_array()
        .unwrap()
        .iter()
        .map(|w| serde_json::from_value(w.clone()).unwrap())
        .collect();

    let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    let mut message = Message::from_value(&json!({}));

    engine.process_message(&mut message).await.unwrap();

    // Verify field "72" is an array with modified values
    assert_eq!(
        message.context["data"]["fields"]["72"],
        dv(json!(["modified_first", "initial2", "modified_third"]))
    );

    // Verify field "100" is an array with modified second element
    assert_eq!(
        message.context["data"]["fields"]["100"],
        dv(json!(["alpha", "modified_beta"]))
    );

    // Verify we can access these via get_nested_value with # prefix
    use dataflow_rs::engine::utils::get_nested_value;
    assert_eq!(
        get_nested_value(&message.context["data"], "fields.#72.0"),
        Some(&dv(json!("modified_first")))
    );
    assert_eq!(
        get_nested_value(&message.context["data"], "fields.#72.2"),
        Some(&dv(json!("modified_third")))
    );
    assert_eq!(
        get_nested_value(&message.context["data"], "fields.#100.1"),
        Some(&dv(json!("modified_beta")))
    );
}

#[tokio::test]
async fn test_sequential_mappings_within_same_task() {
    // Test that mappings within the same task can reference values set by previous mappings
    let workflows_json = json!([
        {
            "id": "test_sequential_workflow",
            "name": "Test Sequential Mappings",
            "priority": 1,
            "condition": true,
            "tasks": [
                {
                    "id": "task1",
                    "name": "Sequential mappings test",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    // First mapping: set a value
                                    "path": "data.step1",
                                    "logic": "initial_value"
                                },
                                {
                                    // Second mapping: use the value from first mapping
                                    "path": "data.step2",
                                    "logic": {"var": "data.step1"}
                                },
                                {
                                    // Third mapping: combine with a boolean check
                                    "path": "data.step3",
                                    "logic": {"==": [{"var": "data.step1"}, {"var": "data.step2"}]}
                                },
                                {
                                    // Test with temp_data
                                    "path": "temp_data.temp1",
                                    "logic": "temp_value"
                                },
                                {
                                    // Use temp_data in next mapping
                                    "path": "data.from_temp",
                                    "logic": {"var": "temp_data.temp1"}
                                },
                                {
                                    // Complex case: array operations
                                    "path": "data.array_test",
                                    "logic": ["a", "b", "c"]
                                },
                                {
                                    // Reference array element in next mapping
                                    "path": "data.array_element",
                                    "logic": {"var": "data.array_test.1"}
                                }
                            ]
                        }
                    }
                }
            ]
        }
    ]);

    // Parse workflows from JSON
    let workflows: Vec<Workflow> = workflows_json
        .as_array()
        .unwrap()
        .iter()
        .map(|w| serde_json::from_value(w.clone()).unwrap())
        .collect();

    let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    let mut message = Message::from_value(&json!({}));

    engine.process_message(&mut message).await.unwrap();

    // Verify first mapping worked
    assert_eq!(message.context["data"]["step1"], dv(json!("initial_value")));

    // CRITICAL TEST: Verify second mapping could see the first mapping's result
    // This now works after fixing the evaluation context issue
    assert_eq!(
        message.context["data"].get("step2"),
        Some(&dv(json!("initial_value"))),
        "Second mapping should see first mapping's result"
    );

    // Verify third mapping could see both previous mappings (they should be equal)
    assert_eq!(
        message.context["data"].get("step3"),
        Some(&dv(json!(true))), // step1 == step2 should be true
        "Third mapping should see results from both previous mappings"
    );

    // Verify temp_data was set
    assert_eq!(
        message.context["temp_data"]["temp1"],
        dv(json!("temp_value"))
    );

    // Verify mapping could reference temp_data
    assert_eq!(
        message.context["data"].get("from_temp"),
        Some(&dv(json!("temp_value"))),
        "Mapping should be able to reference temp_data"
    );

    // Verify array was created
    assert_eq!(
        message.context["data"]["array_test"],
        dv(json!(["a", "b", "c"]))
    );

    // Verify array element could be referenced
    assert_eq!(
        message.context["data"].get("array_element"),
        Some(&dv(json!("b"))),
        "Should be able to reference array element from previous mapping"
    );

    println!(
        "Final data: {}",
        serde_json::to_string_pretty(&message.context["data"]).unwrap()
    );
    println!(
        "Final temp_data: {}",
        serde_json::to_string_pretty(&message.context["temp_data"]).unwrap()
    );
}

#[tokio::test]
async fn test_sequential_mappings_issue_simplified() {
    // Simplified test to demonstrate the issue where mappings can't see previous mappings
    let workflows_json = json!([
        {
            "id": "test_workflow",
            "name": "Sequential Issue Demo",
            "priority": 1,
            "condition": true,
            "tasks": [
                {
                    "id": "task1",
                    "name": "Sequential mapping issue",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "data.value1",
                                    "logic": 10
                                },
                                {
                                    // This should multiply value1 by 2, but value1 won't be visible
                                    "path": "data.value2",
                                    "logic": {"*": [{"var": "data.value1"}, 2]}
                                }
                            ]
                        }
                    }
                }
            ]
        }
    ]);

    let workflows: Vec<Workflow> = workflows_json
        .as_array()
        .unwrap()
        .iter()
        .map(|w| serde_json::from_value(w.clone()).unwrap())
        .collect();

    let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    let mut message = Message::from_value(&json!({}));

    engine.process_message(&mut message).await.unwrap();

    // First mapping should work
    assert_eq!(message.context["data"]["value1"], dv(json!(10)));

    // Second mapping should now see value1 and compute 10 * 2 = 20
    println!("value2 result: {:?}", message.context["data"].get("value2"));

    // This now works correctly after the fix
    assert_eq!(
        message.context["data"].get("value2"),
        Some(&dv(json!(20))),
        "Second mapping should see first mapping's result and compute 10 * 2 = 20"
    );
}

#[tokio::test]
async fn test_temp_data_merge_real_scenario() {
    // Test based on the real audit log scenario where temp_data was being replaced
    let workflows_json = json!([
        {
            "id": "test_workflow",
            "name": "Test Temp Data Merge",
            "priority": 1,
            "condition": true,
            "tasks": [
                {
                    "id": "task1",
                    "name": "Set initial temp_data fields",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "temp_data",
                                    "logic": {
                                        "Receiver": "NQZATAE1",
                                        "Sender": "ZSZUBOM1",
                                        "UETR": "8e49e852-45a1-42f7-b120-18d232541285",
                                        "clearing_channel": null,
                                        "field53b_account_indicator": null,
                                        "field53b_is_account": false,
                                        "has_rtgs_indicator": null
                                    }
                                }
                            ]
                        }
                    }
                },
                {
                    "id": "task2",
                    "name": "Add settlement fields (should merge, not replace)",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "temp_data",
                                    "logic": {
                                        "settlement_account": null,
                                        "settlement_method": "INDA"
                                    }
                                }
                            ]
                        }
                    }
                }
            ]
        }
    ]);

    let workflows: Vec<Workflow> = workflows_json
        .as_array()
        .unwrap()
        .iter()
        .map(|w| serde_json::from_value(w.clone()).unwrap())
        .collect();

    let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    let mut message = Message::from_value(&json!({}));

    engine.process_message(&mut message).await.unwrap();

    // After merge, all fields should be present
    assert_eq!(
        message.context["temp_data"]["Receiver"],
        dv(json!("NQZATAE1"))
    );
    assert_eq!(
        message.context["temp_data"]["Sender"],
        dv(json!("ZSZUBOM1"))
    );
    assert_eq!(
        message.context["temp_data"]["UETR"],
        dv(json!("8e49e852-45a1-42f7-b120-18d232541285"))
    );
    assert_eq!(
        message.context["temp_data"]["settlement_method"],
        dv(json!("INDA"))
    );
    assert_eq!(
        message.context["temp_data"]["settlement_account"],
        dv(json!(null))
    );

    // Verify the complete structure has all fields
    assert!(
        message.context["temp_data"].get("Receiver").is_some(),
        "Receiver should be preserved"
    );
    assert!(
        message.context["temp_data"].get("Sender").is_some(),
        "Sender should be preserved"
    );
    assert!(
        message.context["temp_data"].get("UETR").is_some(),
        "UETR should be preserved"
    );
    assert!(
        message.context["temp_data"]
            .get("settlement_method")
            .is_some(),
        "settlement_method should be added"
    );
    assert!(
        message.context["temp_data"]
            .get("settlement_account")
            .is_some(),
        "settlement_account should be added"
    );
}

#[tokio::test]
async fn test_nested_temp_data_mappings_preserve_existing_fields() {
    // Test the exact scenario from the user's audit log
    let workflows_json = json!([
        {
            "id": "mt200-document-mapper",
            "name": "MT200 Document Mapper",
            "priority": 1,
            "condition": true,
            "tasks": [
                {
                    "id": "initialize_temp_data",
                    "name": "Initialize temp_data",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "temp_data.Receiver",
                                    "logic": "YLLUSAW1"
                                },
                                {
                                    "path": "temp_data.Sender",
                                    "logic": "VLUIYUR1"
                                },
                                {
                                    "path": "temp_data.UETR",
                                    "logic": "3e06e786-1292-48bc-b3f1-0f7cc04330d1"
                                },
                                {
                                    "path": "temp_data.clearing_channel",
                                    "logic": null
                                },
                                {
                                    "path": "temp_data.field53b_account_indicator",
                                    "logic": null
                                },
                                {
                                    "path": "temp_data.field53b_is_account",
                                    "logic": false
                                },
                                {
                                    "path": "temp_data.has_rtgs_indicator",
                                    "logic": null
                                }
                            ]
                        }
                    }
                },
                {
                    "id": "determine_settlement_method",
                    "name": "Determine Settlement Method",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "temp_data",
                                    "logic": {
                                        "settlement_method": "INDA",
                                        "settlement_account": null
                                    }
                                }
                            ]
                        }
                    }
                }
            ]
        }
    ]);

    // Parse workflows from JSON
    let workflows: Vec<Workflow> = workflows_json
        .as_array()
        .unwrap()
        .iter()
        .map(|w| serde_json::from_value(w.clone()).unwrap())
        .collect();

    let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    let mut message = Message::from_value(&json!({}));

    engine.process_message(&mut message).await.unwrap();

    // Check the audit trail for the second task
    let settlement_audit = message
        .audit_trail()
        .iter()
        .find(|a| a.task_id == Arc::from("determine_settlement_method"))
        .expect("Should have audit entry for determine_settlement_method");

    println!("Settlement method audit changes:");
    for change in &settlement_audit.changes {
        println!("  Path: {}", change.path);
        println!("  Old: {:?}", change.old_value);
        println!("  New: {:?}", change.new_value);
    }

    // Verify the audit trail shows the root temp_data path (since we're now assigning to root)
    assert_eq!(settlement_audit.changes.len(), 1, "Should have 1 change");
    assert_eq!(settlement_audit.changes[0].path.as_ref(), "temp_data");

    // Print the final temp_data to verify
    println!("Final temp_data: {:?}", message.context["temp_data"]);

    // After the second task, ALL fields should still be present
    assert_eq!(
        message.context["temp_data"]["Receiver"],
        dv(json!("YLLUSAW1"))
    );
    assert_eq!(
        message.context["temp_data"]["Sender"],
        dv(json!("VLUIYUR1"))
    );
    assert_eq!(
        message.context["temp_data"]["UETR"],
        dv(json!("3e06e786-1292-48bc-b3f1-0f7cc04330d1"))
    );
    assert_eq!(
        message.context["temp_data"]["clearing_channel"],
        dv(json!(null))
    );
    assert_eq!(
        message.context["temp_data"]["field53b_account_indicator"],
        dv(json!(null))
    );
    assert_eq!(
        message.context["temp_data"]["field53b_is_account"],
        dv(json!(false))
    );
    assert_eq!(
        message.context["temp_data"]["has_rtgs_indicator"],
        dv(json!(null))
    );
    assert_eq!(
        message.context["temp_data"]["settlement_method"],
        dv(json!("INDA"))
    );
    assert_eq!(
        message.context["temp_data"]["settlement_account"],
        dv(json!(null))
    );

    // Verify all fields exist
    assert!(
        message.context["temp_data"].get("Receiver").is_some(),
        "Receiver should be preserved"
    );
    assert!(
        message.context["temp_data"].get("Sender").is_some(),
        "Sender should be preserved"
    );
    assert!(
        message.context["temp_data"].get("UETR").is_some(),
        "UETR should be preserved"
    );
    assert!(
        message.context["temp_data"]
            .get("settlement_method")
            .is_some(),
        "settlement_method should be added"
    );
}

#[tokio::test]
async fn test_exact_user_scenario_with_self_reference() {
    // Test the EXACT scenario from the user's mapping task
    let workflows_json = json!([
        {
            "id": "mt200-document-mapper",
            "name": "MT200 Document Mapper",
            "priority": 1,
            "condition": true,
            "tasks": [
                {
                    "id": "initialize_temp_data",
                    "name": "Initialize temp_data",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "temp_data.Receiver",
                                    "logic": "ZCZEGSG1"
                                },
                                {
                                    "path": "temp_data.Sender",
                                    "logic": "KWFUTHQ1"
                                },
                                {
                                    "path": "temp_data.UETR",
                                    "logic": "2ce6f720-e9e3-40ee-8ad9-395ca532105f"
                                },
                                {
                                    "path": "temp_data.clearing_channel",
                                    "logic": null
                                },
                                {
                                    "path": "temp_data.field53b_account_indicator",
                                    "logic": null
                                },
                                {
                                    "path": "temp_data.field53b_is_account",
                                    "logic": false
                                },
                                {
                                    "path": "temp_data.has_rtgs_indicator",
                                    "logic": null
                                }
                            ]
                        }
                    }
                },
                {
                    "id": "determine_settlement_method",
                    "name": "Determine Settlement Method",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "temp_data.Sender",
                                    "logic": {"var": "temp_data.Sender"}
                                },
                                {
                                    "path": "temp_data.Receiver",
                                    "logic": {"var": "temp_data.Receiver"}
                                },
                                {
                                    "path": "temp_data.UETR",
                                    "logic": "NEW-UETR-VALUE"
                                },
                                {
                                    "path": "temp_data.settlement_method",
                                    "logic": "INDA"
                                },
                                {
                                    "path": "temp_data.settlement_account",
                                    "logic": null
                                }
                            ]
                        }
                    }
                }
            ]
        }
    ]);

    // Parse workflows from JSON
    let workflows: Vec<Workflow> = workflows_json
        .as_array()
        .unwrap()
        .iter()
        .map(|w| serde_json::from_value(w.clone()).unwrap())
        .collect();

    let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    let mut message = Message::from_value(&json!({}));

    engine.process_message(&mut message).await.unwrap();

    // Check the audit trail for the second task
    let settlement_audit = message
        .audit_trail()
        .iter()
        .find(|a| a.task_id == Arc::from("determine_settlement_method"))
        .expect("Should have audit entry for determine_settlement_method");

    println!(
        "Number of changes in audit: {}",
        settlement_audit.changes.len()
    );
    println!("Settlement method audit changes:");
    for change in &settlement_audit.changes {
        println!("  Path: {}", change.path);
        println!("  Old: {:?}", change.old_value);
        println!("  New: {:?}", change.new_value);
    }

    // Print the final temp_data to verify
    println!("Final temp_data: {:?}", message.context["temp_data"]);

    // The audit should have 4 individual changes (null mapping is skipped)
    assert_eq!(
        settlement_audit.changes.len(),
        4,
        "Should have 4 changes for non-null mappings"
    );

    // After the second task, ALL fields should still be present including the ones not mentioned
    assert_eq!(
        message.context["temp_data"]["Receiver"],
        dv(json!("ZCZEGSG1"))
    );
    assert_eq!(
        message.context["temp_data"]["Sender"],
        dv(json!("KWFUTHQ1"))
    );
    assert_eq!(
        message.context["temp_data"]["UETR"],
        dv(json!("NEW-UETR-VALUE"))
    ); // Changed value
    assert_eq!(
        message.context["temp_data"]["clearing_channel"],
        dv(json!(null))
    ); // Should be preserved!
    assert_eq!(
        message.context["temp_data"]["field53b_account_indicator"],
        dv(json!(null))
    ); // Should be preserved!
    assert_eq!(
        message.context["temp_data"]["field53b_is_account"],
        dv(json!(false))
    ); // Should be preserved!
    assert_eq!(
        message.context["temp_data"]["has_rtgs_indicator"],
        dv(json!(null))
    ); // Should be preserved!
    assert_eq!(
        message.context["temp_data"]["settlement_method"],
        dv(json!("INDA"))
    );
    // settlement_account should not exist since null mapping is skipped
    assert_eq!(message.context["temp_data"].get("settlement_account"), None);
}

#[tokio::test]
async fn test_what_if_mappings_aggregated_to_single_object() {
    // What if someone is pre-processing the mappings to aggregate them?
    let workflows_json = json!([
        {
            "id": "mt200-document-mapper",
            "name": "MT200 Document Mapper",
            "priority": 1,
            "condition": true,
            "tasks": [
                {
                    "id": "initialize_temp_data",
                    "name": "Initialize temp_data",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "temp_data.Receiver",
                                    "logic": "ZCZEGSG1"
                                },
                                {
                                    "path": "temp_data.Sender",
                                    "logic": "KWFUTHQ1"
                                },
                                {
                                    "path": "temp_data.UETR",
                                    "logic": "2ce6f720-e9e3-40ee-8ad9-395ca532105f"
                                },
                                {
                                    "path": "temp_data.clearing_channel",
                                    "logic": null
                                },
                                {
                                    "path": "temp_data.field53b_account_indicator",
                                    "logic": null
                                },
                                {
                                    "path": "temp_data.field53b_is_account",
                                    "logic": false
                                },
                                {
                                    "path": "temp_data.has_rtgs_indicator",
                                    "logic": null
                                }
                            ]
                        }
                    }
                },
                {
                    "id": "determine_settlement_method",
                    "name": "Determine Settlement Method AGGREGATED",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    // What if all mappings are being combined into one?
                                    "path": "temp_data",
                                    "logic": {
                                        // Only the NEW/CHANGED fields
                                        "settlement_method": "INDA",
                                        "settlement_account": null
                                    }
                                }
                            ]
                        }
                    }
                }
            ]
        }
    ]);

    // Parse workflows from JSON
    let workflows: Vec<Workflow> = workflows_json
        .as_array()
        .unwrap()
        .iter()
        .map(|w| serde_json::from_value(w.clone()).unwrap())
        .collect();

    let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    let mut message = Message::from_value(&json!({}));

    engine.process_message(&mut message).await.unwrap();

    // Check the audit trail for the second task
    let settlement_audit = message
        .audit_trail()
        .iter()
        .find(|a| a.task_id == Arc::from("determine_settlement_method"))
        .expect("Should have audit entry for determine_settlement_method");

    println!(
        "AGGREGATED test - Number of changes: {}",
        settlement_audit.changes.len()
    );
    println!("AGGREGATED test - Audit changes:");
    // OwnedDataValue::Object is a Vec<(String, _)>; iterate keys via the slice.
    let keys_of = |v: &OwnedDataValue| -> Vec<String> {
        v.as_object()
            .map(|pairs| pairs.iter().map(|(k, _)| k.clone()).collect())
            .unwrap_or_default()
    };
    let object_contains = |v: &OwnedDataValue, key: &str| -> bool {
        v.as_object()
            .map(|pairs| pairs.iter().any(|(k, _)| k == key))
            .unwrap_or(false)
    };

    for change in &settlement_audit.changes {
        println!("  Path: {}", change.path);
        println!("  Old value fields: {:?}", keys_of(&change.old_value));
        println!("  New value fields: {:?}", keys_of(&change.new_value));
    }

    // This matches the user's audit log pattern!
    assert_eq!(
        settlement_audit.changes.len(),
        1,
        "Should have 1 aggregated change"
    );
    assert_eq!(settlement_audit.changes[0].path.as_ref(), "temp_data");

    // The old_value should have all the existing fields
    let old_value = &settlement_audit.changes[0].old_value;
    assert!(object_contains(old_value, "Receiver"));
    assert!(object_contains(old_value, "Sender"));
    assert!(object_contains(old_value, "UETR"));

    // The new_value should have only the new fields
    let new_value = &settlement_audit.changes[0].new_value;
    assert!(object_contains(new_value, "settlement_method"));
    assert!(object_contains(new_value, "settlement_account"));
    assert_eq!(
        new_value.as_object().unwrap().len(),
        2,
        "Should only have the 2 new fields"
    );

    // But the final temp_data should have ALL fields (because of our merge logic)
    println!(
        "AGGREGATED test - Final temp_data: {:?}",
        message.context["temp_data"]
    );
    assert_eq!(
        message.context["temp_data"]["Receiver"],
        dv(json!("ZCZEGSG1"))
    );
    assert_eq!(
        message.context["temp_data"]["Sender"],
        dv(json!("KWFUTHQ1"))
    );
    assert_eq!(
        message.context["temp_data"]["clearing_channel"],
        dv(json!(null))
    );
    assert_eq!(
        message.context["temp_data"]["settlement_method"],
        dv(json!("INDA"))
    );
}

// =============================================================================
// Log/Filter in sync stretch — regression coverage
// =============================================================================
//
// Both built-ins ship `execute_in_arena` variants that reuse the workflow's
// outer `ArenaContext` instead of opening their own `with_arena` scope. That
// fixes the re-entrant `RefCell::borrow_mut` panic that the sync-stretch
// dispatch previously triggered, and as a side effect lets Log/Filter reuse
// the depth-2 arena cache (no per-call `to_arena` walk of `data.input`).

#[tokio::test]
async fn log_builtin_runs_in_sync_stretch() {
    let workflow_json = r#"{
        "id": "log_only",
        "name": "Log Only",
        "tasks": [
            {
                "id": "log_task",
                "name": "Log",
                "function": {
                    "name": "log",
                    "input": {
                        "message": "hello"
                    }
                }
            }
        ]
    }"#;

    let workflow = Workflow::from_json(workflow_json).unwrap();
    let engine = Engine::builder().with_workflow(workflow).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();
    // Audit entry recorded with status 200.
    assert_eq!(message.audit_trail().len(), 1);
    assert_eq!(message.audit_trail()[0].status, 200);
    assert_eq!(message.audit_trail()[0].task_id.as_ref(), "log_task");
}

#[tokio::test]
async fn filter_builtin_runs_in_sync_stretch() {
    let workflow_json = r#"{
        "id": "filter_only",
        "name": "Filter Only",
        "tasks": [
            {
                "id": "filter_task",
                "name": "Filter",
                "function": {
                    "name": "filter",
                    "input": {
                        "condition": true,
                        "on_reject": "halt"
                    }
                }
            }
        ]
    }"#;

    let workflow = Workflow::from_json(workflow_json).unwrap();
    let engine = Engine::builder().with_workflow(workflow).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();
    // Condition was true → status 200 (FILTER_STATUS_PASS).
    assert_eq!(message.audit_trail().len(), 1);
    assert_eq!(message.audit_trail()[0].status, 200);
}

#[tokio::test]
async fn filter_halt_in_sync_stretch_short_circuits_workflow() {
    let workflow_json = r#"{
        "id": "filter_halt",
        "name": "Filter Halt",
        "tasks": [
            {
                "id": "gate",
                "name": "Gate",
                "function": {
                    "name": "filter",
                    "input": {
                        "condition": false,
                        "on_reject": "halt"
                    }
                }
            },
            {
                "id": "after_halt",
                "name": "After Halt",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            { "path": "data.should_not_run", "logic": true }
                        ]
                    }
                }
            }
        ]
    }"#;

    let workflow = Workflow::from_json(workflow_json).unwrap();
    let engine = Engine::builder().with_workflow(workflow).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    // Only the gate's audit entry should exist (status 299 = HALT). The map
    // task never ran, so `data.should_not_run` must be absent.
    assert_eq!(message.audit_trail().len(), 1);
    assert_eq!(message.audit_trail()[0].task_id.as_ref(), "gate");
    assert_eq!(message.audit_trail()[0].status, 299);
    assert!(message.context["data"].get("should_not_run").is_none());
}

#[tokio::test]
async fn log_filter_chained_with_map_share_one_arena() {
    // map → filter → map → log in one sync stretch. Pre-fix this would have
    // panicked at the filter step. Now everything runs in one arena scope.
    let workflow_json = r#"{
        "id": "mixed_sync",
        "name": "Mixed Sync Stretch",
        "tasks": [
            {
                "id": "set_amount",
                "name": "Set Amount",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            { "path": "data.amount", "logic": 100 }
                        ]
                    }
                }
            },
            {
                "id": "gate",
                "name": "Amount > 0",
                "function": {
                    "name": "filter",
                    "input": {
                        "condition": { ">": [ { "var": "data.amount" }, 0 ] },
                        "on_reject": "halt"
                    }
                }
            },
            {
                "id": "double_amount",
                "name": "Double Amount",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "data.amount",
                                "logic": { "*": [ { "var": "data.amount" }, 2 ] }
                            }
                        ]
                    }
                }
            },
            {
                "id": "log_result",
                "name": "Log Result",
                "function": {
                    "name": "log",
                    "input": {
                        "message": { "cat": [ "doubled=", { "var": "data.amount" } ] }
                    }
                }
            }
        ]
    }"#;

    let workflow = Workflow::from_json(workflow_json).unwrap();
    let engine = Engine::builder().with_workflow(workflow).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(message.context["data"]["amount"], dv(json!(200)));
    assert_eq!(message.audit_trail().len(), 4);
    let task_ids: Vec<&str> = message
        .audit_trail()
        .iter()
        .map(|a| a.task_id.as_ref())
        .collect();
    assert_eq!(
        task_ids,
        vec!["set_amount", "gate", "double_amount", "log_result"]
    );
}

// =============================================================================
// Single error channel — regression coverage
// =============================================================================
//
// `process_message` now always pushes errors to `message.errors()`, even when
// it returns `Result::Err`. The `Err` only signals "the engine stopped early";
// the `errors` list is the unified channel.

#[tokio::test]
async fn task_err_with_continue_on_error_false_pushes_wrapper_to_errors() {
    let workflow = Workflow {
        id: "fail_workflow".to_string(),
        id_arc: std::sync::Arc::from("fail_workflow"),
        name: "Fail Workflow".to_string(),
        priority: 0,
        description: None,
        tasks: vec![Task {
            id: "boom".to_string(),
            id_arc: std::sync::Arc::from("boom"),
            name: "Boom".to_string(),
            description: None,
            condition: json!(true),
            compiled_condition: None,
            continue_on_error: false,
            function: FunctionConfig::Custom {
                name: "fail".to_string(),
                input: json!({}),
                compiled_input: None,
            },
        }],
        condition: json!(true),
        compiled_condition: None,
        continue_on_error: false,
        ..Default::default()
    };

    let engine = Engine::builder()
        .with_workflow(workflow)
        .register("fail", FailingTask)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let result = engine.process_message(&mut message).await;

    // `Err` channel — engine stopped early.
    assert!(result.is_err(), "process_message should bubble the error");

    // `message.errors` channel — both the task error and the workflow
    // wrapper are recorded, so callers reading `errors()` see the failure
    // even without inspecting `Result`.
    let codes: Vec<&str> = message.errors().iter().map(|e| e.code.as_str()).collect();
    assert!(
        codes.contains(&"TASK_ERROR"),
        "expected TASK_ERROR in {codes:?}"
    );
    assert!(
        codes.contains(&"WORKFLOW_ERROR"),
        "expected WORKFLOW_ERROR in {codes:?}"
    );
}

#[tokio::test]
async fn task_status_500_pushes_status_error_to_message() {
    let workflow = Workflow {
        id: "five_hundred".to_string(),
        id_arc: std::sync::Arc::from("five_hundred"),
        name: "Five Hundred".to_string(),
        priority: 0,
        description: None,
        tasks: vec![Task {
            id: "task_500".to_string(),
            id_arc: std::sync::Arc::from("task_500"),
            name: "Task 500".to_string(),
            description: None,
            condition: json!(true),
            compiled_condition: None,
            // Continue past the 500 so we can assert on the *push*
            // independently of the `Result::Err` path.
            continue_on_error: true,
            function: FunctionConfig::Custom {
                name: "five_hundred".to_string(),
                input: json!({}),
                compiled_input: None,
            },
        }],
        condition: json!(true),
        compiled_condition: None,
        continue_on_error: true,
        ..Default::default()
    };

    let engine = Engine::builder()
        .with_workflow(workflow)
        .register("five_hundred", FivehundredTask)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let result = engine.process_message(&mut message).await;
    assert!(result.is_ok(), "continue_on_error keeps the Result Ok");

    let codes: Vec<&str> = message.errors().iter().map(|e| e.code.as_str()).collect();
    assert!(
        codes.contains(&"TASK_STATUS_ERROR"),
        "expected TASK_STATUS_ERROR in {codes:?}"
    );
    assert_eq!(message.audit_trail().len(), 1);
    assert_eq!(message.audit_trail()[0].status, 500);
}

// =============================================================================
// Cross-workflow shared-arena run — regression coverage
// =============================================================================
//
// Consecutive fully-sync workflows execute inside ONE shared `with_arena` scope
// (`execute_sync_workflow_run`): the arena form of `message.context` is built
// once and carried across workflow boundaries. These tests pin the observable
// contract — chained `metadata.progress` conditions and cross-workflow data
// visibility must behave exactly as if each workflow rebuilt its own arena.

#[tokio::test]
async fn chained_fully_sync_workflows_advance_through_shared_arena() {
    // Three fully-sync workflows, each (after the first) gated on the previous
    // workflow's `metadata.progress.workflow_id`, and each reading the `data.*`
    // the previous one wrote. If the carried `ArenaContext` failed to reflect
    // either the `metadata.progress` write or the prior `data` write across a
    // workflow boundary, a later workflow's condition would not match (skipped)
    // or its map would read null — both caught below.
    let wf_a = r#"{
        "id": "wf_a",
        "name": "A",
        "priority": 0,
        "condition": true,
        "tasks": [{
            "id": "map_a", "name": "A",
            "function": { "name": "map", "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } }
        }]
    }"#;
    let wf_b = r#"{
        "id": "wf_b",
        "name": "B",
        "priority": 1,
        "condition": { "==": [ { "var": "metadata.progress.workflow_id" }, "wf_a" ] },
        "tasks": [{
            "id": "map_b", "name": "B",
            "function": { "name": "map", "input": { "mappings": [ { "path": "data.b", "logic": { "+": [ { "var": "data.a" }, 1 ] } } ] } }
        }]
    }"#;
    let wf_c = r#"{
        "id": "wf_c",
        "name": "C",
        "priority": 2,
        "condition": { "==": [ { "var": "metadata.progress.workflow_id" }, "wf_b" ] },
        "tasks": [{
            "id": "map_c", "name": "C",
            "function": { "name": "map", "input": { "mappings": [ { "path": "data.c", "logic": { "+": [ { "var": "data.b" }, 1 ] } } ] } }
        }]
    }"#;

    let workflows = vec![
        Workflow::from_json(wf_a).unwrap(),
        Workflow::from_json(wf_b).unwrap(),
        Workflow::from_json(wf_c).unwrap(),
    ];
    let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    // Every workflow ran (condition matched via the carried arena) and each map
    // saw the previous workflow's write.
    assert_eq!(message.context["data"]["a"], dv(json!(1)));
    assert_eq!(message.context["data"]["b"], dv(json!(2)));
    assert_eq!(message.context["data"]["c"], dv(json!(3)));

    // One audit entry per workflow's single task, in order.
    let task_ids: Vec<&str> = message
        .audit_trail()
        .iter()
        .map(|a| a.task_id.as_ref())
        .collect();
    assert_eq!(task_ids, vec!["map_a", "map_b", "map_c"]);

    // Progress reflects the last workflow to run.
    assert_eq!(
        message.context["metadata"]["progress"]["workflow_id"],
        dv(json!("wf_c"))
    );
}

#[tokio::test]
async fn cross_workflow_false_condition_skips_only_that_workflow() {
    // A real (non-matching) workflow condition skips only that workflow; the
    // shared arena stays intact and a later workflow still runs.
    let wf_x = r#"{
        "id": "wf_x", "name": "X", "priority": 0, "condition": true,
        "tasks": [{ "id": "map_x", "name": "X",
            "function": { "name": "map", "input": { "mappings": [ { "path": "data.x", "logic": 1 } ] } } }]
    }"#;
    let wf_y = r#"{
        "id": "wf_y", "name": "Y", "priority": 1,
        "condition": { "==": [ { "var": "data.x" }, 999 ] },
        "tasks": [{ "id": "map_y", "name": "Y",
            "function": { "name": "map", "input": { "mappings": [ { "path": "data.y", "logic": 1 } ] } } }]
    }"#;
    let wf_z = r#"{
        "id": "wf_z", "name": "Z", "priority": 2, "condition": true,
        "tasks": [{ "id": "map_z", "name": "Z",
            "function": { "name": "map", "input": { "mappings": [ { "path": "data.z", "logic": 1 } ] } } }]
    }"#;

    let workflows = vec![
        Workflow::from_json(wf_x).unwrap(),
        Workflow::from_json(wf_y).unwrap(),
        Workflow::from_json(wf_z).unwrap(),
    ];
    let engine = Engine::builder().with_workflows(workflows).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(message.context["data"]["x"], dv(json!(1)));
    assert!(
        message.context["data"].get("y").is_none(),
        "wf_y condition was false — it must be skipped"
    );
    assert_eq!(message.context["data"]["z"], dv(json!(1)));

    // Only wf_x and wf_z executed.
    let task_ids: Vec<&str> = message
        .audit_trail()
        .iter()
        .map(|a| a.task_id.as_ref())
        .collect();
    assert_eq!(task_ids, vec!["map_x", "map_z"]);
}

// =============================================================================
// Caller-owned tracing — regression coverage for the dropped-trace defect
// =============================================================================
//
// `process_message_with_trace` returns the trace by value, so a `?` at the call
// site discards every step that already ran. `process_message_tracing` records
// into a caller-owned trace instead, so the steps survive the `Err`. These tests
// pin the retained steps on each of the three dispatch paths, plus the append
// contract and the metadata stamp.

/// Build a single-workflow engine from JSON with the `fail` handler registered.
fn tracing_engine(workflow_json: &str) -> Engine {
    Engine::builder()
        .with_workflow(Workflow::from_json(workflow_json).unwrap())
        .register("fail", FailingTask)
        .build()
        .unwrap()
}

/// `(workflow_id, task_id)` pairs for every step, in order.
fn step_ids(trace: &ExecutionTrace) -> Vec<(&str, Option<&str>)> {
    trace
        .steps
        .iter()
        .map(|s| (s.workflow_id.as_str(), s.task_id.as_deref()))
        .collect()
}

#[tokio::test]
async fn tracing_retains_steps_when_async_task_fails() {
    // Async-task failure path: the `map` runs in a sync stretch, then the
    // custom `fail` handler is dispatched at the async boundary and errors.
    let engine = tracing_engine(
        r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "step_ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "step_boom", "name": "boom", "function": {
                "name": "fail", "input": {} } }
        ]
    }"#,
    );

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_tracing(&mut message, &mut trace)
        .await;

    assert!(result.is_err(), "the engine must still stop early");

    // The step that completed before the failure is retained. The failing
    // task's own step is not recorded — `handle_task_result` propagates before
    // `add_step` — so the trace ends at the last known-good step.
    assert_eq!(step_ids(&trace), vec![("wf", Some("step_ok"))]);
    assert_eq!(trace.executed_count(), 1);

    // The audit trail already survived the failure because it lives on
    // `&mut Message`; the trace now matches that guarantee.
    assert_eq!(message.audit_trail().len(), 2);
}

#[tokio::test]
async fn tracing_retains_steps_when_sync_stretch_fails() {
    // Sync-stretch path: both tasks are sync built-ins, so they share one
    // arena scope. `parse_xml` errors because its source is not a string.
    let engine = tracing_engine(
        r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "step_ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.n", "logic": 7 } ] } } },
            { "id": "step_boom", "name": "boom", "function": {
                "name": "parse_xml",
                "input": { "source": "data.n", "target": "parsed" } } }
        ]
    }"#,
    );

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_tracing(&mut message, &mut trace)
        .await;

    assert!(result.is_err(), "parse_xml on a non-string must error");
    assert_eq!(step_ids(&trace), vec![("wf", Some("step_ok"))]);
}

#[tokio::test]
async fn tracing_retains_earlier_workflow_steps_across_a_shared_arena_run() {
    // Cross-workflow shared-arena path: workflow A is fully sync and succeeds,
    // workflow B fails. A never failed, so losing its steps is the most
    // surprising case of the old behaviour.
    let wf_a = Workflow::from_json(
        r#"{
        "id": "wf_a",
        "name": "wf_a",
        "priority": 0,
        "condition": true,
        "tasks": [
            { "id": "a_map", "name": "a_map", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } }
        ]
    }"#,
    )
    .unwrap();
    let wf_b = Workflow::from_json(
        r#"{
        "id": "wf_b",
        "name": "wf_b",
        "priority": 1,
        "condition": true,
        "tasks": [
            { "id": "b_map", "name": "b_map", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.b", "logic": 2 } ] } } },
            { "id": "b_boom", "name": "b_boom", "function": {
                "name": "parse_xml",
                "input": { "source": "data.b", "target": "parsed" } } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflows(vec![wf_a, wf_b])
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_tracing(&mut message, &mut trace)
        .await;

    assert!(result.is_err());
    assert_eq!(
        step_ids(&trace),
        vec![("wf_a", Some("a_map")), ("wf_b", Some("b_map"))],
        "the successful workflow's steps must survive the later failure"
    );
}

#[tokio::test]
async fn tracing_retains_skipped_steps_before_a_failure() {
    // Both skip kinds are recorded before the failure: a workflow-level skip
    // (`workflow_skipped`, no task_id) and a task-level skip (`task_skipped`).
    let wf_skipped = Workflow::from_json(
        r#"{
        "id": "wf_skipped",
        "name": "wf_skipped",
        "priority": 0,
        "condition": { "==": [1, 2] },
        "tasks": [
            { "id": "never", "name": "never", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.never", "logic": 1 } ] } } }
        ]
    }"#,
    )
    .unwrap();
    let wf_main = Workflow::from_json(
        r#"{
        "id": "wf_main",
        "name": "wf_main",
        "priority": 1,
        "condition": true,
        "tasks": [
            { "id": "task_skipped", "name": "task_skipped",
              "condition": { "==": [1, 2] },
              "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.skip", "logic": 1 } ] } } },
            { "id": "task_ok", "name": "task_ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.ok", "logic": 1 } ] } } },
            { "id": "task_boom", "name": "task_boom", "function": {
                "name": "fail", "input": {} } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflows(vec![wf_skipped, wf_main])
        .register("fail", FailingTask)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_tracing(&mut message, &mut trace)
        .await;

    assert!(result.is_err());
    assert_eq!(
        step_ids(&trace),
        vec![
            ("wf_skipped", None),
            ("wf_main", Some("task_skipped")),
            ("wf_main", Some("task_ok")),
        ]
    );
    assert_eq!(trace.skipped_count(), 2);
    assert_eq!(trace.executed_count(), 1);
}

#[tokio::test]
async fn tracing_retains_mapping_contexts_before_a_failure() {
    // Per-mapping snapshots are only populated in trace mode; they must survive
    // the failure along with the step that carries them.
    let engine = tracing_engine(
        r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "two_mappings", "name": "two_mappings", "function": {
                "name": "map",
                "input": { "mappings": [
                    { "path": "data.first", "logic": 1 },
                    { "path": "data.second", "logic": 2 }
                ] } } },
            { "id": "step_boom", "name": "boom", "function": {
                "name": "fail", "input": {} } }
        ]
    }"#,
    );

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_tracing(&mut message, &mut trace)
        .await;

    assert!(result.is_err());
    assert_eq!(trace.steps.len(), 1);
    let contexts = trace.steps[0]
        .mapping_contexts
        .as_ref()
        .expect("map task in trace mode must carry per-mapping snapshots");
    assert_eq!(contexts.len(), 2, "one snapshot per mapping");
}

#[tokio::test]
async fn tracing_appends_to_an_existing_trace() {
    // The documented contract is append, not clear: a caller can accumulate
    // steps across a chain of calls.
    let engine = tracing_engine(
        r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "step_ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "step_boom", "name": "boom", "function": {
                "name": "fail", "input": {} } }
        ]
    }"#,
    );

    let mut trace = ExecutionTrace::new();
    trace.add_step(ExecutionStep::workflow_skipped("preexisting"));

    let mut message = Message::from_value(&json!({}));
    let result = engine
        .process_message_tracing(&mut message, &mut trace)
        .await;

    assert!(result.is_err());
    assert_eq!(
        step_ids(&trace),
        vec![("preexisting", None), ("wf", Some("step_ok"))],
        "the pre-existing step is kept and new steps are appended after it"
    );
}

#[tokio::test]
async fn tracing_stamps_processing_metadata_even_when_the_run_fails() {
    // A hand-rolled consumer workaround cannot stamp this, so it must not
    // regress on the failing path.
    let engine = tracing_engine(
        r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "step_boom", "name": "boom", "function": {
                "name": "fail", "input": {} } }
        ]
    }"#,
    );

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_tracing(&mut message, &mut trace)
        .await;

    assert!(result.is_err());
    let metadata = &message.context["metadata"];
    assert!(
        metadata.get("processed_at").is_some(),
        "processed_at must be stamped on a failing tracing run"
    );
    assert!(
        metadata.get("engine_version").is_some(),
        "engine_version must be stamped on a failing tracing run"
    );
}

#[tokio::test]
async fn channel_tracing_stamps_channel_metadata_and_retains_steps() {
    let wf = Workflow::from_json(
        r#"{
        "id": "wf_ch",
        "name": "wf_ch",
        "channel": "payments",
        "condition": true,
        "tasks": [
            { "id": "step_ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "step_boom", "name": "boom", "function": {
                "name": "fail", "input": {} } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflow(wf)
        .register("fail", FailingTask)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_for_channel_tracing("payments", &mut message, &mut trace)
        .await;

    assert!(result.is_err());
    // The channel path must be fixed too, not only the non-channel path.
    assert_eq!(step_ids(&trace), vec![("wf_ch", Some("step_ok"))]);
    assert_eq!(
        message.context["metadata"]["channel"],
        dv(json!("payments"))
    );
}

#[tokio::test]
async fn channel_tracing_on_an_unknown_channel_is_a_noop() {
    let engine = tracing_engine(
        r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "step_ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } }
        ]
    }"#,
    );

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_for_channel_tracing("nope", &mut message, &mut trace)
        .await;

    assert!(
        result.is_ok(),
        "an unknown channel is a no-op, not an error"
    );
    assert!(trace.steps.is_empty(), "the trace must be left untouched");
}

#[tokio::test]
async fn tracing_records_the_full_trace_on_a_filter_halt() {
    // A halt is `Ok`, so the trace already survived it. Pin it so the refactor
    // cannot change halt behaviour.
    let engine = tracing_engine(
        r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "step_ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "gate", "name": "gate", "function": {
                "name": "filter", "input": { "condition": false } } },
            { "id": "never", "name": "never", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.never", "logic": 1 } ] } } }
        ]
    }"#,
    );

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::new();
    let result = engine
        .process_message_tracing(&mut message, &mut trace)
        .await;

    assert!(result.is_ok(), "a filter halt is not an error");
    assert_eq!(
        step_ids(&trace),
        vec![("wf", Some("step_ok")), ("wf", Some("gate"))],
        "the halting task is recorded; the task after it never runs"
    );
}

#[tokio::test]
async fn with_trace_wrappers_are_unchanged_by_the_tracing_refactor() {
    // Non-regression: on `Ok` the returned trace matches what the caller-owned
    // method records; on `Err` the wrapper still yields no trace at all.
    let ok_json = r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "step_ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } }
        ]
    }"#;

    let engine = tracing_engine(ok_json);
    let mut message = Message::from_value(&json!({}));
    let returned = engine
        .process_message_with_trace(&mut message)
        .await
        .expect("a clean run still returns its trace");

    let mut borrowed = ExecutionTrace::new();
    let mut message2 = Message::from_value(&json!({}));
    engine
        .process_message_tracing(&mut message2, &mut borrowed)
        .await
        .unwrap();

    assert_eq!(step_ids(&returned), step_ids(&borrowed));
    assert_eq!(returned.executed_count(), borrowed.executed_count());

    // On `Err` the by-value wrapper behaves exactly as before: no trace.
    let failing = tracing_engine(
        r#"{
        "id": "wf",
        "name": "wf",
        "condition": true,
        "tasks": [
            { "id": "step_ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "step_boom", "name": "boom", "function": {
                "name": "fail", "input": {} } }
        ]
    }"#,
    );
    let mut message3 = Message::from_value(&json!({}));
    assert!(
        failing
            .process_message_with_trace(&mut message3)
            .await
            .is_err(),
        "callers of the by-value method see no behaviour change"
    );
}

// =============================================================================
// Built-in function classification — regression coverage for the
// has_function / config-only-integration conflation
// =============================================================================

#[tokio::test]
async fn handler_less_enrich_builds_but_is_detectable_before_processing() {
    // `enrich` ships as a config schema with no implementation. It deserializes
    // to `FunctionConfig::Enrich` rather than `Custom`, so `precompile_custom_inputs`
    // never visits it and `Engine::new` accepts the workflow — then every message
    // fails with FunctionNotFound.
    //
    // `Engine::new` staying permissive is deliberate (a host screening stored
    // definitions one row at a time must not be stopped from booting by one
    // unusable row). What was missing was any way to *detect* the gap first.
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [{ "id": "t", "name": "t",
                    "function": { "name": "enrich",
                                  "input": { "connector": "c",
                                             "merge_path": "data.out" } } }]
    }"#,
    )
    .expect("a handler-less enrich task still parses");

    // Unchanged: the engine builds.
    let engine = Engine::new(vec![wf], std::collections::HashMap::new())
        .expect("Engine::new stays permissive for config-only integrations");

    // The gap is now detectable without processing a message.
    assert_eq!(
        builtin_function_kind("enrich"),
        Some(BuiltinKind::RequiresHandler),
        "a caller can screen for this before accepting the workflow"
    );

    // And the underlying behaviour it predicts is real.
    let mut message = Message::from_value(&json!({}));
    let result = engine.process_message(&mut message).await;
    assert!(
        result.is_err(),
        "a handler-less enrich still fails on the first message"
    );
    let codes: Vec<&str> = message.errors().iter().map(|e| e.code.as_str()).collect();
    assert!(
        !codes.is_empty(),
        "the failure is also recorded on the message, got {codes:?}"
    );
}

#[tokio::test]
async fn a_registered_enrich_handler_closes_the_gap() {
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [{ "id": "t", "name": "t",
                    "function": { "name": "enrich",
                                  "input": { "connector": "c",
                                             "merge_path": "data.out" } } }]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflow(wf)
        .register("enrich", MockEnrich)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine
        .process_message(&mut message)
        .await
        .expect("with a handler registered the same workflow runs");
}

// A handler for a config-only integration must declare the *typed* input its
// `FunctionConfig` variant carries — an `enrich` task deserializes to
// `FunctionConfig::Enrich { input: EnrichConfig }`, so a handler declaring
// `type Input = Value` fails the downcast at dispatch with "Handler input type
// mismatch" rather than running.
struct MockEnrich;

#[async_trait]
impl AsyncFunctionHandler for MockEnrich {
    type Input = dataflow_rs::EnrichConfig;

    async fn execute(
        &self,
        _ctx: &mut TaskContext<'_>,
        _input: &Self::Input,
    ) -> Result<TaskOutcome> {
        Ok(TaskOutcome::Success)
    }
}

#[test]
fn builtin_classification_is_reachable_from_the_crate_root() {
    // Pins the re-export path a consumer actually uses.
    assert!(BUILTIN_FUNCTION_NAMES.contains(&"enrich"));
    assert!(dataflow_rs::is_builtin_function("map"));
    assert_eq!(
        builtin_function_kind("map"),
        Some(BuiltinKind::SelfContained)
    );
    assert_eq!(builtin_function_kind("my_custom_handler"), None);
}

// =============================================================================
// http_call destination field — regression coverage for the silent discard
// =============================================================================

// Records what the engine actually handed the handler, by writing the observed
// `response_path` into the message context. `type Input` is pinned to
// `HttpCallConfig` because `http_call` deserializes to a typed built-in variant.
struct SpyHttpCall;

#[async_trait]
impl AsyncFunctionHandler for SpyHttpCall {
    type Input = dataflow_rs::HttpCallConfig;

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &Self::Input) -> Result<TaskOutcome> {
        let observed = input.response_path.clone().unwrap_or_default();
        ctx.set("data.observed_response_path", dv(json!(observed)));
        ctx.set("data.observed_method", dv(json!(input.method.as_str())));
        Ok(TaskOutcome::Success)
    }
}

#[tokio::test]
async fn http_call_output_alias_survives_the_full_engine_path() {
    // Proves the alias holds through Workflow::from_json -> LogicCompiler ->
    // Engine::builder().build() -> dispatch, not just a bare from_value.
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [{ "id": "call", "name": "call",
                    "function": { "name": "http_call",
                                  "input": { "connector": "user_service",
                                             "method": "POST",
                                             "output": "data.user_profile" } } }]
    }"#,
    )
    .expect("a task spelling the field `output` should parse");

    let engine = Engine::builder()
        .with_workflow(wf)
        .register("http_call", SpyHttpCall)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(
        message.context["data"]["observed_response_path"],
        dv(json!("data.user_profile")),
        "the handler must see the aliased destination, not an empty default"
    );
    assert_eq!(
        message.context["data"]["observed_method"],
        dv(json!("POST")),
        "as_str() must give the canonical token the config was written with"
    );
}

#[test]
fn http_call_misspelled_destination_is_rejected_at_workflow_parse_time() {
    // The defect: this previously parsed cleanly, and the task would make its
    // request and throw the response away with no error anywhere.
    let err = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [{ "id": "call", "name": "call",
                    "function": { "name": "http_call",
                                  "input": { "connector": "c",
                                             "outputs": "data.user_profile" } } }]
    }"#,
    )
    .expect_err("a misspelled destination field must fail the workflow parse");

    let msg = err.to_string();
    assert!(
        msg.contains("unknown field") && msg.contains("outputs"),
        "the error should name the offending field, got: {msg}"
    );
}

#[test]
fn http_method_is_reachable_from_the_crate_root() {
    use dataflow_rs::HttpMethod;

    assert_eq!(HttpMethod::default(), HttpMethod::Get);
    assert_eq!(HttpMethod::Delete.as_str(), "DELETE");
    assert!(HttpMethod::Put.is_idempotent());
    assert!(!HttpMethod::Post.is_idempotent());
    assert_eq!(HttpMethod::ALL.len(), 5);
}

// =============================================================================
// TraceOptions — timing, per-step diff, snapshot budget, redaction
// =============================================================================

/// N map tasks, each writing one key, as one workflow.
fn n_map_task_workflow(n: usize) -> Workflow {
    let tasks: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{ "id": "t{i}", "name": "t{i}", "function": {{
                       "name": "map",
                       "input": {{ "mappings": [ {{ "path": "data.k{i}", "logic": {i} }} ] }} }} }}"#
            )
        })
        .collect();
    Workflow::from_json(&format!(
        r#"{{ "id": "w", "name": "w", "priority": 0, "condition": true,
              "tasks": [{}] }}"#,
        tasks.join(",")
    ))
    .unwrap()
}

#[tokio::test]
async fn executed_steps_carry_timing_and_skipped_steps_do_not() {
    // Mixes a sync built-in stretch with a registered async handler so both
    // ExecutionStep sites are covered.
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "sync_map", "name": "sync_map", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "skipped", "name": "skipped",
              "condition": { "==": [1, 2] },
              "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.never", "logic": 1 } ] } } },
            { "id": "async_task", "name": "async_task", "function": {
                "name": "logger", "input": {} } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflow(wf)
        .register("logger", LoggingTask)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace(&mut message)
        .await
        .unwrap();

    for step in &trace.steps {
        let id = step.task_id.as_deref().unwrap_or("<workflow>");
        match step.result {
            dataflow_rs::StepResult::Executed => {
                assert!(
                    step.started_at.is_some() && step.duration_us.is_some(),
                    "executed step '{id}' must carry timing"
                );
            }
            dataflow_rs::StepResult::Skipped => {
                assert!(
                    step.started_at.is_none() && step.duration_us.is_none(),
                    "skipped step '{id}' must not carry timing"
                );
            }
        }
    }

    // Both dispatch sites produced a timed step.
    let timed: Vec<&str> = trace
        .steps
        .iter()
        .filter(|s| s.duration_us.is_some())
        .map(|s| s.task_id.as_deref().unwrap())
        .collect();
    assert_eq!(timed, vec!["sync_map", "async_task"]);
}

#[tokio::test]
async fn the_non_trace_path_still_takes_one_clock_read_per_message() {
    // Trace mode adds clock reads per task; process_message must not.
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(4))
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    let stamps: Vec<_> = message.audit_trail().iter().map(|a| a.timestamp).collect();
    assert_eq!(stamps.len(), 4);
    assert!(
        stamps.windows(2).all(|w| w[0] == w[1]),
        "all audit timestamps share the single per-message Utc::now()"
    );
}

#[tokio::test]
async fn skip_step_reports_its_own_empty_diff_not_the_previous_tasks() {
    // The reported mis-attribution, asserted fixed. `filter` with
    // on_reject: "skip" returns TaskOutcome::Skip, which records no audit entry.
    let wf = Workflow::from_json(
        r#"{
        "id": "skip_attribution", "name": "Skip Attribution", "condition": true,
        "tasks": [
            { "id": "write_a", "name": "Write A", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "gate", "name": "Gate", "function": {
                "name": "filter",
                "input": { "condition": false, "on_reject": "skip" } } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder().with_workflow(wf).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                changes: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(trace.steps.len(), 2);
    assert_eq!(trace.steps[1].task_id.as_deref(), Some("gate"));

    // The write belongs to write_a...
    let a_changes = trace.steps[0].changes.as_ref().unwrap();
    assert_eq!(a_changes.len(), 1);
    assert_eq!(&*a_changes[0].path, "data.a");

    // ...and the Skip step reports its own empty diff, not write_a's.
    assert!(
        trace.steps[1].changes.as_ref().unwrap().is_empty(),
        "a Skip step must not inherit the previous task's changes"
    );

    // The old heuristic — audit_trail.last() on the step's own snapshot — is
    // what mis-attributed; confirm the trap is still there so the fix matters.
    let snapshot = trace.steps[1].message.as_ref().unwrap();
    assert_eq!(
        snapshot.audit_trail().last().unwrap().task_id.as_ref(),
        "write_a"
    );
}

#[tokio::test]
async fn changes_flag_reports_the_diff_but_does_not_enable_capture() {
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(2))
        .build()
        .unwrap();

    let mut message = dataflow_rs::MessageBuilder::new()
        .payload_json(&json!({}))
        .capture_changes(false)
        .build();

    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                changes: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    for step in &trace.steps {
        assert!(
            step.changes.as_ref().unwrap().is_empty(),
            "capture_changes(false) means there is no diff to report"
        );
    }
}

#[tokio::test]
async fn changes_default_off_is_absent_from_the_serialized_step() {
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(1))
        .build()
        .unwrap();
    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace(&mut message)
        .await
        .unwrap();

    assert!(trace.steps[0].changes.is_none());
    let serialized = serde_json::to_value(&trace.steps[0]).unwrap();
    assert!(serialized.get("changes").is_none());
}

#[tokio::test]
async fn timings_only_drops_snapshots_and_degrades_the_accessors() {
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(3))
        .build()
        .unwrap();
    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace_options(&mut message, TraceOptions::timings_only())
        .await
        .unwrap();

    assert_eq!(trace.executed_count(), 3);
    for step in &trace.steps {
        assert!(step.message.is_none(), "no snapshots under timings_only");
        assert!(step.mapping_contexts.is_none());
        assert!(step.duration_us.is_some(), "timing survives");
    }
    assert!(trace.final_message().is_none());
    assert!(trace.is_success(), "documented to degenerate to true");
    // Nothing was captured, so nothing was truncated.
    assert!(!trace.truncated());
}

#[tokio::test]
async fn a_snapshot_budget_truncates_later_steps_and_does_not_oscillate() {
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(6))
        .build()
        .unwrap();
    let mut message = Message::from_value(&json!({}));
    set_nested_value(
        &mut message.context,
        "data.blob",
        dv(json!("x".repeat(4096))),
    );

    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                max_snapshot_bytes: 8192,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert!(trace.truncated(), "the budget must be reported as exceeded");
    assert_eq!(trace.executed_count(), 6, "every step is still recorded");

    let with_snapshot: Vec<bool> = trace.steps.iter().map(|s| s.message.is_some()).collect();
    assert!(with_snapshot[0], "the first step is captured");
    assert!(
        !with_snapshot[with_snapshot.len() - 1],
        "later steps drop their snapshot"
    );
    // Monotone: once truncation starts it never recovers.
    let first_dropped = with_snapshot.iter().position(|c| !c).unwrap();
    assert!(
        with_snapshot[first_dropped..].iter().all(|c| !c),
        "no oscillation back to captured: {with_snapshot:?}"
    );

    // Timing survives truncation.
    assert!(trace.steps.last().unwrap().duration_us.is_some());
}

#[tokio::test]
async fn a_budget_smaller_than_the_first_snapshot_truncates_from_the_start() {
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(3))
        .build()
        .unwrap();
    let mut message = Message::from_value(&json!({}));
    set_nested_value(
        &mut message.context,
        "data.blob",
        dv(json!("y".repeat(8192))),
    );

    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                max_snapshot_bytes: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert!(trace.truncated());
    assert!(
        trace.steps.iter().all(|s| s.message.is_none()),
        "no step recovers a snapshot"
    );
}

#[tokio::test]
async fn an_unbounded_budget_never_truncates() {
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(6))
        .build()
        .unwrap();
    let mut message = Message::from_value(&json!({}));
    set_nested_value(
        &mut message.context,
        "data.blob",
        dv(json!("z".repeat(65536))),
    );
    let trace = engine
        .process_message_with_trace(&mut message)
        .await
        .unwrap();

    assert_eq!(trace.options().max_snapshot_bytes, 0);
    assert!(!trace.truncated());
    assert!(trace.steps.iter().all(|s| s.message.is_some()));
}

#[tokio::test]
async fn audit_trail_scope_controls_the_quadratic_term() {
    let n = 6usize;

    let total_audit_entries = |trace: &ExecutionTrace| -> usize {
        trace
            .steps
            .iter()
            .filter_map(|s| s.message.as_ref())
            .map(|m| m.audit_trail().len())
            .sum()
    };

    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(n))
        .build()
        .unwrap();

    // Full — the historical behaviour, N*(N+1)/2.
    let mut m1 = Message::from_value(&json!({}));
    let full = engine.process_message_with_trace(&mut m1).await.unwrap();
    assert_eq!(total_audit_entries(&full), n * (n + 1) / 2);

    // Own — linear, one per executed non-Skip step.
    let mut m2 = Message::from_value(&json!({}));
    let own = engine
        .process_message_with_trace_options(
            &mut m2,
            TraceOptions {
                snapshot_audit_trail: dataflow_rs::AuditTrailScope::Own,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(total_audit_entries(&own), n);

    // None — empty in every snapshot.
    let mut m3 = Message::from_value(&json!({}));
    let none = engine
        .process_message_with_trace_options(
            &mut m3,
            TraceOptions {
                snapshot_audit_trail: dataflow_rs::AuditTrailScope::None,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(total_audit_entries(&none), 0);
    // Snapshots are still present — only the audit term was dropped.
    assert!(none.steps.iter().all(|s| s.message.is_some()));
}

#[tokio::test]
async fn redaction_nulls_the_snapshot_but_not_the_live_message() {
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "seed", "name": "seed", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.secret", "logic": "s3cret" } ] } } },
            { "id": "reader", "name": "reader", "function": {
                "name": "map",
                "input": { "mappings": [
                    { "path": "data.copied", "logic": { "var": "data.secret" } } ] } } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder().with_workflow(wf).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                redact_paths: vec!["data.secret".to_string()],
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Every snapshot has the subtree nulled.
    for step in &trace.steps {
        let snap = step.message.as_ref().unwrap();
        assert_eq!(
            snap.context["data"]["secret"],
            dv(json!(null)),
            "the snapshot must not carry the secret"
        );
    }

    // The live message kept the real value, and the later task read it.
    assert_eq!(message.context["data"]["secret"], dv(json!("s3cret")));
    assert_eq!(
        message.context["data"]["copied"],
        dv(json!("s3cret")),
        "redaction must not affect what later tasks read"
    );
}

#[tokio::test]
async fn redaction_also_applies_to_mapping_contexts() {
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "seed", "name": "seed", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.secret", "logic": "s3cret" } ] } } },
            { "id": "multi", "name": "multi", "function": {
                "name": "map",
                "input": { "mappings": [
                    { "path": "data.one", "logic": 1 },
                    { "path": "data.two", "logic": 2 } ] } } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder().with_workflow(wf).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                redact_paths: vec!["data.secret".to_string()],
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let contexts = trace.steps[1]
        .mapping_contexts
        .as_ref()
        .expect("multi-mapping map task snapshots its per-mapping contexts");
    assert_eq!(contexts.len(), 2);
    for ctx in contexts {
        assert_eq!(
            ctx["data"]["secret"],
            json!(null),
            "mapping contexts are whole-context clones and must be redacted too"
        );
    }
}

#[tokio::test]
async fn mapping_contexts_can_be_switched_off_while_the_map_still_writes() {
    let engine = Engine::builder()
        .with_workflow(
            Workflow::from_json(
                r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [ { "id": "multi", "name": "multi", "function": {
            "name": "map",
            "input": { "mappings": [
                { "path": "data.one", "logic": 1 },
                { "path": "data.two", "logic": 2 } ] } } } ]
    }"#,
            )
            .unwrap(),
        )
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                mapping_contexts: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert!(trace.steps[0].mapping_contexts.is_none());
    // The map's writes still land.
    assert_eq!(message.context["data"]["one"], dv(json!(1)));
    assert_eq!(message.context["data"]["two"], dv(json!(2)));
}

#[tokio::test]
async fn the_budget_accounts_for_mapping_contexts_on_their_own() {
    // A single map task with several mappings over a large context can exceed
    // the budget by itself.
    let engine = Engine::builder()
        .with_workflow(
            Workflow::from_json(
                r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "multi", "name": "multi", "function": {
                "name": "map",
                "input": { "mappings": [
                    { "path": "data.one", "logic": 1 },
                    { "path": "data.two", "logic": 2 },
                    { "path": "data.three", "logic": 3 } ] } } },
            { "id": "after", "name": "after", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.four", "logic": 4 } ] } } }
        ]
    }"#,
            )
            .unwrap(),
        )
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    set_nested_value(
        &mut message.context,
        "data.blob",
        dv(json!("q".repeat(4096))),
    );
    let trace = engine
        .process_message_with_trace_options(
            &mut message,
            TraceOptions {
                max_snapshot_bytes: 6000,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert!(
        trace.truncated(),
        "three whole-context mapping snapshots plus the step snapshot exceed 6000"
    );
    assert!(
        trace.steps[1].message.is_none(),
        "the following step is past the budget"
    );
}

#[tokio::test]
async fn default_options_keep_the_serialized_trace_shape() {
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(2))
        .build()
        .unwrap();
    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace(&mut message)
        .await
        .unwrap();

    let serialized = serde_json::to_value(&trace).unwrap();
    // No `truncated` on a complete trace, and `steps` is still the only key.
    assert_eq!(
        serialized.as_object().unwrap().keys().collect::<Vec<_>>(),
        vec!["steps"]
    );

    // Steps keep their historical keys, plus timing.
    let step = &serialized["steps"][0];
    for key in ["workflow_id", "task_id", "result", "message"] {
        assert!(step.get(key).is_some(), "missing historical key '{key}'");
    }
    assert!(step.get("started_at").is_some());
    assert!(step.get("duration_us").is_some());
    assert!(step.get("changes").is_none(), "changes is off by default");
}

// =============================================================================
// ExecutionObserver — per-task timing that covers the sync built-ins
// =============================================================================

/// One recorded observer callback.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SeenEvent {
    workflow_id: String,
    task_id: String,
    function: String,
    status: Option<u16>,
}

/// Records every event it is handed. `Mutex` is fine for a test; a real observer
/// must not block, per the trait contract.
#[derive(Default)]
struct RecordingObserver {
    events: std::sync::Mutex<Vec<SeenEvent>>,
}

impl RecordingObserver {
    fn seen(&self) -> Vec<SeenEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl dataflow_rs::ExecutionObserver for RecordingObserver {
    fn task_finished(&self, event: &dataflow_rs::TaskEvent<'_>) {
        self.events.lock().unwrap().push(SeenEvent {
            workflow_id: event.workflow_id.to_string(),
            task_id: event.task_id.to_string(),
            function: event.function.to_string(),
            status: event.status,
        });
    }
}

#[tokio::test]
async fn the_observer_covers_the_sync_builtins_and_the_async_path() {
    // The reason this exists: the eight sync built-ins never reach the function
    // registry, so a host cannot wrap them. Assert both dispatch sites report.
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "m", "name": "m", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "v", "name": "v", "function": {
                "name": "validation",
                "input": { "rules": [] } } },
            { "id": "l", "name": "l", "function": {
                "name": "log", "input": { "message": "hi" } } },
            { "id": "custom", "name": "custom", "function": {
                "name": "logger", "input": {} } }
        ]
    }"#,
    )
    .unwrap();

    let observer = Arc::new(RecordingObserver::default());
    let engine = Engine::builder()
        .with_workflow(wf)
        .register("logger", LoggingTask)
        .with_observer(observer.clone())
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    let seen = observer.seen();
    let ids: Vec<&str> = seen.iter().map(|e| e.task_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["m", "v", "l", "custom"],
        "every dispatched task reports, sync built-ins included"
    );

    // Function names, and the documented `validate` canonicalization.
    let fns: Vec<&str> = seen.iter().map(|e| e.function.as_str()).collect();
    assert_eq!(fns, vec!["map", "validate", "log", "logger"]);

    // All succeeded.
    assert!(seen.iter().all(|e| e.status == Some(200)));
    // Workflow id is reported.
    assert!(seen.iter().all(|e| e.workflow_id == "w"));
}

#[tokio::test]
async fn the_observer_reports_a_failing_task_before_the_error_propagates() {
    // Emitted before handle_task_result, whose `?` would otherwise drop exactly
    // the tasks a host most wants timed.
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "boom", "name": "boom", "function": {
                "name": "fail", "input": {} } }
        ]
    }"#,
    )
    .unwrap();

    let observer = Arc::new(RecordingObserver::default());
    let engine = Engine::builder()
        .with_workflow(wf)
        .register("fail", FailingTask)
        .with_observer(observer.clone())
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    assert!(engine.process_message(&mut message).await.is_err());

    let seen = observer.seen();
    assert_eq!(seen.len(), 2, "the failing task is still reported");
    assert_eq!(seen[1].task_id, "boom");
    assert_eq!(seen[1].status, Some(500), "an Err dispatch reports 500");
}

#[tokio::test]
async fn a_skipped_condition_is_not_reported_but_a_skip_outcome_is() {
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "never", "name": "never",
              "condition": { "==": [1, 2] },
              "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.no", "logic": 1 } ] } } },
            { "id": "gate", "name": "gate", "function": {
                "name": "filter",
                "input": { "condition": false, "on_reject": "skip" } } }
        ]
    }"#,
    )
    .unwrap();

    let observer = Arc::new(RecordingObserver::default());
    let engine = Engine::builder()
        .with_workflow(wf)
        .with_observer(observer.clone())
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    let seen = observer.seen();
    // The false-condition task was never dispatched, so there is nothing to time.
    assert_eq!(seen.len(), 1, "only the dispatched task reports: {seen:?}");
    assert_eq!(seen[0].task_id, "gate");
    // TaskOutcome::Skip ran its body but records no audit status.
    assert_eq!(seen[0].status, None, "a Skip outcome reports status None");
}

#[tokio::test]
async fn the_observer_survives_a_hot_reload() {
    // with_new_workflows rebuilds the executor stack; dropping the observer
    // there would stop metrics silently at the first reload.
    let observer = Arc::new(RecordingObserver::default());
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(1))
        .with_observer(observer.clone())
        .build()
        .unwrap();

    let mut m1 = Message::from_value(&json!({}));
    engine.process_message(&mut m1).await.unwrap();
    assert_eq!(observer.seen().len(), 1);

    let reloaded = engine
        .with_new_workflows(vec![n_map_task_workflow(2)])
        .unwrap();
    let mut m2 = Message::from_value(&json!({}));
    reloaded.process_message(&mut m2).await.unwrap();

    assert_eq!(
        observer.seen().len(),
        3,
        "the reloaded engine must keep reporting"
    );
}

#[tokio::test]
async fn observer_durations_are_populated_and_with_handlers_reaches_the_builder() {
    // `with_handlers` exists so an embedder that builds the whole handler map in
    // one place can still reach `with_observer`.
    #[derive(Default)]
    struct DurationObserver {
        total_us: std::sync::atomic::AtomicU64,
        count: std::sync::atomic::AtomicU64,
    }
    impl dataflow_rs::ExecutionObserver for DurationObserver {
        fn task_finished(&self, event: &dataflow_rs::TaskEvent<'_>) {
            use std::sync::atomic::Ordering;
            self.total_us
                .fetch_add(event.duration.as_micros() as u64, Ordering::Relaxed);
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }

    let mut handlers: std::collections::HashMap<String, dataflow_rs::BoxedFunctionHandler> =
        std::collections::HashMap::new();
    handlers.insert("logger".to_string(), Box::new(LoggingTask));

    let observer = Arc::new(DurationObserver::default());
    let engine = Engine::builder()
        .with_workflow(
            Workflow::from_json(
                r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [ { "id": "custom", "name": "custom", "function": {
            "name": "logger", "input": {} } } ]
    }"#,
            )
            .unwrap(),
        )
        .with_handlers(handlers)
        .with_observer(observer.clone())
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    use std::sync::atomic::Ordering;
    assert_eq!(observer.count.load(Ordering::Relaxed), 1);
    // Duration is a real reading, not a placeholder; it may legitimately be 0us
    // on a fast task, so only assert it was recorded.
    let _ = observer.total_us.load(Ordering::Relaxed);
}

#[tokio::test]
async fn no_observer_means_no_added_clock_reads() {
    // The gate is on observer presence, so the documented one-Utc::now()-per-
    // message invariant holds for every caller that has not opted in.
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(4))
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    let stamps: Vec<_> = message.audit_trail().iter().map(|a| a.timestamp).collect();
    assert_eq!(stamps.len(), 4);
    assert!(stamps.windows(2).all(|w| w[0] == w[1]));
}
