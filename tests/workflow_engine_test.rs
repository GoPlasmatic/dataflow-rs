use async_trait::async_trait;
use dataflow_rs::engine::functions::{AsyncFunctionHandler, FunctionConfig};
use dataflow_rs::engine::message::{Change, Message};
use dataflow_rs::{Engine, Result, Task, Workflow};
use datalogic_rs::DataLogic;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

// A simple async task implementation
#[derive(Debug)]
struct LoggingTask;

#[async_trait]
impl AsyncFunctionHandler for LoggingTask {
    async fn execute(
        &self,
        message: &mut Message,
        _config: &FunctionConfig,
        _datalogic: Arc<DataLogic>,
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

#[tokio::test]
async fn test_async_task_execution() {
    // This test only tests the task implementation
    let task = LoggingTask;

    // Create a dummy message
    let mut message = Message::from_value(&json!({}));

    // Execute the task directly
    let config = FunctionConfig::Custom {
        name: "log".to_string(),
        input: json!({}),
    };
    let datalogic = Arc::new(DataLogic::with_preserve_structure());
    let result = task.execute(&mut message, &config, datalogic).await;

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

    // Add async logging handler
    custom_functions.insert("log".to_string(), Box::new(LoggingTask));

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

    let engine = Engine::new(workflows, None);
    let mut message = Message::from_value(&json!({"test": "data"}));

    // Initially temp_data should be empty
    assert_eq!(message.temp_data, json!({}));

    // Process the message
    engine.process_message(&mut message).await.unwrap();

    // After fix: temp_data is MERGED, not replaced
    // Both field1 and field2 should exist
    assert_eq!(
        message.temp_data,
        json!({
            "field1": "first_value",
            "field2": "second_value"
        })
    );

    // Verify that both fields are present (demonstrating the merge behavior)
    assert!(
        message.temp_data.get("field1").is_some(),
        "field1 should be present after merge"
    );
    assert!(
        message.temp_data.get("field2").is_some(),
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

    let engine = Engine::new(workflows, None);
    let mut message = Message::from_value(&json!({"test": "data"}));

    engine.process_message(&mut message).await.unwrap();

    // With nested paths, both fields should be preserved
    assert_eq!(
        message.temp_data,
        json!({
            "field1": "first_value",
            "field2": "second_value"
        })
    );

    // Both fields should exist when using nested paths
    assert!(
        message.temp_data.get("field1").is_some(),
        "field1 should exist with nested path approach"
    );
    assert!(
        message.temp_data.get("field2").is_some(),
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

    let engine = Engine::new(workflows, None);
    let mut message = Message::from_value(&json!({}));
    // Initialize the data field with existing data to test merging
    message.data = json!({"initial": "data"});

    engine.process_message(&mut message).await.unwrap();

    // After fix: When using path "data", it merges with existing data
    // Note: Order may vary in the JSON object
    assert_eq!(message.data["initial"], json!("data"));
    assert_eq!(message.data["field1"], json!("value1"));
    assert_eq!(message.data["field2"], json!("value2"));

    // All fields should be present after merging
    assert!(
        message.data.get("initial").is_some(),
        "initial field should be preserved"
    );
    assert!(
        message.data.get("field1").is_some(),
        "field1 should be present"
    );
    assert!(
        message.data.get("field2").is_some(),
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

    let engine = Engine::new(workflows, None);
    let mut message = Message::from_value(&json!({}));

    engine.process_message(&mut message).await.unwrap();

    // Verify fields with numeric names were created correctly
    assert_eq!(message.data["fields"]["20"], json!("value for field 20"));
    assert_eq!(message.data["fields"]["100"], json!("value for field 100"));
    assert_eq!(message.data["fields"]["#"], json!("value for hash field"));
    assert_eq!(message.data["fields"]["##"], json!("value for double hash"));

    // Verify the complete structure
    assert_eq!(
        message.data["fields"],
        json!({
            "20": "value for field 20",
            "100": "value for field 100",
            "#": "value for hash field",
            "##": "value for double hash"
        })
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

    let engine = Engine::new(workflows, None);
    let mut message = Message::from_value(&json!({}));

    engine.process_message(&mut message).await.unwrap();

    // Verify field "72" is an array with modified values
    assert_eq!(
        message.data["fields"]["72"],
        json!(["modified_first", "initial2", "modified_third"])
    );

    // Verify field "100" is an array with modified second element
    assert_eq!(
        message.data["fields"]["100"],
        json!(["alpha", "modified_beta"])
    );

    // Verify we can access these via get_nested_value with # prefix
    use dataflow_rs::engine::utils::get_nested_value;
    assert_eq!(
        get_nested_value(&message.data, "fields.#72.0"),
        Some(&json!("modified_first"))
    );
    assert_eq!(
        get_nested_value(&message.data, "fields.#72.2"),
        Some(&json!("modified_third"))
    );
    assert_eq!(
        get_nested_value(&message.data, "fields.#100.1"),
        Some(&json!("modified_beta"))
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

    let engine = Engine::new(workflows, None);
    let mut message = Message::from_value(&json!({}));

    engine.process_message(&mut message).await.unwrap();

    // Verify first mapping worked
    assert_eq!(message.data["step1"], json!("initial_value"));

    // CRITICAL TEST: Verify second mapping could see the first mapping's result
    // This now works after fixing the evaluation context issue
    assert_eq!(
        message.data.get("step2"),
        Some(&json!("initial_value")),
        "Second mapping should see first mapping's result"
    );

    // Verify third mapping could see both previous mappings (they should be equal)
    assert_eq!(
        message.data.get("step3"),
        Some(&json!(true)), // step1 == step2 should be true
        "Third mapping should see results from both previous mappings"
    );

    // Verify temp_data was set
    assert_eq!(message.temp_data["temp1"], json!("temp_value"));

    // Verify mapping could reference temp_data
    assert_eq!(
        message.data.get("from_temp"),
        Some(&json!("temp_value")),
        "Mapping should be able to reference temp_data"
    );

    // Verify array was created
    assert_eq!(message.data["array_test"], json!(["a", "b", "c"]));

    // Verify array element could be referenced
    assert_eq!(
        message.data.get("array_element"),
        Some(&json!("b")),
        "Should be able to reference array element from previous mapping"
    );

    println!(
        "Final data: {}",
        serde_json::to_string_pretty(&message.data).unwrap()
    );
    println!(
        "Final temp_data: {}",
        serde_json::to_string_pretty(&message.temp_data).unwrap()
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

    let engine = Engine::new(workflows, None);
    let mut message = Message::from_value(&json!({}));

    engine.process_message(&mut message).await.unwrap();

    // First mapping should work
    assert_eq!(message.data["value1"], json!(10));

    // Second mapping should now see value1 and compute 10 * 2 = 20
    println!("value2 result: {:?}", message.data.get("value2"));

    // This now works correctly after the fix
    assert_eq!(
        message.data.get("value2"),
        Some(&json!(20)),
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

    let engine = Engine::new(workflows, None);
    let mut message = Message::from_value(&json!({}));

    engine.process_message(&mut message).await.unwrap();

    // After merge, all fields should be present
    assert_eq!(message.temp_data["Receiver"], json!("NQZATAE1"));
    assert_eq!(message.temp_data["Sender"], json!("ZSZUBOM1"));
    assert_eq!(
        message.temp_data["UETR"],
        json!("8e49e852-45a1-42f7-b120-18d232541285")
    );
    assert_eq!(message.temp_data["settlement_method"], json!("INDA"));
    assert_eq!(message.temp_data["settlement_account"], json!(null));

    // Verify the complete structure has all fields
    assert!(
        message.temp_data.get("Receiver").is_some(),
        "Receiver should be preserved"
    );
    assert!(
        message.temp_data.get("Sender").is_some(),
        "Sender should be preserved"
    );
    assert!(
        message.temp_data.get("UETR").is_some(),
        "UETR should be preserved"
    );
    assert!(
        message.temp_data.get("settlement_method").is_some(),
        "settlement_method should be added"
    );
    assert!(
        message.temp_data.get("settlement_account").is_some(),
        "settlement_account should be added"
    );
}
