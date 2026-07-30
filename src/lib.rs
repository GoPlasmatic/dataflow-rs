/*!
# Dataflow-rs

A lightweight rules engine for building IFTTT-style automation and data processing pipelines in Rust.

## Overview

Dataflow-rs provides a high-performance rules engine that follows the **IF → THEN → THAT** model:

- **IF** — Define conditions using JSONLogic expressions (evaluated against `data`, `metadata`, `temp_data`)
- **THEN** — Execute actions: data transformation, validation, or custom async logic
- **THAT** — Chain multiple actions and rules with priority ordering

Rules are defined declaratively in JSON and compiled once at startup for zero-overhead evaluation at runtime.

## Key Components

| Rules Engine | Workflow Engine | Description |
|---|---|---|
| **RulesEngine** | **Engine** | Central async component that evaluates rules and executes actions |
| **Rule** | **Workflow** | A condition + actions bundle — IF condition THEN execute actions |
| **Action** | **Task** | An individual processing step that performs a function on a message |

* **AsyncFunctionHandler**: A trait implemented by action handlers to define custom async processing logic
* **TaskContext**: Per-call context handed to handlers — typed data accessors, audit-trail-aware setters
* **TaskOutcome**: Return value of a handler — `Success`, `Status(code)`, `Skip`, or `Halt`
* **Message**: The data structure that flows through the engine, containing payload, metadata, and processing results

## Built-in Functions

The engine ships with the following pre-registered functions, available to
any workflow without further setup:

| Category | Function | Purpose |
|---|---|---|
| **Parse** | `parse_json` | Deserialize a JSON payload string into `data` |
| **Parse** | `parse_xml` | Deserialize an XML payload string into `data` |
| **Transform** | `map` | Assign JSONLogic-derived values to dot-paths within the message |
| **Validate** | `validation` | Apply JSONLogic rules with custom error messages |
| **Routing** | `filter` | Skip or halt processing based on a JSONLogic predicate |
| **Routing** | `log` | Emit a log entry at a configurable level |
| **Publish** | `publish_json` | Render `data` back out as a JSON payload |
| **Publish** | `publish_xml` | Render `data` back out as an XML payload |

In addition, dataflow-rs provides **typed config schemas** for three common
service-layer integrations — `http_call`, `enrich`, and `publish_kafka`.
These are *not* pre-registered: register an `AsyncFunctionHandler` under the
matching name and the engine handles config validation and JSONLogic
pre-compilation for you. See [`HttpCallConfig`], [`EnrichConfig`], and
[`PublishKafkaConfig`].

Custom functions are registered through `Engine::builder().register(...)`;
see the **Extending with Custom Functions** section below.

## Usage Example

```rust,no_run
use dataflow_rs::{Engine, Workflow};
use dataflow_rs::engine::message::Message;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Define a workflow in JSON
    let workflow_json = r#"
    {
        "id": "premium_order",
        "name": "Premium Order Processing",
        "condition": {">=": [{"var": "data.order.total"}, 1000]},
        "tasks": [
            {
                "id": "apply_discount",
                "name": "Apply Premium Discount",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "data.order.discount",
                                "logic": {"*": [{"var": "data.order.total"}, 0.1]}
                            }
                        ]
                    }
                }
            }
        ]
    }
    "#;

    // Parse the workflow
    let workflow = Workflow::from_json(workflow_json)?;

    // Create the workflow engine — builder is the recommended path; built-in
    // functions are auto-registered.
    let engine = Engine::builder().with_workflow(workflow).build()?;

    // Create a message to process
    let mut message = Message::from_value(&json!({
        "order": {
            "total": 1500
        }
    }));

    // Process the message through the workflow
    match engine.process_message(&mut message).await {
        Ok(_) => {
            println!("Discount: {}", message.data()["order"]["discount"]); // 150
        }
        Err(e) => {
            println!("Error in workflow: {:?}", e);
        }
    }

    Ok(())
}
```

## Error Handling

The library provides a comprehensive error handling system:

```rust,no_run
use dataflow_rs::{Engine, Result, DataflowError};
use dataflow_rs::engine::message::Message;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    // ... setup workflows ...
    let engine = Engine::builder().build()?;

    let mut message = Message::from_value(&json!({}));

    // Process the message, errors will be collected but not halt execution
    engine.process_message(&mut message).await?;

    // Check if there were any errors during processing
    if message.has_errors() {
        for error in message.errors() {
            println!("Error in workflow: {:?}, task: {:?}: {:?}",
                     error.workflow_id, error.task_id, error.message);
        }
    }

    Ok(())
}
```

## Extending with Custom Functions

Implement `AsyncFunctionHandler` with a typed `Input` so the engine deserializes
your config once at startup; handlers then receive typed input and a
`TaskContext` that records audit-trail changes automatically.

```rust,no_run
use dataflow_rs::{
    AsyncFunctionHandler, Engine, Result, TaskContext, TaskOutcome, Workflow,
};
use dataflow_rs::datavalue::OwnedDataValue;
use serde::Deserialize;
use serde_json::json;
use async_trait::async_trait;

#[derive(Deserialize)]
struct StatsInput {
    /// Path inside `data` whose array of numbers to summarize.
    source: String,
    /// Path inside `data` to write the result to.
    target: String,
}

struct Statistics;

#[async_trait]
impl AsyncFunctionHandler for Statistics {
    type Input = StatsInput;

    async fn execute(
        &self,
        ctx: &mut TaskContext<'_>,
        input: &StatsInput,
    ) -> Result<TaskOutcome> {
        let count = ctx.data()
            .get(input.source.as_str())
            .and_then(|v| v.as_array())
            .map(|arr| arr.len())
            .unwrap_or(0);

        ctx.set(
            &format!("data.{}", input.target),
            OwnedDataValue::from(&json!({ "count": count })),
        );
        Ok(TaskOutcome::Success)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let engine = Engine::builder()
        .register("statistics", Statistics)
        // .with_workflow(workflow)
        .build()?;
    // ...
    Ok(())
}
```

## Ecosystem

Dataflow-rs is part of a small family of crates that share the same workflow
and JSONLogic shape:

| Crate | Purpose |
|---|---|
| [`dataflow-rs`](https://crates.io/crates/dataflow-rs) | This crate — async workflow engine in Rust |
| [`@goplasmatic/dataflow-wasm`](https://www.npmjs.com/package/@goplasmatic/dataflow-wasm) | WebAssembly bindings — run workflows in the browser or Node |
| [`@goplasmatic/dataflow-ui`](https://www.npmjs.com/package/@goplasmatic/dataflow-ui) | React components for visualizing and debugging workflows |
| [`datalogic-rs`](https://crates.io/crates/datalogic-rs) | The JSONLogic compiler/evaluator used internally |

Source for all four lives under <https://github.com/GoPlasmatic>.
*/

pub mod engine;
pub mod prelude;

// Re-export all public APIs for easier access
pub use engine::error::{DataflowError, ErrorInfo, Result};
pub use engine::functions::{
    AsyncFunctionHandler, BUILTIN_FUNCTION_NAMES, BoxedFunctionHandler, BuiltinKind, EnrichConfig,
    FilterConfig, FunctionConfig, HttpCallConfig, HttpMethod, LogConfig, MapConfig, MapMapping,
    PublishKafkaConfig, ValidationConfig, ValidationRule, builtin_function_kind,
    is_builtin_function,
};
pub use engine::message::{AuditTrail, Change, Message, MessageBuilder};
pub use engine::observer::{ExecutionObserver, TaskEvent};
pub use engine::task_context::TaskContext;
pub use engine::task_outcome::TaskOutcome;
pub use engine::trace::{AuditTrailScope, ExecutionStep, ExecutionTrace, StepResult, TraceOptions};
pub use engine::{ConnectorRef, Engine, EngineBuilder, Task, Workflow, WorkflowStatus};

/// The [`datalogic_rs`] JSONLogic engine, re-exported.
///
/// [`Engine::datalogic`] and [`TaskContext::datalogic`] both return
/// `&Arc<datalogic_rs::Engine>`, and [`HttpCallConfig::compiled_path_logic`]
/// (with its siblings on [`EnrichConfig`] and [`PublishKafkaConfig`]) is an
/// `Option<Arc<datalogic_rs::Logic>>` — so any crate implementing
/// [`AsyncFunctionHandler`] against those configs has to name these types.
/// Reaching them through here locks their major version to whatever
/// `dataflow-rs` depends on; prefer it over an independent `datalogic-rs`
/// dependency, which can skew.
///
/// Two things to know:
///
/// - `datalogic_rs::Engine` and [`crate::Engine`] are both called `Engine`.
///   This crate imports the former under an alias
///   (`use datalogic_rs::Engine as DatalogicEngine;`); you will want to do the
///   same.
/// - The operator surface is feature-dependent. `datalogic-rs` gates
///   `length`/`starts_with`/`upper`/`split` behind `ext-string`, `sort`/`slice`
///   behind `ext-array`, `abs`/`ceil`/`floor` behind `ext-math`,
///   `exists`/`??`/`switch`/`type` behind `ext-control`, `try`/`throw` behind
///   `error-handling`, and the date operators behind `datetime`. `dataflow-rs`
///   enables none of those. To turn one on, keep a direct `datalogic-rs`
///   dependency with the feature enabled — cargo's additive feature unification
///   then enables it here too. The two arrangements coexist.
pub use datalogic_rs;

/// The [`datavalue`] value-type crate, re-exported.
///
/// [`Message::context`], [`Message::data`], [`TaskContext::get`] /
/// [`TaskContext::set`], [`Change::old_value`] and the [`engine::utils`] path
/// helpers are all expressed in terms of `datavalue::OwnedDataValue`, so it is
/// unavoidable for handler authors.
///
/// Note the crate is published as `datavalue-rs` and used here under the name
/// `datavalue` via a `package =` rename, which makes the correct `Cargo.toml`
/// line hard to guess — reach it as `dataflow_rs::datavalue` instead. Same
/// major-version-skew argument as [`datalogic_rs`].
pub use datavalue;

/// Type alias for `Workflow` — a Rule represents an IF-THEN unit: IF condition THEN execute actions.
pub type Rule = Workflow;

/// Type alias for `Task` — an Action is an individual processing step within a rule.
pub type Action = Task;

/// Type alias for `Engine` — the RulesEngine evaluates rules and executes their actions.
pub type RulesEngine = Engine;

/// Compiles the Rust snippets in `README.md` as doctests so the landing-page
/// examples cannot drift from the API. Compiled only under `cargo test`; this
/// type does not exist in a normal build and is not part of the public API.
///
/// Snippets that are illustrative fragments rather than runnable programs are
/// tagged `ignore` in the README and are skipped here.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
