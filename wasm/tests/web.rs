#![cfg(target_arch = "wasm32")]

use dataflow_wasm::*;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn test_create_message() {
    let data = r#"{"name": "John", "age": 30}"#;
    let metadata = r#"{"type": "user"}"#;
    let result = create_message(data, metadata).unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["context"]["data"]["name"], "John");
    assert_eq!(parsed["context"]["data"]["age"], 30);
    assert_eq!(parsed["context"]["metadata"]["type"], "user");
}

#[wasm_bindgen_test]
fn test_create_engine_simple() {
    let workflows = r#"[{
        "id": "test_workflow",
        "name": "Test Workflow",
        "priority": 1,
        "tasks": [{
            "id": "task1",
            "name": "Map Task",
            "function": {
                "name": "map",
                "input": {
                    "mappings": []
                }
            }
        }]
    }]"#;

    let engine = WasmEngine::new(workflows).unwrap();
    assert_eq!(engine.workflow_count(), 1);
}

#[wasm_bindgen_test]
fn test_workflow_ids() {
    let workflows = r#"[
        {"id": "workflow_a", "name": "Workflow A", "priority": 1, "tasks": [{"id": "t1", "name": "T1", "function": {"name": "map", "input": {"mappings": []}}}]},
        {"id": "workflow_b", "name": "Workflow B", "priority": 2, "tasks": [{"id": "t2", "name": "T2", "function": {"name": "map", "input": {"mappings": []}}}]}
    ]"#;

    let engine = WasmEngine::new(workflows).unwrap();
    let ids = engine.workflow_ids();
    let parsed: Vec<String> = serde_json::from_str(&ids).unwrap();

    assert_eq!(parsed.len(), 2);
    assert!(parsed.contains(&"workflow_a".to_string()));
    assert!(parsed.contains(&"workflow_b".to_string()));
}

#[wasm_bindgen_test]
fn test_invalid_workflows_json() {
    let invalid = r#"not valid json"#;
    let result = WasmEngine::new(invalid);
    assert!(result.is_err());
}

#[wasm_bindgen_test]
fn test_workflows_must_be_array() {
    let not_array = r#"{"id": "single"}"#;
    let result = WasmEngine::new(not_array);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must be a JSON array"));
}

#[wasm_bindgen_test]
fn test_create_message_invalid_data() {
    let result = create_message("not json", "{}");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Invalid data JSON"));
}

#[wasm_bindgen_test]
fn test_create_message_invalid_metadata() {
    let result = create_message("{}", "not json");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Invalid metadata JSON"));
}

#[wasm_bindgen_test]
async fn test_process_message_with_mapping() {
    let workflows = r#"[{
        "id": "mapper",
        "name": "Mapper Workflow",
        "priority": 1,
        "tasks": [{
            "id": "copy_name",
            "name": "Copy Name",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [{
                        "path": "data.output_name",
                        "logic": {"var": "data.input_name"}
                    }]
                }
            }
        }]
    }]"#;

    let engine = WasmEngine::new(workflows).unwrap();

    // Create a message with input data
    let message = create_message(r#"{"input_name": "Alice"}"#, r#"{}"#).unwrap();

    // Process it (this returns a Promise in JS, but in Rust tests we await it directly)
    let promise = engine.process(&message);
    let result = wasm_bindgen_futures::JsFuture::from(promise).await;

    assert!(result.is_ok());
    let result_str = result.unwrap().as_string().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result_str).unwrap();

    assert_eq!(parsed["context"]["data"]["output_name"], "Alice");
}

#[wasm_bindgen_test]
async fn test_process_invalid_message() {
    let workflows = r#"[{
        "id": "test",
        "name": "Test",
        "priority": 1,
        "tasks": [{"id": "t", "name": "T", "function": {"name": "map", "input": {"mappings": []}}}]
    }]"#;

    let engine = WasmEngine::new(workflows).unwrap();
    let promise = engine.process("not valid json");
    let result = wasm_bindgen_futures::JsFuture::from(promise).await;

    assert!(result.is_err());
}

#[wasm_bindgen_test]
async fn test_process_message_standalone() {
    let workflows = r#"[{
        "id": "standalone",
        "name": "Standalone",
        "priority": 1,
        "tasks": [{
            "id": "t",
            "name": "T",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [{
                        "path": "data.processed",
                        "logic": true
                    }]
                }
            }
        }]
    }]"#;

    let message = create_message("{}", "{}").unwrap();
    let promise = process_message(workflows, &message);
    let result = wasm_bindgen_futures::JsFuture::from(promise).await;

    assert!(result.is_ok());
    let result_str = result.unwrap().as_string().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result_str).unwrap();

    assert_eq!(parsed["context"]["data"]["processed"], true);
}

#[wasm_bindgen_test]
async fn test_workflow_with_condition() {
    let workflows = r#"[{
        "id": "conditional",
        "name": "Conditional Workflow",
        "priority": 1,
        "condition": {"==": [{"var": "metadata.should_run"}, true]},
        "tasks": [{
            "id": "mark",
            "name": "Mark",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [{
                        "path": "data.ran",
                        "logic": true
                    }]
                }
            }
        }]
    }]"#;

    let engine = WasmEngine::new(workflows).unwrap();

    // Test with condition met
    let message_run = create_message("{}", r#"{"should_run": true}"#).unwrap();
    let result = wasm_bindgen_futures::JsFuture::from(engine.process(&message_run))
        .await
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result.as_string().unwrap()).unwrap();
    assert_eq!(parsed["context"]["data"]["ran"], true);

    // Test with condition not met
    let message_skip = create_message("{}", r#"{"should_run": false}"#).unwrap();
    let result = wasm_bindgen_futures::JsFuture::from(engine.process(&message_skip))
        .await
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result.as_string().unwrap()).unwrap();
    assert!(parsed["context"]["data"]["ran"].is_null());
}
