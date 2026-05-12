use async_trait::async_trait;
use dataflow_rs::engine::functions::{AsyncFunctionHandler, FunctionConfig};
use dataflow_rs::engine::message::Message;
use dataflow_rs::engine::utils::set_nested_value;
use dataflow_rs::{Engine, Result, Task, TaskContext, TaskOutcome, Workflow};
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
