# API Reference

Quick reference for the main dataflow-rs types and methods.

## Engine

The central component that processes messages through workflows.

```rust
use dataflow_rs::Engine;
```

### Constructor

```rust
pub fn new(
    workflows: Vec<Workflow>,
    custom_functions: Option<HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>>>
) -> Engine
```

Creates a new engine with the given workflows. All JSONLogic is compiled at creation.

### Methods

```rust
// Process a message through all matching workflows
pub async fn process_message(&self, message: &mut Message) -> Result<()>

// Get registered workflows
pub fn workflows(&self) -> &HashMap<String, Workflow>
```

## Workflow

A collection of tasks with optional conditions and priority.

```rust
use dataflow_rs::Workflow;
```

### Constructors

```rust
// Parse from JSON string
pub fn from_json(json: &str) -> Result<Workflow>

// Load from file
pub fn from_file(path: &str) -> Result<Workflow>
```

### JSON Schema

```json
{
    "id": "string (required)",
    "name": "string (optional)",
    "priority": "number (optional, default: 0)",
    "condition": "JSONLogic (optional)",
    "continue_on_error": "boolean (optional, default: false)",
    "tasks": "array of Task (required)"
}
```

## Task

An individual processing unit within a workflow.

### JSON Schema

```json
{
    "id": "string (required)",
    "name": "string (optional)",
    "condition": "JSONLogic (optional)",
    "continue_on_error": "boolean (optional)",
    "function": {
        "name": "string (required)",
        "input": "object (required)"
    }
}
```

## Message

The data container that flows through workflows.

```rust
use dataflow_rs::Message;
```

### Constructors

```rust
// Create from data (goes to context.data)
pub fn new(data: &Value) -> Message

// Create from full context value
pub fn from_value(context: &Value) -> Message
```

### Fields

```rust
pub struct Message {
    pub id: Uuid,
    pub payload: Arc<Value>,
    pub context: Value,  // Contains data, metadata, temp_data
    pub audit_trail: Vec<AuditTrail>,
    pub errors: Vec<ErrorInfo>,
}
```

### Methods

```rust
// Get context as Arc for efficient sharing
pub fn get_context_arc(&mut self) -> Arc<Value>

// Invalidate context cache after modifications
pub fn invalidate_context_cache(&mut self)
```

## AsyncFunctionHandler

Trait for implementing custom functions.

```rust
use dataflow_rs::engine::AsyncFunctionHandler;
```

### Trait Definition

```rust
#[async_trait]
pub trait AsyncFunctionHandler: Send + Sync {
    async fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
        datalogic: Arc<DataLogic>,
    ) -> Result<(usize, Vec<Change>)>;
}
```

### Return Value

- `usize` - Status code (200 = success, 400 = validation error, 500 = execution error)
- `Vec<Change>` - List of changes for audit trail

## FunctionConfig

Configuration for function execution.

```rust
pub struct FunctionConfig {
    pub name: String,
    pub input: Value,
}
```

## Change

Represents a single data modification.

```rust
pub struct Change {
    pub path: Arc<str>,
    pub old_value: Arc<Value>,
    pub new_value: Arc<Value>,
}
```

## AuditTrail

Records changes made by a task.

```rust
pub struct AuditTrail {
    pub task_id: String,
    pub workflow_id: String,
    pub timestamp: DateTime<Utc>,
    pub changes: Vec<Change>,
}
```

## ErrorInfo

Error information recorded in the message.

```rust
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
    pub workflow_id: Option<String>,
    pub task_id: Option<String>,
}
```

## DataflowError

Main error type for the library.

```rust
use dataflow_rs::engine::error::DataflowError;
```

### Variants

```rust
pub enum DataflowError {
    Validation(String),
    Execution(String),
    Logic(String),
    Io(String),
    // ... other variants
}
```

## Built-in Functions

### map

Data transformation using JSONLogic.

```json
{
    "name": "map",
    "input": {
        "mappings": [
            {
                "path": "string",
                "logic": "JSONLogic expression"
            }
        ]
    }
}
```

### validation

Rule-based data validation.

```json
{
    "name": "validation",
    "input": {
        "rules": [
            {
                "logic": "JSONLogic expression",
                "message": "string"
            }
        ]
    }
}
```

## WASM API (dataflow-wasm)

For browser/JavaScript usage.

```javascript
import init, { WasmEngine, process_message } from 'dataflow-wasm';

// Initialize
await init();

// Create engine
const engine = new WasmEngine(workflowsJson);

// Process with payload string (returns Promise)
const result = await engine.process(payloadStr);

// One-off convenience function (no engine needed)
const result2 = await process_message(workflowsJson, payloadStr);

// Get workflow info
const count = engine.workflow_count();
const ids = engine.workflow_ids();
```

## Full API Documentation

For complete API documentation, run:

```bash
cargo doc --open
```

This generates detailed documentation from the source code comments.
