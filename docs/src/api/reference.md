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
`.with_error_context_path(path)`, `.with_error_context_limit(n)`,
`.with_secrets(value)` / `.with_secrets_json(&json)`, then
`.build() -> Result<Engine>`. All JSONLogic is compiled and Custom
inputs are pre-parsed into their typed `Self::Input` at `.build()` —
config-shape errors fail there, not on first message. An error-context
path that the JSONLogic evaluation context cannot see fails there too, as
does a workflow that reads an undeclared secret or reads any secret from a
`map` or `log` expression (see [Secrets](../advanced/secrets.md)).

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

// Create a new engine with different workflows, preserving custom functions.
// Fallible: the new definitions are compiled and validated here.
pub fn with_new_workflows(&self, workflows: Vec<Workflow>) -> Result<Self>

// Attach a lifecycle observer, consuming and returning the engine
pub fn with_observer(self, observer: Arc<dyn ExecutionObserver>) -> Self

// Borrow the underlying JSONLogic engine
pub fn datalogic(&self) -> &Arc<datalogic_rs::Engine>
```

### Introspection

Added in 3.7.0, for hosts that store, validate and operate workflow
definitions. `can_dispatch`, `dispatchable_functions` and `check_workflow` are
available on both `Engine` and `EngineBuilder`, so a definition can be checked
before anything is built. `operator_names` and `declared_secrets` are on
`Engine` only.

```rust,ignore
// Will a task named `name` actually run? `false` guarantees it fails with
// FunctionNotFound on the first message that reaches it.
pub fn can_dispatch(&self, name: &str) -> bool

// The full vocabulary this engine dispatches. Aliases are grouped, so
// `validate` appears once carrying ["validation"]. Ordering is not meaningful.
pub fn dispatchable_functions(&self) -> impl Iterator<Item = DispatchableFunction<'_>>

// Check a workflow against the registered handlers and the secret store
// without building anything. Reports rather than aborts; empty means build()
// will not reject it for these reasons. Covers UnknownFunction,
// MissingHandler, InputParse, TemplateCompile, UnknownSecret,
// SecretInMessageWrite, DuplicateTemplateKey — plus EscapedTemplateKey, which
// is informational and never refused by build().
pub fn check_workflow(&self, workflow: &Workflow) -> Vec<WorkflowIssue>

// The names in the secret store — what {"secret": "name"} can resolve. Names
// only, never values. Ordering is not meaningful.
pub fn declared_secrets(&self) -> impl Iterator<Item = &str>

// Every operator name this build evaluates: core, plus enabled families, plus
// custom registrations. See the JSONLogic page on operator families.
pub fn operator_names(&self) -> impl Iterator<Item = &str> + '_

// The prefix that escapes a template key, so the key is emitted as data
// instead of resolving as an operator. '$' on every build, fixed for the life
// of the engine — this exists so an authoring tool renders and validates the
// spelling without hardcoding it, not because it varies.
pub fn template_key_escape(&self) -> char
```

`template_key_escape` is the companion to `operator_names`: that answers *which
names are live*, this answers *how to opt a key out of being one*. See
[Literal keys and the `$` escape](../advanced/jsonlogic.md#literal-keys-and-the--escape).

```rust,ignore
pub struct DispatchableFunction<'a> {
    pub name: &'a str,
    /// `Some(..)` for a built-in; `None` for a registered custom handler.
    pub kind: Option<BuiltinKind>,
    pub aliases: &'static [&'static str],
}
```

`can_dispatch` answers the half of the question `builtin_function_kind` cannot:
that function reports `enrich` *needs* a handler, but not whether one is
registered. A workflow using a config-only integration with nothing behind it
still builds cleanly — deliberately — so this is the check that catches it
before activation rather than on the first request.

## Authoring-time validation

Check a definition before it ever reaches an engine. See
[Authoring-Time Validation](../advanced/authoring-validation.md) for the
submission-time sequence.

```rust,ignore
// Check a definition's JSON without building an engine. Collects *every*
// problem rather than failing at the first, each carrying the coordinate the
// author typed (`tasks[1].tasks[0].id`), a stable code, and a message.
//
// Returns empty if and only if the JSON parses into a Workflow and that
// workflow validates.
impl Workflow {
    pub fn validate_authored(json: &serde_json::Value) -> Vec<WorkflowIssue>
}

pub struct WorkflowIssue {
    pub code: IssueCode,
    /// Human-readable. Not stable — branch on `code`.
    pub message: String,
    /// Authored coordinate, e.g. `tasks[1].tasks[0].id`.
    pub path: Option<String>,
    pub step_id: Option<String>,
}

#[non_exhaustive]
pub enum IssueCode { /* ... */ }

impl IssueCode {
    pub fn as_str(&self) -> &'static str
}
```

`IssueCode` is an enum rather than string codes because a host branching on a
string literal has no protection against a typo that compiles and silently
never matches. Its variants cover the structural rules —
`EmptyWorkflowId`, `EmptyWorkflowName`, `NoTasks`, `MissingStepId`,
`DuplicateStepId`, `EmptyGroup`, `GroupTooDeep`, `MissingFunction`,
`InvalidFunctionName`, `InvalidTerminal`, `LoopIncrementTooSmall`,
`LoopBoundEmpty`, `LoopCounterInvalid` — the registry rules reported by
`check_workflow` — `UnknownFunction`, `MissingHandler`, `InputParse`,
`TemplateCompile`, `UnknownSecret`, `SecretInMessageWrite`,
`DuplicateTemplateKey`, `EscapedTemplateKey` — and the two backstops,
`ParseFailed` and `ValidateFailed`.

`EscapedTemplateKey` is the one **informational** code: `check_workflow` reports
it and `build()` never refuses it. It lists every `$`-prefixed template key, so
a host upgrading to 3.9 can find each place the escape changed what a template
emits. `DuplicateTemplateKey`, by contrast, is always a bug and is refused.

`MissingHandler` is deliberately distinct from `UnknownFunction`: `enrich`,
`http_call` and `publish_kafka` are real names awaiting a registration, and
reporting them as unknown would send an author hunting a typo that is not there.

`check_workflow` receives an already-flattened workflow, so its issues anchor on
`task_id` with a task-relative path (`function.input`) rather than an authored
coordinate. Join them with the walker to recover one:

```rust,ignore
// Walk the authored step tree — tasks and groups, in document order — yielding
// each step with the coordinate the author typed.
pub fn walk_authored_steps(tasks: &serde_json::Value) -> AuthoredSteps<'_>
```

## Retry

Native only — the loop uses tokio time, so it is not compiled for
`wasm32-unknown-unknown`. Nothing in the engine retries a task for you; this is
the loop to wrap your own fallible calls in, typically inside a custom handler.

```rust,ignore
pub struct RetryPolicy {
    /// Retries *after* the first attempt. `0` means try once and give up.
    pub max_retries: u32,
    /// Base delay in milliseconds. Doubles per attempt, capped at 60s.
    pub retry_delay_ms: u64,
    /// Wall-clock ceiling for the whole loop, sleeps included.
    pub deadline: Option<Duration>,
}

impl RetryPolicy {
    /// Three retries, 100ms base delay, no deadline.
    fn default() -> Self
    /// Never retries — an explicit opt-out at a call site that takes a policy.
    pub fn none() -> Self
}

// Run `operation`, retrying while it fails *retryably* and budget remains.
pub async fn retry_with_policy<T, F, Fut>(policy: RetryPolicy, label: &str, operation: F) -> Result<T>

// As above, also reporting how many attempts were made — the count that fills
// ErrorInfo::retry_attempted and retry_count.
pub async fn retry_with_attempts<T, F, Fut>(policy: RetryPolicy, label: &str, operation: F) -> (Result<T>, u32)
```

Two behaviours worth knowing:

- **Retryability is declared, not inferred.** The loop consults
  `DataflowError::retryable()`, so a validation error fails once and returns
  immediately. For your own failures, use
  `DataflowError::service(..).retryable(true).build()`.
- **A backoff that would cross the deadline is skipped, not slept**, and the
  loop ends with the last error. The deadline bounds the sleeps; it does not
  abort an attempt already in flight, so pair it with a per-attempt timeout if
  your operation can hang.

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
    "name": "string (required)",
    "description": "string (optional)",
    "priority": "number (optional, default: 0)",
    "condition": "JSONLogic (optional, evaluated against full context)",
    "continue_on_error": "boolean (optional, default: false)",
    "tasks": "array of Task or TaskGroup (required)",
    "channel": "string (optional, default: 'default')",
    "version": "number (optional, default: 1)",
    "status": "'active' | 'paused' | 'archived' (optional, default: 'active')",
    "tags": "array of string (optional, default: [])",
    "rollout": "{bucket_start, bucket_end} over 0..100 (optional, default: none)",
    "loop": "{max, init, increment, counter} (optional, default: none)",
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
    "name": "string (required)",
    "description": "string (optional)",
    "condition": "JSONLogic (optional, evaluated against full context)",
    "continue_on_error": "boolean (optional, default: false)",
    "terminal": "boolean (optional, default: false)",
    "function": {
        "name": "string (required)",
        "input": "object (required)"
    }
}
```

A step carrying a `tasks` key parses as a **group** instead, whose members share
one condition evaluated once on entry:

```json
{
    "id": "string (required)",
    "name": "string (optional)",
    "description": "string (optional)",
    "condition": "JSONLogic (optional, evaluated once on entry)",
    "terminal": "boolean (optional, default: false)",
    "tasks": "array of Task or TaskGroup (required)"
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

A config field whose authored JSON is JSONLogic. Every parameter of every
built-in is one, and custom handlers declare them for their own config. A
literal is JSONLogic for itself, so the static spelling an author already writes
folds to a constant at `build()` and is cached.

```rust,ignore
pub struct Template { /* opaque */ }

impl Template {
    // Called from `AsyncFunctionHandler::compile_input`.
    pub fn compile(&mut self, c: &TemplateCompiler, label: &str) -> Result<()>

    // The sanctioned reads: the cached constant when the expression folded,
    // otherwise a fresh evaluation.
    pub fn resolve(&self, ctx: &TaskContext<'_>) -> Result<OwnedDataValue>
    pub fn resolve_string(&self, ctx: &TaskContext<'_>) -> Result<String>
    pub fn resolve_u64(&self, ctx: &TaskContext<'_>, label: &str) -> Result<u64>

    pub fn eval(&self, ctx: &TaskContext<'_>) -> Result<OwnedDataValue>
    pub fn eval_into<T: serde::de::DeserializeOwned>(&self, ctx: &TaskContext<'_>) -> Result<T>

    pub fn as_json(&self) -> &serde_json::Value
    pub fn is_compiled(&self) -> bool
    // Whether the expression folded to a compile-time constant, so every
    // resolve_* returns a cached value instead of evaluating.
    pub fn is_constant(&self) -> bool
    pub fn constant_string(&self) -> Option<String>
}

// A config field naming a *write destination*. `R` fixes the rooting:
// ContextRoot for a path that names its own root (`data.x`), DataRoot for one
// relative to `data`. A constant destination precomputes its split write path
// at build(), which is what keeps the map hot loop allocation-free.
pub struct PathTemplate<R: PathRoot = ContextRoot> { /* opaque */ }

// Handed to `compile_input`; wraps the same shared datalogic engine
// `LogicCompiler` uses internally, so a compiled `Template` evaluates against
// the same engine that will run the message.
pub struct TemplateCompiler { /* opaque */ }
impl TemplateCompiler {
    pub fn engine(&self) -> &datalogic_rs::Engine
}
```

Any config field may be a `Template`. The engine compiles with templating
enabled, so a single-key object whose key matches an operator name evaluates as
that operator — write `{"$cat": …}` for the literal object. A `Template` that
folds to a constant is evaluated once at `build()` and cached. See
[Config fields that are JSONLogic](../advanced/custom-functions.md#config-fields-that-are-jsonlogic-template)
and [Literal keys and the `$` escape](../advanced/jsonlogic.md#literal-keys-and-the--escape).

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
    // Execution identity. `None` only where the engine has no task to name —
    // e.g. a handler invoked outside a workflow run in a host's own test.
    pub fn workflow_id(&self) -> Option<&str>
    pub fn task_id(&self) -> Option<&str>
    /// Counter value of the sweep this call belongs to, for a looping
    /// workflow; `None` otherwise.
    pub fn loop_counter(&self) -> Option<i64>

    // A secret by dotted name, from the store the host configured with
    // `with_secrets`. `None` when undeclared, and always `None` for a context
    // built with `TaskContext::new`. See the Secrets page for the contract.
    pub fn secret(&self, name: &str) -> Option<&OwnedDataValue>

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
pub fn FunctionConfig::connector(&self) -> Option<ConnectorName<'_>>

// A connector parameter is JSONLogic, so it names something only once a
// message is in hand.
pub enum ConnectorName<'a> {
    Static(&'a str),    // authored as a literal string; known without a message
    Computed(&'a Value),// authored as an expression; carries the authored JSON
}

impl<'a> ConnectorName<'a> {
    // The literal name, or None when the connector is computed. The narrowing
    // accessor for callers that genuinely only handle static connectors.
    pub fn as_static(&self) -> Option<&'a str>
}

// Every connector reference in a workflow, in task order. One item per task,
// not deduplicated. Works on an uncompiled `Workflow::from_json` result.
pub fn Workflow::connector_refs(&self) -> impl Iterator<Item = ConnectorRef<'_>>

pub struct ConnectorRef<'a> {
    pub workflow_id: &'a str,
    pub task_id: &'a str,
    pub function: &'a str,
    pub connector: ConnectorName<'a>,
    pub config: &'a FunctionConfig,   // for cross-field rules
}
```

> **Changed in 3.9.0.** `connector()` and `ConnectorRef::connector` were
> `&str`. Every parameter became JSONLogic, so a computed connector names
> nothing until a message arrives — returning the enum makes a host enumerating
> connectors decide what to do with those rather than have them silently vanish
> from `connector_refs`. Prefer matching the enum; `as_static()` is there for
> the cases that genuinely only handle literals. Resolve a computed one per
> message with the config's `resolve_connector(ctx)?`.

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
    /// Counter value of the sweep that produced this entry, for workflows
    /// carrying a `loop`; `None` otherwise. Omitted when serializing, so a
    /// non-looping workflow's audit JSON is unchanged.
    pub loop_counter: Option<i64>,
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

    // Ordered percentages -> contiguous ranges covering exactly 0..100, in
    // traffic order. A `0` entry yields an empty range that accepts nothing.
    pub fn partition(percentages: &[u8]) -> Result<Vec<Rollout>, RolloutError>

    // Check that a set — typically the live versions of one logical workflow —
    // partitions 0..100: every bucket served, none served twice.
    // Order-independent.
    pub fn validate_set<'a>(
        rollouts: impl IntoIterator<Item = &'a Rollout>,
    ) -> Result<(), RolloutError>
}

// Its own error type rather than a DataflowError variant: pure arithmetic over
// the bucket space, with no engine involvement and no retryability to classify.
pub enum RolloutError {
    Under { total: u32 },        // percentages sum below 100 — traffic silently dropped
    Over { total: u32 },         // sum above 100 — later entries can never match
    Gap { bucket: u8 },          // no range serves this bucket
    Overlap { bucket: u8 },      // more than one range serves it
    InvalidRange { rollout: Rollout },  // inverted, or reaching past bucket 100
}
```

`partition` and `validate_set` are the two halves of keeping a *set* correct:
build the ranges from percentages, or check ranges you already hold. Neither is
run by the engine — a single workflow's `rollout` is never validated at build
time, so an inverted range simply serves nobody.

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

## WASM API (@goplasmatic/dataflow-wasm)

For browser/JavaScript usage. Everything crossing the boundary is a string —
see the [WASM Package](../wasm/overview.md) page for the full contract.

```javascript
import init, { WasmEngine, process_message, engine_version } from '@goplasmatic/dataflow-wasm';

// Initialize the module once, before touching any other export
await init();

// Create engine
const engine = new WasmEngine(workflowsJson);

// Process a payload string; resolves to a serialized Message
const result = JSON.parse(await engine.process(payloadStr));

// Same run, with the execution trace instead
const traced = JSON.parse(await engine.process_with_trace(payloadStr));

// One-off convenience function (no engine needed)
const result2 = JSON.parse(await process_message(workflowsJson, payloadStr));

// Get rule info
const count = engine.workflow_count();          // number
const ids = JSON.parse(engine.workflow_ids());  // JSON array, returned as a string
const version = engine_version();               // e.g. "3.7.0"
```

## Full API Documentation

For complete API documentation, run:

```bash
cargo doc --open
```

This generates detailed documentation from the source code comments.
