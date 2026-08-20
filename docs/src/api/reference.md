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

### Constructors

```rust,ignore
// Recommended path — fluent builder.
pub fn builder() -> EngineBuilder

// Lower-level entry. Use HashMap::new() for the no-handler case.
pub fn new(
    workflows: Vec<Workflow>,
    custom_functions: HashMap<String, BoxedFunctionHandler>,
) -> Result<Engine>
```

`EngineBuilder` (`#[must_use]`) chains
`.register("name", handler)`, `.register_boxed(name, boxed)`,
`.with_workflow(w)`, `.with_workflows(iter)`, `.with_handlers(map)`,
`.with_observer(obs)`, `.with_datalogic_operator(name, op)`,
`.with_error_context_path(path)`, `.with_error_context_limit(n)`, then
`.build() -> Result<Engine>`. All JSONLogic is compiled and Custom
inputs are pre-parsed into their typed `Self::Input` at `.build()` —
config-shape errors fail there, not on first message. An error-context
path that the JSONLogic evaluation context cannot see fails there too.

### Methods

```rust,ignore
// Process a message through all matching rules
pub async fn process_message(&self, message: &mut Message) -> Result<()>

// Process with execution trace for debugging (trace is lost if this returns Err)
pub async fn process_message_with_trace(&self, message: &mut Message) -> Result<ExecutionTrace>

// Process with tracing into a caller-owned trace, so steps survive a hard failure
pub async fn process_message_tracing(&self, message: &mut Message, trace: &mut ExecutionTrace) -> Result<()>

// Process with tracing under an explicit capture policy (snapshot budget,
// audit-trail scope, redaction, per-step diff)
pub async fn process_message_with_trace_options(&self, message: &mut Message, options: TraceOptions) -> Result<ExecutionTrace>

// Process only workflows on a specific channel (O(1) lookup)
pub async fn process_message_for_channel(&self, channel: &str, message: &mut Message) -> Result<()>

// Channel routing with execution trace
pub async fn process_message_for_channel_with_trace(&self, channel: &str, message: &mut Message) -> Result<ExecutionTrace>

// Channel routing, recording into a caller-owned trace
pub async fn process_message_for_channel_tracing(&self, channel: &str, message: &mut Message, trace: &mut ExecutionTrace) -> Result<()>

// Channel routing under an explicit capture policy
pub async fn process_message_for_channel_with_trace_options(&self, channel: &str, message: &mut Message, options: TraceOptions) -> Result<ExecutionTrace>

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

```rust,ignore
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
    "rollout": "{bucket_start, bucket_end} over 0..100 (optional, default: none)",
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

```rust,ignore
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
use dataflow_rs::datavalue::OwnedDataValue;
use std::sync::Arc;
```

### Constructors

```rust,ignore
// Fluent builder — recommended path for richer cases (custom id,
// capture_changes off, etc.).
pub fn builder() -> MessageBuilder

// Native zero-conversion entry point — perf path.
pub fn new(payload: Arc<OwnedDataValue>) -> Message

// Convenience: bridge from a serde_json::Value payload.
pub fn from_value(payload: &serde_json::Value) -> Message
```

`MessageBuilder` (`#[must_use]`) chains
`.id(...)`, `.payload(Arc<OwnedDataValue>)` /
`.payload_json(&serde_json::Value)`, `.capture_changes(bool)`,
`.data(..)` / `.data_json(..)`, `.metadata(..)` / `.metadata_json(..)`,
`.temp_data(..)` / `.temp_data_json(..)`, `.routing_bucket(u8)`,
then `.build() -> Message`.

The three context setters seed `context.data` / `metadata` / `temp_data`
directly, so a workflow condition reading `data.*` fires without needing a
`parse_json` task first. Keys are taken **literally** — unlike
`set_nested_value`, a key containing `.` stays one key and a leading `#` is not
stripped — and a non-`Object` value is ignored, preserving the invariant that the
three root fields are always objects. Seeding records no audit entry and no
`Change`; it is initial state, not a mutation.

### Structure

```rust,ignore
pub struct Message {
    pub context: OwnedDataValue,    // Always Object {data, metadata, temp_data}
    // ... encapsulated fields ...
}
```

`context` is the only `pub` field — it's the legitimate read surface
(tests do `message.context["data"]["x"]` lookups). Every other field
is read via accessors and mutated via `add_error` (errors) or
`TaskContext::set` (context) so audit-trail changes are recorded.

### Methods

```rust,ignore
// Identity + payload
pub fn id(&self) -> &str
pub fn payload(&self) -> &OwnedDataValue
pub fn payload_arc(&self) -> &Arc<OwnedDataValue>

// Context accessors
pub fn data(&self) -> &OwnedDataValue
pub fn metadata(&self) -> &OwnedDataValue
pub fn temp_data(&self) -> &OwnedDataValue

// Error + audit views
pub fn errors(&self) -> &[ErrorInfo]
pub fn audit_trail(&self) -> &[AuditTrail]
pub fn capture_changes(&self) -> bool
pub fn routing_bucket(&self) -> Option<u8>

// Mutation (constructive)
pub fn add_error(&mut self, error: ErrorInfo)

// Predicates
pub fn has_errors(&self) -> bool
```

Inside a custom `AsyncFunctionHandler`, mutate the context via
[`TaskContext::set`](#taskcontext) — it records audit-trail changes
automatically.

## AsyncFunctionHandler

Trait for implementing custom action handlers. See
[Custom Functions](../advanced/custom-functions.md) for the full
walk-through.

```rust
use dataflow_rs::prelude::*;
```

### Trait Definition

```rust,ignore
use serde::de::DeserializeOwned;

#[async_trait]
pub trait AsyncFunctionHandler: Send + Sync + 'static {
    /// Typed configuration shape for this handler. Use
    /// `serde_json::Value` for freeform JSON.
    type Input: DeserializeOwned + Send + Sync + 'static;

    /// Parse the raw `FunctionConfig::Custom { input }` JSON into
    /// `Self::Input`. Default impl uses `serde_json::from_value`;
    /// override only for custom validation beyond what serde provides.
    fn parse_input(input: &serde_json::Value) -> Result<Self::Input> { ... }

    /// Compile the `Template` fields of a just-parsed input. Called once per
    /// task at engine construction, right after `parse_input`. Default is a
    /// no-op, so a handler with no `Template` fields needs no override.
    fn compile_input(input: &mut Self::Input, c: &TemplateCompiler) -> Result<()> { ... }

    /// Execute the handler.
    async fn execute(
        &self,
        ctx: &mut TaskContext<'_>,
        input: &Self::Input,
    ) -> Result<TaskOutcome>;
}
```

The engine pre-parses each `FunctionConfig::Custom { input }` JSON into
the registered handler's typed `Self::Input` at `Engine::builder().build()`
(or `Engine::new`) — config-shape errors fail there, not on first message.

### Template

A config field whose authored JSON is JSONLogic, for custom handlers — the same
shape this crate's own `*_logic` fields (`path_logic`, `body_logic`, `key_logic`,
`value_logic`) use.

```rust,ignore
pub struct Template { /* opaque */ }

impl Template {
    // Called from `AsyncFunctionHandler::compile_input`.
    pub fn compile(&mut self, c: &TemplateCompiler, label: &str) -> Result<()>

    pub fn eval(&self, ctx: &TaskContext<'_>) -> Result<OwnedDataValue>
    pub fn eval_into<T: serde::de::DeserializeOwned>(&self, ctx: &TaskContext<'_>) -> Result<T>

    pub fn as_json(&self) -> &serde_json::Value
    pub fn is_compiled(&self) -> bool
}

// Handed to `compile_input`; wraps the same shared datalogic engine
// `LogicCompiler` uses internally, so a compiled `Template` evaluates against
// the same engine that will run the message.
pub struct TemplateCompiler { /* opaque */ }
impl TemplateCompiler {
    pub fn engine(&self) -> &datalogic_rs::Engine
}
```

Declare `Template` only on fields the workflow author is told are JSONLogic — the
engine compiles with templating enabled, so a single-key object whose key
matches an operator name evaluates as that operator rather than as a literal.
See [Config fields that are JSONLogic](../advanced/custom-functions.md#config-fields-that-are-jsonlogic-template).

### Boxing

```rust,ignore
pub type BoxedFunctionHandler = Box<dyn DynAsyncFunctionHandler + Send + Sync>;
```

Stored in the engine's registry. Users construct these via `Box::new(handler)`
(or via `Engine::builder().register("name", handler)`) — the dyn-trait
plumbing stays out of user code.

## TaskContext

Per-call context handed to every `AsyncFunctionHandler::execute` call.

```rust,ignore
pub struct TaskContext<'a> { /* ... */ }

impl<'a> TaskContext<'a> {
    // Read accessors
    pub fn message(&self) -> &Message
    pub fn message_mut(&mut self) -> &mut Message
    pub fn datalogic(&self) -> &Arc<datalogic_rs::Engine>
    pub fn data(&self) -> &OwnedDataValue
    pub fn metadata(&self) -> &OwnedDataValue
    pub fn temp_data(&self) -> &OwnedDataValue
    pub fn context(&self) -> &OwnedDataValue      // the whole {data, metadata, temp_data} tree
    pub fn get(&self, path: &str) -> Option<&OwnedDataValue>

    // Value-returning evaluation on the worker thread's pooled arena.
    // Unlike `executor::evaluate_condition`, these return the value rather than
    // collapsing to a bool, and surface failures as Err rather than false.
    pub fn eval(&self, logic: &Logic) -> Result<OwnedDataValue>
    pub fn eval_json(&self, logic: &Logic) -> Result<serde_json::Value>
    pub fn eval_to_plain_string(&self, logic: &Logic) -> Result<String>

    // Audit-trail-aware mutation
    pub fn set(&mut self, path: &str, value: OwnedDataValue)
    pub fn set_json(&mut self, path: &str, value: &serde_json::Value)
    pub fn add_error(&mut self, error: ErrorInfo)
}
```

`set` records a `Change` on the audit trail when `message.capture_changes`
is `true`, then writes through `set_nested_value` (auto-creates
intermediate objects/arrays, handles `#`-prefix escapes).

`eval_to_plain_string` **deliberately disagrees** with datalogic-rs's own string
projection: `Session::eval_str` keeps the JSON quoting, so a string result comes
back from it as `"\"abc\""`, whereas this returns `abc`. The name says
`plain_string` rather than `to_string` so the difference is visible at the call
site — these values end up in URL paths and message keys. A test pins both sides.

`eval_json` projects straight from the arena to `serde_json::Value` in one walk,
skipping the `OwnedDataValue` intermediate and the `from_value` rebuild.

## Path helpers (`engine::utils`)

Dot-path read, write and remove over the `OwnedDataValue` tree behind
`Message::context`. Numeric segments index arrays; one leading `#` escapes a
numerically-named object key (`data.#20` is the object key `"20"`).

```rust,ignore
pub fn get_nested_value<'b>(data: &'b OwnedDataValue, path: &str) -> Option<&'b OwnedDataValue>
pub fn get_nested_value_cloned(data: &OwnedDataValue, path: &str) -> Option<OwnedDataValue>
pub fn set_nested_value(data: &mut OwnedDataValue, path: &str, value: OwnedDataValue)

// Remove and return. `None` — leaving the tree untouched — for a missing key,
// an out-of-bounds or non-numeric index, descent through a non-container, or
// an empty path. Never panics.
pub fn remove_nested_value(data: &mut OwnedDataValue, path: &str) -> Option<OwnedDataValue>
```

`remove_nested_value` is genuine removal:
`set_nested_value(path, OwnedDataValue::Null)` leaves an explicit `null` behind,
which survives serialization because `Message` emits `context` whole. Object
removal preserves the order of the surviving keys; array removal shifts the tail
rather than leaving a hole.

## Connector introspection

Which function configs carry a connector is this crate's fact, so it is exposed
rather than reimplemented downstream.

```rust,ignore
// `Some` for http_call / enrich / publish_kafka (typed field), and for a
// `Custom` input whose `connector` key holds a string. `None` otherwise.
pub fn FunctionConfig::connector(&self) -> Option<&str>

// Every connector reference in a workflow, in task order. One item per task,
// not deduplicated. Works on an uncompiled `Workflow::from_json` result.
pub fn Workflow::connector_refs(&self) -> impl Iterator<Item = ConnectorRef<'_>>

pub struct ConnectorRef<'a> {
    pub workflow_id: &'a str,
    pub task_id: &'a str,
    pub function: &'a str,
    pub connector: &'a str,
    pub config: &'a FunctionConfig,   // for cross-field rules
}
```

Across a whole engine, `engine.workflows().iter().flat_map(Workflow::connector_refs)`
covers it — there is deliberately no `Engine::connector_refs()`, since the engine
has no stake in connectors.

## TaskOutcome

Return value of every handler:

```rust
pub enum TaskOutcome {
    Success,         // audit status 200, continue
    Status(u16),     // audit status = code; 5xx pushes TASK_STATUS_ERROR
    Skip,            // no audit entry, continue
    Halt,            // audit status 299 (HALT_STATUS_CODE), stop workflow
}
```

## FunctionConfig

`FunctionConfig` is an enum: every built-in is a typed variant, and unknown
function names deserialize into `Custom { name, input }`. Custom handlers
typically destructure the `Custom` variant to access their config.

```rust,ignore
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
    Custom {
        name: String,
        input: serde_json::Value,
        // #[serde(skip)] — populated by the engine at .build() with the
        // typed Self::Input for the registered handler.
        compiled_input: Option<CompiledCustomInput>,
    },
}
```

### Classifying a function name

Which names get a typed variant is a fact about this crate, so it is exposed
rather than left to be copied or scraped out of an error message:

```rust,ignore
// Every name that resolves to a typed variant instead of `Custom`.
pub const BUILTIN_FUNCTION_NAMES: &[&str]

// How a built-in reaches an implementation.
pub enum BuiltinKind {
    SelfContained,    // executed by this crate; no registration needed
    RequiresHandler,  // config schema only; needs a registered handler
}

// `None` means the name lands in `FunctionConfig::Custom`.
pub fn builtin_function_kind(name: &str) -> Option<BuiltinKind>

// Equivalent to `builtin_function_kind(name).is_some()`.
pub fn is_builtin_function(name: &str) -> bool
```

`RequiresHandler` covers `http_call`, `enrich` and `publish_kafka`. These parse
without complaint and fail on the first message if no handler is registered, so a
validator that treats them like `SelfContained` will accept a workflow that fails
every request — see
[Integration Functions](../built-in-functions/integrations.md#detecting-a-missing-handler-before-it-fails).

Matching is exact: `"HTTP_CALL"` and `"htttp_call"` are both `None`.

### Service-classified errors

```rust,ignore
// Build a handler-owned error. `kind` becomes ErrorInfo::code verbatim.
pub fn DataflowError::service(kind: impl Into<String>, message: impl Into<String>)
    -> ServiceErrorBuilder

impl ServiceErrorBuilder {
    pub fn detail(self, detail: impl Into<String>) -> Self   // operator-only
    pub fn retryable(self, retryable: bool) -> Self          // default false
    pub fn build(self) -> DataflowError
}

// None for every engine-owned variant.
pub fn DataflowError::kind(&self) -> Option<&str>
pub fn DataflowError::detail(&self) -> Option<&str>
```

`Display` on `DataflowError::Service` renders `message` alone, so `to_string()`
never leaks the detail. `DataflowError` and `ErrorInfo` are `#[non_exhaustive]`.
See [Service-classified errors](../core-concepts/error-handling.md#service-classified-errors).

## Change

Represents a single data modification recorded in the audit trail.

```rust,ignore
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

```rust,ignore
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

## Rollout

Traffic split for a workflow, compared against `Message::routing_bucket()`.

```rust,ignore
pub struct Rollout {
    pub bucket_start: u8,   // inclusive
    pub bucket_end: u8,     // exclusive; 100 means "up to and including 99"
}

impl Rollout {
    // `[0,100)` accepts everything; an empty or inverted range accepts nothing.
    pub fn accepts(&self, bucket: u8) -> bool
}
```

`Workflow::rollout` is `Option<Rollout>`, defaulting to `None` (not part of a
split). A message with **no** bucket is admitted by every workflow. See
[Traffic Splits](../core-concepts/workflow.md#traffic-splits-rollout).

## Built-in Functions

`map`, `validation`/`validate`, `parse_json`, `parse_xml`, `publish_json`,
`publish_xml`, `filter` and `log` are executed by the crate itself
(`BuiltinKind::SelfContained`). `http_call`, `enrich` and `publish_kafka` ship as
typed config only and require a registered handler
(`BuiltinKind::RequiresHandler`) — see
[Classifying a function name](#classifying-a-function-name).

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

Returns `TaskOutcome::Success` (pass), `TaskOutcome::Skip` (no audit
entry, continue), or `TaskOutcome::Halt` (audit status 299, stop
workflow) depending on the condition and `on_reject`.

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

Always returns `TaskOutcome::Success` — never modifies the message.

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
