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
//!             name: "parse_json",
//!             input: { source: "payload", target: "input" }
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

/// Build the message every entry point processes: the payload is stored as a
/// raw string, so a `parse_json` / `parse_xml` task in the workflow is what
/// turns it into structured `data`. The engine never parses it.
fn raw_string_message(payload: &str) -> Message {
    Message::from_value(&Value::String(payload.to_string()))
}

/// Render an engine result as a JSON string `JsValue` — what every entry
/// point resolves its Promise with.
fn to_js_json<T: serde::Serialize>(value: &T) -> Result<JsValue, JsValue> {
    serde_json::to_string(value)
        .map(|s| JsValue::from_str(&s))
        .map_err(js_err)
}

/// Reject a Promise with `e`'s `Display` form.
fn js_err(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
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
    pub fn new(workflows_json: &str) -> Result<Self, String> {
        Self::build(workflows_json, None)
    }

    /// As the constructor, with a secret store the workflows can read through
    /// `{"secret": "name"}`.
    ///
    /// A second constructor rather than an optional argument, so `new`'s
    /// signature — and every existing caller — is untouched. The values are
    /// held by the engine, never by a message, so they appear in no processed
    /// message and no trace this engine returns.
    ///
    /// # Arguments
    /// * `workflows_json` - JSON array of workflow definitions
    /// * `secrets_json` - JSON object of name → value
    ///
    /// # Example
    /// ```javascript
    /// const engine = WasmEngine.with_secrets(workflows, JSON.stringify({ token: "…" }));
    /// ```
    #[wasm_bindgen]
    pub fn with_secrets(workflows_json: &str, secrets_json: &str) -> Result<Self, String> {
        Self::build(workflows_json, Some(secrets_json))
    }

    fn build(workflows_json: &str, secrets_json: Option<&str>) -> Result<Self, String> {
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

        let mut builder = Engine::builder().with_workflows(workflows);
        if let Some(secrets_json) = secrets_json {
            let secrets: Value = serde_json::from_str(secrets_json)
                .map_err(|e| format!("Invalid secrets JSON: {}", e))?;
            builder = builder.with_secrets_json(&secrets);
        }
        let engine = builder
            .build()
            .map_err(|e| format!("Engine construction failed: {}", e))?;
        Ok(Self {
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
        let mut message = raw_string_message(payload);
        // Clone the Arc for the async block
        let engine = Arc::clone(&self.inner);

        future_to_promise(async move {
            engine.process_message(&mut message).await.map_err(js_err)?;
            to_js_json(&message)
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
        let mut message = raw_string_message(payload);
        // Clone the Arc for the async block
        let engine = Arc::clone(&self.inner);

        future_to_promise(async move {
            let trace = engine
                .process_message_with_trace(&mut message)
                .await
                .map_err(js_err)?;
            to_js_json(&trace)
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
        let ids: Vec<&str> = self
            .inner
            .workflows()
            .iter()
            .map(|w| w.id.as_str())
            .collect();
        serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_string())
    }
}

/// The version of the engine compiled into this module.
///
/// Matches the published npm package version — the release workflow stamps
/// both from the root `Cargo.toml`.
///
/// Callers pair a wasm build with a UI build, and a mismatch is otherwise
/// invisible: [`Workflow`] does not set `deny_unknown_fields`, so an older
/// engine *ignores* a field it predates rather than rejecting it. The workflow
/// then runs and quietly does something other than what it says. Compare this
/// against the version your frontend was built for and fail loudly instead.
#[wasm_bindgen]
pub fn engine_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
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
