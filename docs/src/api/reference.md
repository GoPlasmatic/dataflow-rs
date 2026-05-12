# API Reference

Quick reference for the main dataflow-rs types and methods.

## Type Aliases

Dataflow-rs provides rules-engine aliases alongside the original workflow terminology:

| Rules Engine | Workflow Engine | Import |
|---|---|---|
| `RulesEngine` | `Engine` | `use dataflow_rs::RulesEngine;` |
| `Rule` | `Workflow` | `use dataflow_rs::Rule;` |
| `Action` | `Task` | `use dataflow_rs::Action;` |

Both names refer to the same types — use whichever fits your mental model.

## Engine (RulesEngine)

The central component that evaluates rules and processes messages.

```rust
use dataflow_rs::Engine;  // or: use dataflow_rs::RulesEngine;
```

### Constructor

```rust
pub fn new(
    workflows: Vec<Workflow>,
    custom_functions: Option<HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>>>
) -> Engine
```

Creates a new engine with the given rules. All JSONLogic is compiled at creation.

### Methods

```rust
// Process a message through all matching rules
pub async fn process_message(&self, message: &mut Message) -> Result<()>

// Process with execution trace for debugging
pub async fn process_message_with_trace(&self, message: &mut Message) -> Result<ExecutionTrace>

// Process only workflows on a specific channel (O(1) lookup)
pub async fn process_message_for_channel(&self, channel: &str, message: &mut Message) -> Result<()>

// Channel routing with execution trace
pub async fn process_message_for_channel_with_trace(&self, channel: &str, message: &mut Message) -> Result<ExecutionTrace>

// Get registered rules (sorted by priority)
pub fn workflows(&self) -> &Arc<Vec<Workflow>>

// Find a workflow by ID
pub fn workflow_by_id(&self, id: &str) -> Option<&Workflow>

// Create a new engine with different workflows, preserving custom functions
pub fn with_new_workflows(&self, workflows: Vec<Workflow>) -> Self
```

## Workflow (Rule)

A collection of actions with optional conditions and priority.

```rust
use dataflow_rs::Workflow;  // or: use dataflow_rs::Rule;
```

### Constructors

```rust
// Parse from JSON string
pub fn from_json(json: &str) -> Result<Workflow>

// Load from file
pub fn from_file(path: &str) -> Result<Workflow>

// Convenience constructor for rules-engine pattern
pub fn rule(id: &str, name: &str, condition: Value, tasks: Vec<Task>) -> Self
```

### JSON Schema

```json
{
    "id": "string (required)",
    "name": "string (optional)",
    "priority": "number (optional, default: 0)",
    "condition": "JSONLogic (optional, evaluated against full context)",
    "continue_on_error": "boolean (optional, default: false)",
    "tasks": "array of Task (required)",
    "channel": "string (optional, default: 'default')",
    "version": "number (optional, default: 1)",
    "status": "'active' | 'paused' | 'archived' (optional, default: 'active')",
    "tags": "array of string (optional, default: [])",
    "created_at": "ISO 8601 datetime (optional)",
    "updated_at": "ISO 8601 datetime (optional)"
}
```

## Task (Action)

An individual processing unit within a rule.

```rust
use dataflow_rs::Task;  // or: use dataflow_rs::Action;
```

### Constructor

```rust
// Convenience constructor for rules-engine pattern
pub fn action(id: &str, name: &str, function: FunctionConfig) -> Self
```

### JSON Schema

```json
{
    "id": "string (required)",
    "name": "string (optional)",
    "condition": "JSONLogic (optional, evaluated against full context)",
    "continue_on_error": "boolean (optional)",
    "function": {
        "name": "string (required)",
        "input": "object (required)"
    }
}
```

## Message

The data container that flows through rules. The context tree is held as
`datavalue::OwnedDataValue` (not `serde_json::Value`) so the JSONLogic
evaluator can borrow it into its arena without a `serde_json` round-trip.

```rust
use dataflow_rs::Message;
use datavalue::OwnedDataValue;
use std::sync::Arc;
```

### Constructors

```rust
// Native zero-conversion entry point
pub fn new(payload: Arc<OwnedDataValue>) -> Message

// Convenience: bridge from a serde_json::Value payload
pub fn from_value(payload: &serde_json::Value) -> Message

// Construct with a caller-supplied id (skips the UUID v7 allocation)
pub fn with_id(id: impl Into<String>, payload: Arc<OwnedDataValue>) -> Message

// Disable per-write Change capture (audit trail still recorded, but
// `changes: []` — the bulk-pipeline fast path)
pub fn without_change_capture(self) -> Message
```

### Fields

```rust
pub struct Message {
    pub id: String,                       // UUID v7 string by default
    pub payload: Arc<OwnedDataValue>,
    pub context: OwnedDataValue,          // Always an Object with keys
                                          // "data", "metadata", "temp_data"
    pub audit_trail: Vec<AuditTrail>,
    pub errors: Vec<ErrorInfo>,
    pub capture_changes: bool,            // In-memory only; not serialized
}
```

### Methods

```rust
// Convenience accessors into context
pub fn data(&self) -> &OwnedDataValue
pub fn metadata(&self) -> &OwnedDataValue
pub fn temp_data(&self) -> &OwnedDataValue

// Add an error to the message
pub fn add_error(&mut self, error: ErrorInfo)

// Whether any errors were recorded
pub fn has_errors(&self) -> bool
```

To mutate `context`, use the path helpers from `dataflow_rs::engine::utils`:

```rust
use dataflow_rs::engine::utils::{get_nested_value, set_nested_value};

set_nested_value(
    &mut message.context,
    "data.user.name",
    OwnedDataValue::from(&serde_json::json!("Alice")),
);
```

## AsyncFunctionHandler

Trait for implementing custom action handlers.

```rust
use dataflow_rs::engine::AsyncFunctionHandler;
```

### Trait Definition

```rust
use datalogic_rs::Engine as DatalogicEngine;

#[async_trait]
pub trait AsyncFunctionHandler: Send + Sync {
    async fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
        engine: Arc<DatalogicEngine>,
    ) -> Result<(usize, Vec<Change>)>;
}
```

The supplied `Arc<datalogic_rs::Engine>` is the shared JSONLogic engine —
custom handlers that need to evaluate ad-hoc JSONLogic can call
`engine.compile_arc(&logic)` and `engine.evaluate(...)`. Handlers that
don't need it can simply ignore the argument.

### Return Value

- `usize` - Status code (200 = success, 400 = validation error, 500 = execution error)
- `Vec<Change>` - List of changes for audit trail

## FunctionConfig

`FunctionConfig` is an enum: every built-in is a typed variant, and unknown
function names deserialize into `Custom { name, input }`. Custom handlers
typically destructure the `Custom` variant to access their config.

```rust
pub enum FunctionConfig {
    Map { input: MapConfig, .. },
    Validation { input: ValidationConfig, .. },
    ParseJson { input: ParseConfig, .. },
    ParseXml { input: ParseConfig, .. },
    PublishJson { input: PublishConfig, .. },
    PublishXml { input: PublishConfig, .. },
    Filter { input: FilterConfig, .. },
    Log { input: LogConfig, .. },
    HttpCall { input: HttpCallConfig, .. },
    Enrich { input: EnrichConfig, .. },
    PublishKafka { input: PublishKafkaConfig, .. },
    Custom { name: String, input: serde_json::Value },
}
```

## Change

Represents a single data modification recorded in the audit trail.

```rust
pub struct Change {
    pub path: Arc<str>,
    pub old_value: OwnedDataValue,
    pub new_value: OwnedDataValue,
}
```

`old_value` and `new_value` are owned (not `Arc<OwnedDataValue>`) — one
less heap allocation per recorded mutation. Wrap them yourself if you need
to share a `Change` across threads.

## AuditTrail

Records changes made by an action. `workflow_id` / `task_id` are
`Arc<str>` mirrors of the workflow/task ids — the engine clones them by
refcount bump rather than allocating per audit entry.

```rust
pub struct AuditTrail {
    pub workflow_id: Arc<str>,
    pub task_id: Arc<str>,
    pub timestamp: DateTime<Utc>,
    pub changes: Vec<Change>,
    pub status: usize,
}
```

## ErrorInfo

Error information recorded in the message.

```rust
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
    pub path: Option<String>,
    pub workflow_id: Option<String>,
    pub task_id: Option<String>,
    pub timestamp: Option<String>,
    pub retry_attempted: Option<bool>,
    pub retry_count: Option<u32>,
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
    FunctionExecution { context: String, source: Option<Box<DataflowError>> },
    Workflow(String),
    Task(String),
    FunctionNotFound(String),
    Deserialization(String),
    Io(String),
    LogicEvaluation(String),
    Http { status: u16, message: String },
    Timeout(String),
    Unknown(String),
}
```

`DataflowError::retryable()` returns `true` for transient infrastructure
failures (5xx HTTP, 429, 408, timeouts, IO) and `false` for data/logic/
configuration errors.

## WorkflowStatus

Lifecycle status for workflows.

```rust
use dataflow_rs::WorkflowStatus;
```

### Variants

```rust
pub enum WorkflowStatus {
    Active,    // Default — workflow executes normally
    Paused,    // Excluded from channel routing
    Archived,  // Permanently retired
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

### filter

Pipeline control flow — halt workflow or skip task.

```json
{
    "name": "filter",
    "input": {
        "condition": "JSONLogic expression",
        "on_reject": "halt | skip (default: halt)"
    }
}
```

Status codes: 200 (pass), 298 (skip), 299 (halt).

### log

Structured logging with JSONLogic expressions.

```json
{
    "name": "log",
    "input": {
        "level": "trace | debug | info | warn | error (default: info)",
        "message": "JSONLogic expression",
        "fields": {
            "key": "JSONLogic expression"
        }
    }
}
```

Always returns (200, []) — never modifies the message.

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

// Get rule info
const count = engine.workflow_count();
const ids = engine.workflow_ids();
```

## Full API Documentation

For complete API documentation, run:

```bash
cargo doc --open
```

This generates detailed documentation from the source code comments.
