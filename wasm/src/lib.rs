//! WebAssembly bindings for dataflow-rs workflow engine.
//!
//! This crate provides WASM bindings that allow using dataflow-rs from JavaScript/TypeScript.
//!
//! # Usage
//!
//! ```javascript
//! import init, { WasmEngine } from 'dataflow-wasm';
//!
//! await init();
//!
//! // Define workflows
//! const workflows = JSON.stringify([{
//!     id: "example",
//!     name: "Example Workflow",
//!     priority: 1,
//!     tasks: [{
//!         id: "parse_payload",
//!         name: "Parse Payload",
//!         function: {
//!             name: "parse",
//!             input: {}
//!         }
//!     }, {
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
//! // Process a payload (raw string, parsed by the parse plugin)
//! const payload = '{"input": "hello"}';
//! const result = await engine.process(payload);
//! console.log(JSON.parse(result));
//! ```

use dataflow_rs::{Engine, Message, Workflow};
use serde_json::Value;
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

    /// Process a payload through the engine's workflows.
    ///
    /// This is an async operation that returns a Promise.
    /// The payload is stored as a raw string and should be parsed by a parse plugin
    /// in the workflow if JSON parsing is needed.
    ///
    /// # Arguments
    /// * `payload` - Raw string payload to process (not parsed by the engine)
    ///
    /// # Returns
    /// A Promise that resolves to the processed message as a JSON string
    ///
    /// # Example
    /// ```javascript
    /// const payload = '{"name": "John", "email": "john@example.com"}';
    /// const result = await engine.process(payload);
    /// const processed = JSON.parse(result);
    /// console.log(processed.context.data);
    /// ```
    #[wasm_bindgen]
    pub fn process(&self, payload: &str) -> js_sys::Promise {
        // Store payload as a raw string - parsing is done by the parse plugin
        let mut message = Message::from_value(&Value::String(payload.to_string()));

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

    /// Process a payload with step-by-step execution tracing.
    ///
    /// This is an async operation that returns a Promise with the execution trace.
    /// The trace contains message snapshots after each step, including which
    /// workflows/tasks were executed or skipped.
    /// The payload is stored as a raw string and should be parsed by a parse plugin.
    ///
    /// # Arguments
    /// * `payload` - Raw string payload to process (not parsed by the engine)
    ///
    /// # Returns
    /// A Promise that resolves to the execution trace as a JSON string
    ///
    /// # Example
    /// ```javascript
    /// const payload = '{"name": "John", "email": "john@example.com"}';
    /// const trace = await engine.process_with_trace(payload);
    /// const traceData = JSON.parse(trace);
    /// console.log(traceData.steps); // Array of execution steps
    /// ```
    #[wasm_bindgen]
    pub fn process_with_trace(&self, payload: &str) -> js_sys::Promise {
        // Store payload as a raw string - parsing is done by the parse plugin
        let mut message = Message::from_value(&Value::String(payload.to_string()));

        // Clone the Arc for the async block
        let engine = Arc::clone(&self.inner);

        future_to_promise(async move {
            match engine.process_message_with_trace(&mut message).await {
                Ok(trace) => serde_json::to_string(&trace)
                    .map(|s| JsValue::from_str(&s))
                    .map_err(|e| JsValue::from_str(&e.to_string())),
                Err(e) => Err(JsValue::from_str(&e.to_string())),
            }
        })
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

/// Process a payload through a one-off engine (convenience function).
///
/// Creates an engine with the given workflows and processes a single payload.
/// Use WasmEngine class for better performance when processing multiple payloads.
/// The payload is stored as a raw string and should be parsed by a parse plugin.
///
/// # Arguments
/// * `workflows_json` - JSON string containing an array of workflow definitions
/// * `payload` - Raw string payload to process (not parsed by the engine)
///
/// # Returns
/// A Promise that resolves to the processed message as a JSON string
///
/// # Example
/// ```javascript
/// const payload = '{"name": "John", "email": "john@example.com"}';
/// const result = await process_message(workflowsJson, payload);
/// console.log(JSON.parse(result));
/// ```
#[wasm_bindgen]
pub fn process_message(workflows_json: &str, payload: &str) -> js_sys::Promise {
    let engine_result = WasmEngine::new(workflows_json);
    match engine_result {
        Ok(engine) => engine.process(payload),
        Err(e) => future_to_promise(async move { Err(JsValue::from_str(&e)) }),
    }
}
