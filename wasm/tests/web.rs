#![cfg(target_arch = "wasm32")]

use dataflow_wasm::*;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

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
async fn test_process_payload_as_string() {
    // Workflow that uses parse plugin first, then maps data
    let workflows = r#"[{
        "id": "mapper",
        "name": "Mapper Workflow",
        "priority": 1,
        "tasks": [{
            "id": "parse_payload",
            "name": "Parse Payload",
            "function": {
                "name": "parse",
                "input": {}
            }
        }, {
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

    // Payload is passed as a raw string (not pre-parsed)
    let payload = r#"{"input_name": "Alice"}"#;

    let promise = engine.process(payload);
    let result = wasm_bindgen_futures::JsFuture::from(promise).await;

    assert!(result.is_ok());
    let result_str = result.unwrap().as_string().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result_str).unwrap();

    assert_eq!(parsed["context"]["data"]["output_name"], "Alice");
}

#[wasm_bindgen_test]
async fn test_process_raw_payload_stored_as_string() {
    // Workflow without parse plugin - payload should remain as string
    let workflows = r#"[{
        "id": "no_parse",
        "name": "No Parse Workflow",
        "priority": 1,
        "tasks": [{
            "id": "noop",
            "name": "No-op Task",
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

    let engine = WasmEngine::new(workflows).unwrap();

    // Payload is passed as a raw string
    let payload = r#"{"some": "data"}"#;

    let promise = engine.process(payload);
    let result = wasm_bindgen_futures::JsFuture::from(promise).await;

    assert!(result.is_ok());
    let result_str = result.unwrap().as_string().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result_str).unwrap();

    // Payload should be stored as a string value (not parsed JSON)
    assert_eq!(parsed["payload"], r#"{"some": "data"}"#);
    assert_eq!(parsed["context"]["data"]["processed"], true);
}

#[wasm_bindgen_test]
async fn test_process_message_standalone() {
    let workflows = r#"[{
        "id": "standalone",
        "name": "Standalone",
        "priority": 1,
        "tasks": [{
            "id": "parse",
            "name": "Parse",
            "function": {
                "name": "parse",
                "input": {}
            }
        }, {
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

    let payload = r#"{}"#;
    let promise = process_message(workflows, payload);
    let result = wasm_bindgen_futures::JsFuture::from(promise).await;

    assert!(result.is_ok());
    let result_str = result.unwrap().as_string().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result_str).unwrap();

    assert_eq!(parsed["context"]["data"]["processed"], true);
}

#[wasm_bindgen_test]
async fn test_workflow_with_condition() {
    // Workflow with condition - uses parse plugin first to populate metadata
    let workflows = r#"[{
        "id": "parse_first",
        "name": "Parse First",
        "priority": 0,
        "tasks": [{
            "id": "parse",
            "name": "Parse",
            "function": {
                "name": "parse",
                "input": {}
            }
        }, {
            "id": "copy_metadata",
            "name": "Copy to Metadata",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [{
                        "path": "metadata.should_run",
                        "logic": {"var": "data.should_run"}
                    }]
                }
            }
        }]
    }, {
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
    let payload_run = r#"{"should_run": true}"#;
    let result = wasm_bindgen_futures::JsFuture::from(engine.process(payload_run))
        .await
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result.as_string().unwrap()).unwrap();
    assert_eq!(parsed["context"]["data"]["ran"], true);

    // Test with condition not met
    let payload_skip = r#"{"should_run": false}"#;
    let result = wasm_bindgen_futures::JsFuture::from(engine.process(payload_skip))
        .await
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result.as_string().unwrap()).unwrap();
    assert!(parsed["context"]["data"]["ran"].is_null());
}
