//! WebAssembly bindings for dataflow-rs workflow engine.
//!
//! This crate provides WASM bindings that allow using dataflow-rs from JavaScript/TypeScript.
//!
//! # Usage
//!
//! ```javascript
//! import init, { WasmEngine, create_message } from 'dataflow-wasm';
//!
//! await init();
//!
//! // Define workflows
//! const workflows = JSON.stringify([{
//!     id: "example",
//!     name: "Example Workflow",
//!     priority: 1,
//!     tasks: [{
//!         id: "task1",
//!         name: "Transform Data",
//!         function: {
//!             name: "map",
//!             input: {
//!                 mappings: [{
//!                     path: "data.result",
//!                     logic: { "var": "data.input" }
//!                 }]
//!             }
//!         }
//!     }]
//! }]);
//!
//! // Create engine
//! const engine = new WasmEngine(workflows);
//!
//! // Create and process a message
//! const message = create_message('{"input": "hello"}', '{"type": "test"}');
//! const result = await engine.process(message);
//! console.log(JSON.parse(result));
//! ```

use dataflow_rs::{Engine, Message, Workflow};
use serde_json::{Value, json};
use std::sync::Arc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

/// Initialize the WASM module.
///
/// This is automatically called when the module loads.
/// Sets up the panic hook for better error messages in the browser console.
#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// Create a message JSON string from data and metadata.
///
/// # Arguments
/// * `data` - JSON string containing the message data (goes to context.data)
/// * `metadata` - JSON string containing the message metadata (goes to context.metadata)
///
/// # Returns
/// JSON string representing the complete message, or an error message
///
/// # Example
/// ```javascript
/// const message = create_message('{"name": "John"}', '{"type": "user"}');
/// const result = await engine.process(message);
/// ```
#[wasm_bindgen]
pub fn create_message(data: &str, metadata: &str) -> Result<String, String> {
    let data_value: Value =
        serde_json::from_str(data).map_err(|e| format!("Invalid data JSON: {}", e))?;
    let metadata_value: Value =
        serde_json::from_str(metadata).map_err(|e| format!("Invalid metadata JSON: {}", e))?;

    // Create a message with empty payload, then set context data and metadata
    let mut message = Message::from_value(&json!({}));
    message.context["data"] = data_value;
    message.context["metadata"] = metadata_value;

    serde_json::to_string(&message).map_err(|e| e.to_string())
}

/// A WebAssembly-compatible workflow engine.
///
/// Wraps the dataflow-rs Engine to provide async message processing
/// that returns JavaScript Promises.
#[wasm_bindgen]
pub struct WasmEngine {
    inner: Arc<Engine>,
}

#[wasm_bindgen]
impl WasmEngine {
    /// Create a new WasmEngine from a JSON array of workflow definitions.
    ///
    /// # Arguments
    /// * `workflows_json` - JSON string containing an array of workflow definitions
    ///
    /// # Example
    /// ```javascript
    /// const workflows = JSON.stringify([{
    ///     id: "workflow1",
    ///     name: "My Workflow",
    ///     priority: 1,
    ///     tasks: [...]
    /// }]);
    /// const engine = new WasmEngine(workflows);
    /// ```
    #[wasm_bindgen(constructor)]
    pub fn new(workflows_json: &str) -> Result<WasmEngine, String> {
        let workflows_value: Value = serde_json::from_str(workflows_json)
            .map_err(|e| format!("Invalid workflows JSON: {}", e))?;

        let workflows_array = workflows_value
            .as_array()
            .ok_or_else(|| "Workflows must be a JSON array".to_string())?;

        let mut workflows = Vec::with_capacity(workflows_array.len());
        for (i, workflow_value) in workflows_array.iter().enumerate() {
            let workflow_str = serde_json::to_string(workflow_value).map_err(|e| e.to_string())?;
            let workflow = Workflow::from_json(&workflow_str)
                .map_err(|e| format!("Invalid workflow at index {}: {}", i, e))?;
            workflows.push(workflow);
        }

        let engine = Engine::new(workflows, None);
        Ok(WasmEngine {
            inner: Arc::new(engine),
        })
    }

    /// Process a message through the engine's workflows.
    ///
    /// This is an async operation that returns a Promise.
    ///
    /// # Arguments
    /// * `message_json` - JSON string of the message to process
    ///
    /// # Returns
    /// A Promise that resolves to the processed message as a JSON string
    ///
    /// # Example
    /// ```javascript
    /// const result = await engine.process(messageJson);
    /// const processed = JSON.parse(result);
    /// console.log(processed.context.data);
    /// ```
    #[wasm_bindgen]
    pub fn process(&self, message_json: &str) -> js_sys::Promise {
        let message_result: Result<Message, _> = serde_json::from_str(message_json);

        match message_result {
            Ok(mut message) => {
                // Clone the Arc for the async block
                let engine = Arc::clone(&self.inner);

                future_to_promise(async move {
                    match engine.process_message(&mut message).await {
                        Ok(()) => serde_json::to_string(&message)
                            .map(|s| JsValue::from_str(&s))
                            .map_err(|e| JsValue::from_str(&e.to_string())),
                        Err(e) => Err(JsValue::from_str(&e.to_string())),
                    }
                })
            }
            Err(e) => {
                let error_msg = format!("Invalid message JSON: {}", e);
                future_to_promise(async move { Err(JsValue::from_str(&error_msg)) })
            }
        }
    }

    /// Get the number of workflows registered in the engine.
    #[wasm_bindgen]
    pub fn workflow_count(&self) -> usize {
        self.inner.workflows().len()
    }

    /// Get the list of workflow IDs.
    ///
    /// # Returns
    /// JSON array of workflow IDs as a string
    #[wasm_bindgen]
    pub fn workflow_ids(&self) -> String {
        let ids: Vec<&String> = self.inner.workflows().keys().collect();
        serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_string())
    }
}

/// Process a message through a one-off engine (convenience function).
///
/// Creates an engine with the given workflows and processes a single message.
/// Use WasmEngine class for better performance when processing multiple messages.
///
/// # Arguments
/// * `workflows_json` - JSON string containing an array of workflow definitions
/// * `message_json` - JSON string of the message to process
///
/// # Returns
/// A Promise that resolves to the processed message as a JSON string
///
/// # Example
/// ```javascript
/// const result = await process_message(workflowsJson, messageJson);
/// console.log(JSON.parse(result));
/// ```
#[wasm_bindgen]
pub fn process_message(workflows_json: &str, message_json: &str) -> js_sys::Promise {
    let engine_result = WasmEngine::new(workflows_json);
    match engine_result {
        Ok(engine) => engine.process(message_json),
        Err(e) => future_to_promise(async move { Err(JsValue::from_str(&e)) }),
    }
}
