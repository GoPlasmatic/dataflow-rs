<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  # Dataflow-rs

  **A high-performance rules engine for IFTTT-style automation in Rust with zero-overhead JSONLogic evaluation.**

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![Rust](https://img.shields.io/badge/rust-1.85+-orange.svg)](https://www.rust-lang.org)
  [![Crates.io](https://img.shields.io/crates/v/dataflow-rs.svg)](https://crates.io/crates/dataflow-rs)
  [![docs.rs](https://docs.rs/dataflow-rs/badge.svg)](https://docs.rs/dataflow-rs)
  [![Crates.io Downloads](https://img.shields.io/crates/d/dataflow-rs.svg)](https://crates.io/crates/dataflow-rs)
</div>

---

<div align="center">
  <a href="https://goplasmatic.github.io/dataflow-rs/debugger/">
    <img src="assets/visual-debugger.png" alt="Dataflow visual debugger showing two chained rules: Order Intake always runs, then a JSONLogic condition gates the Premium Perks rule">
  </a>
  <p><em>Two chained rules in the visual debugger — <strong>IF</strong> the condition matches, <strong>THEN</strong> the next rule runs. <a href="https://goplasmatic.github.io/dataflow-rs/debugger/">Try it live in your browser →</a></em></p>
</div>

Dataflow-rs is a lightweight, embeddable rules engine that lets you define **IF → THEN → THAT** automation in JSON. Rules are evaluated using pre-compiled JSONLogic for zero runtime overhead, and actions execute asynchronously for high throughput. Whether you're routing events, validating data, or building complex automation pipelines, Dataflow-rs gives you enterprise-grade performance with minimal complexity.

### ⚡ Blazing Fast Performance
Dataflow-rs is built for high-throughput hot paths. By compiling all JSONLogic expressions once at engine startup, runtime evaluation runs with zero allocations, zero parsing overhead, and predictable latency. 

A multi-threaded benchmark (1,000,000 concurrent events) on a 10-core machine yields:
*   **Throughput:** **~640,000 messages/sec**
*   **Median (P50) Latency:** **6 μs**
*   **Tail (P99) Latency:** **51 μs**
*   **Tail (P99.9) Latency:** **93 μs**

### 🧩 Full-Stack Ecosystem
Go beyond backend microservices. Use the same rule definitions across your entire stack:
1.  **Rust Backend:** Run natively with maximum speed and concurrency using `dataflow-rs`.
2.  **Browser & Edge:** Run client-side validations or edge routing using WebAssembly bindings via [@goplasmatic/dataflow-wasm](https://www.npmjs.com/package/@goplasmatic/dataflow-wasm).
3.  **React UI Admin Portal:** Let users and developers visualize, edit, and step-by-step debug rules using [@goplasmatic/dataflow-ui](https://www.npmjs.com/package/@goplasmatic/dataflow-ui).

## How It Works: IF → THEN → THAT

```
┌─────────────────────────────────────────────────────────────────┐
│  Rule (Workflow)                                                │
│                                                                 │
│  IF    condition matches        →  JSONLogic against any field  │
│  THEN  execute actions (tasks)  →  map, validate, custom logic  │
│  THAT  chain more rules         →  priority-ordered execution   │
└─────────────────────────────────────────────────────────────────┘
```

**Example:** IF `order.total > 1000` THEN `apply_discount` AND `notify_manager`

## Core Concepts

| Rules Engine | Workflow Engine | Description |
|---|---|---|
| **Rule** | **Workflow** | A condition + actions bundle — IF condition THEN execute actions |
| **Action** | **Task** | An individual processing step (map, validate, or custom function) |
| **RulesEngine** | **Engine** | Evaluates rules against messages and executes matching actions |

Both naming conventions are fully supported — use whichever fits your mental model.

## Why dataflow-rs?

If you need dynamic business rules or user-customizable workflows, writing manual `if/else` checks makes your code rigid, while running full orchestrators (like Temporal or Zeebe) adds heavy infrastructure overhead and milliseconds of network latency. Dataflow-rs gives you the best of both worlds:

| Capability | Hardcoded Rust | dataflow-rs | Heavy Orchestrators (Temporal/Zeebe) |
|---|---|---|---|
| **Hot Reload Rules** | ❌ Recompile & redeploy |  Instant JSON update | ❌ Deploy new worker code |
| **Execution Overhead** | None | **Zero (pre-compiled JSONLogic)** | ❌ DB reads/writes (tens of ms) |
| **Browser execution** | ❌ Compile full app to WASM |  Run same rules in JS via WASM | ❌ Network round-trip required |
| **Visual Debugger** | ❌ Build your own UI |  Included React UI components |  Included Dashboard |
| **Infrastructure** | None | **None (embeddable library)** | ❌ Requires server clusters & DBs |

## Getting Started

### 1. Add to `Cargo.toml`

```toml
[dependencies]
dataflow-rs = "3.0"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
serde_json = "1.0"
```

### 2. Define Rules in JSON

```json
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
                        },
                        {
                            "path": "data.order.final_total",
                            "logic": {"-": [{"var": "data.order.total"}, {"*": [{"var": "data.order.total"}, 0.1]}]}
                        }
                    ]
                }
            }
        }
    ]
}
```

### 3. Run the Engine

```rust
use dataflow_rs::{Engine, Workflow};
use dataflow_rs::engine::message::Message;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Paste the JSON rule defined in Step 2:
    let rule_json = r#"{
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
                            },
                            {
                                "path": "data.order.final_total",
                                "logic": {"-": [{"var": "data.order.total"}, {"*": [{"var": "data.order.total"}, 0.1]}]}
                            }
                        ]
                    }
                }
            }
        ]
    }"#;

    let workflow = Workflow::from_json(rule_json)?;

    // Create engine — all JSONLogic compiled once here
    let engine = Engine::builder().with_workflow(workflow).build()?;

    // Process a message
    let payload = json!({"order": {"total": 1500}});
    let mut message = Message::from_value(&payload);
    engine.process_message(&mut message).await?;

    println!("Discount: {}", message.data()["order"]["discount"]); // 150
    println!("Final Total: {}", message.data()["order"]["final_total"]); // 1350
    Ok(())
}
```

### Handling Errors — Two Channels

`process_message` reports errors through **two complementary channels**:

- `Result::Err` signals that the engine **stopped early** (a task failed without
  `continue_on_error`, or an engine-level error occurred).
- `message.errors()` **always** contains every error encountered, including
  errors from tasks that ran with `continue_on_error = true` and so didn't
  short-circuit the workflow.

A short-circuit `?` will surface only the first kind. For full coverage:

```rust
use dataflow_rs::{Engine, Workflow};
use dataflow_rs::engine::message::Message;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::builder()
        .with_workflow(Workflow::from_json(r#"{ ... }"#)?)
        .build()?;

    let mut message = Message::from_value(&json!({"order": {"total": 1500}}));

    // `continue_on_error` tasks may record errors here without returning Err.
    if let Err(e) = engine.process_message(&mut message).await {
        eprintln!("engine halted: {e}");
    }

    // Always iterate `message.errors()` to see everything that went wrong.
    for err in message.errors() {
        eprintln!(
            "[{workflow_id}/{task_id}] {msg}",
            workflow_id = err.workflow_id.as_deref().unwrap_or("-"),
            task_id = err.task_id.as_deref().unwrap_or("-"),
            msg = err.message,
        );
    }

    Ok(())
}
```

### Using Rules Engine Aliases

```rust
use dataflow_rs::{RulesEngine, Rule, Action};

// These are type aliases — same types, rules-engine terminology
let rule = Rule::from_json(r#"{ ... }"#)?;
let engine = RulesEngine::builder().with_workflow(rule).build()?;
```

## Key Features

- **IF → THEN → THAT Model:** Define rules with JSONLogic conditions, execute actions, chain with priority ordering.
- **Zero Runtime Compilation:** All JSONLogic expressions pre-compiled at startup for optimal performance.
- **Full Context Access:** Conditions can access any field — `data`, `metadata`, `temp_data`.
- **Async-First Architecture:** Native async/await support with Tokio for high-throughput processing.
- **Execution Tracing:** Step-by-step debugging with message snapshots after each action.
- **Built-in Functions:** Parse, Map, Validate, Filter, Log, and Publish for complete data pipelines.
- **Pipeline Control Flow:** Filter/gate function to halt workflows or skip tasks based on conditions.
- **Channel Routing:** Route messages to specific workflow channels with O(1) lookup.
- **Workflow Lifecycle:** Manage workflow status (active/paused/archived), versioning, and tagging.
- **Hot Reload:** Swap workflows at runtime without re-registering custom functions.
- **Extensible:** Add custom async actions by implementing the `AsyncFunctionHandler` trait.
- **Typed Integration Configs:** Pre-validated configs for HTTP, Enrich, and Kafka integrations.
- **WebAssembly Support:** Run rules in the browser with `@goplasmatic/dataflow-wasm`.
- **React UI Components:** Visualize and debug rules with `@goplasmatic/dataflow-ui`.
- **Auditing:** Full audit trail of all changes as data flows through the pipeline.

## Architecture

### Compilation Phase (Startup)
1. All JSONLogic expressions compiled once when the Engine is created
2. Compiled logic cached with Arc for zero-copy sharing
3. Validates all expressions early, failing fast on errors

### Execution Phase (Runtime)
1. **Engine** evaluates each rule's condition against the message context
2. Matching rules execute their actions with pre-compiled logic (zero compilation overhead)
3. `process_message()` for normal execution, `process_message_with_trace()` for debugging
4. Each action can be async, enabling I/O operations without blocking

## Performance

On a 10-core machine processing **1,000,000 messages** concurrently (Tokio multi-threaded runtime, `--release`; per message: 1 parse + 6 mappings + 3 validations):

| Metric | Value |
|---|---|
| **Throughput** | ~640,000 msg/sec |
| **Avg Latency** | 10 μs |
| **P50 Latency** | 6 μs |
| **P99 Latency** | 51 μs |
| **P99.9 Latency** | 93 μs |

**Why it's fast:**
- **Pre-Compilation:** All JSONLogic compiled at startup, zero runtime parsing
- **Arc-Wrapped Logic:** Zero-copy sharing of compiled expressions across threads
- **Arena Evaluation:** Consecutive sync tasks evaluate against one bump-arena view of the context; map writes are spliced into it in place instead of re-cloning the written subtree
- **Precomputed Paths:** Mapping, parse, and publish target paths are split and interned at compile time — the hot path never re-parses a path string
- **Async I/O:** Non-blocking operations for external services via Tokio

**Tuning tip:** if you never read audit trails, build messages with
`Message::builder().capture_changes(false)` — skipping the per-mapping
old/new value snapshots is the largest single lever in mapping-heavy
workloads. See the [performance guide](https://goplasmatic.github.io/dataflow-rs/advanced/performance.html) for more.

Run the benchmarks and examples yourself:

```bash
cargo run --example benchmark --release             # Full throughput + latency percentiles
cargo run --example realistic_benchmark --release   # ISO 20022 -> SwiftMT-style workload
cargo run --example micro_aggregate_bench --release # Aggregate-heavy (reduce/map) workload
cargo run --example hello_world           # Minimal getting-started example
cargo run --example rules_engine          # IFTTT-style rules engine demo
cargo run --example complete_workflow     # Parse → Transform → Validate pipeline
cargo run --example custom_function       # Extending the engine with custom handlers
cargo run --example error_handling        # Error handling patterns
```

## Custom Functions

Extend the engine with your own async actions. Each handler declares a typed
`Input` (deserialized once at engine init), receives a `TaskContext` that
records audit-trail changes automatically, and returns a `TaskOutcome`:

```rust
use async_trait::async_trait;
use dataflow_rs::{AsyncFunctionHandler, Engine, Result, TaskContext, TaskOutcome};
use datavalue::OwnedDataValue;
use serde::Deserialize;
use serde_json::json;

/// Typed config for the handler — fails at `Engine::new()` if malformed,
/// not on first message.
#[derive(Deserialize)]
pub struct NotifyInput {
    pub channel: String,
}

pub struct NotifyManager;

#[async_trait]
impl AsyncFunctionHandler for NotifyManager {
    type Input = NotifyInput;

    async fn execute(
        &self,
        ctx: &mut TaskContext<'_>,
        input: &NotifyInput,
    ) -> Result<TaskOutcome> {
        // Your custom async logic here (HTTP calls, DB writes, etc.)
        ctx.set(
            "data.notified_channel",
            OwnedDataValue::from(&json!(input.channel)),
        );
        Ok(TaskOutcome::Success)
    }
}

// Register handlers via the builder. `.register("name", h)` accepts any
// `AsyncFunctionHandler` and boxes it internally.
let engine = Engine::builder()
    .with_workflows(workflows)
    .register("notify_manager", NotifyManager)
    .build()?;
```

## Built-in Functions

| Function | Purpose | Modifies Data |
|----------|---------|---------------|
| `parse_json` | Parse JSON from payload into data context | Yes |
| `parse_xml` | Parse XML string into JSON data structure | Yes |
| `map` | Data transformation using JSONLogic | Yes |
| `validation` | Rule-based data validation | No (read-only) |
| `filter` | Pipeline control flow — halt workflow or skip task | No |
| `log` | Structured logging with JSONLogic expressions | No |
| `publish_json` | Serialize data to JSON string | Yes |
| `publish_xml` | Serialize data to XML string | Yes |

### Filter (Pipeline Control Flow)

The `filter` function evaluates a JSONLogic condition and controls pipeline execution:

```json
{
    "function": {
        "name": "filter",
        "input": {
            "condition": {"==": [{"var": "data.status"}, "active"]},
            "on_reject": "halt"
        }
    }
}
```

- `on_reject: "halt"` — stops the entire workflow when the condition is false
- `on_reject: "skip"` — skips just the current task and continues

### Log (Structured Logging)

The `log` function outputs structured log messages using the `log` crate:

```json
{
    "function": {
        "name": "log",
        "input": {
            "level": "info",
            "message": {"cat": ["Processing order ", {"var": "data.order.id"}]},
            "fields": {
                "total": {"var": "data.order.total"},
                "user": {"var": "data.user.name"}
            }
        }
    }
}
```

Log levels: `trace`, `debug`, `info`, `warn`, `error`. Messages and fields support JSONLogic expressions.

## Channel Routing

Route messages to specific workflow channels for efficient O(1) dispatch:

```rust
// Workflows define their channel
// { "id": "order_rule", "channel": "orders", "status": "active", ... }

// Process only workflows on a specific channel
engine.process_message_for_channel("orders", &mut message).await?;
```

Only `active` workflows are included in channel routing. Workflows default to the `"default"` channel.

## Workflow Lifecycle

Workflows support lifecycle management fields:

```json
{
    "id": "my_rule",
    "channel": "orders",
    "version": 2,
    "status": "active",
    "tags": ["premium", "high-priority"],
    "created_at": "2025-01-15T10:00:00Z",
    "updated_at": "2025-06-01T14:30:00Z",
    "tasks": [...]
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `channel` | string | `"default"` | Channel for message routing |
| `version` | number | `1` | Workflow version |
| `status` | string | `"active"` | `active`, `paused`, or `archived` |
| `tags` | array | `[]` | Arbitrary tags for organization |
| `created_at` | datetime | `null` | Creation timestamp (ISO 8601) |
| `updated_at` | datetime | `null` | Last update timestamp (ISO 8601) |

All fields are optional and backward-compatible with existing configurations.

## Engine Hot Reload

Swap workflows at runtime without losing custom function registrations:

```rust
let new_workflows = vec![Workflow::from_json(r#"{ ... }"#)?];
let new_engine = engine.with_new_workflows(new_workflows);
// Old engine remains valid for in-flight messages
```

## Visualize & Debug Rules

Because every rule is plain JSON, the [React UI](https://www.npmjs.com/package/@goplasmatic/dataflow-ui) can render it: JSONLogic expressions become readable flow diagrams, and the debugger steps through execution with a message diff after every task.

<div align="center">
  <a href="https://goplasmatic.github.io/dataflow-rs/debugger/">
    <img src="assets/jsonlogic-visualizer.png" alt="JSONLogic visualizer rendering an if/else-if chain that assigns gold, silver, or bronze loyalty tiers based on the order total">
  </a>
</div>

## Ecosystem

| Package | Description | Install |
|---------|-------------|-------|
| [dataflow-rs](https://crates.io/crates/dataflow-rs) | Async rules engine in Rust (this crate) | `cargo add dataflow-rs` |
| [@goplasmatic/dataflow-wasm](https://www.npmjs.com/package/@goplasmatic/dataflow-wasm) | WebAssembly bindings — run rules in browser or Node.js | `npm i @goplasmatic/dataflow-wasm` |
| [@goplasmatic/dataflow-ui](https://www.npmjs.com/package/@goplasmatic/dataflow-ui) | React components for rule visualization, editing, and step-by-step debugging | `npm i @goplasmatic/dataflow-ui` |
| [datalogic-rs](https://crates.io/crates/datalogic-rs) | JSONLogic compiler/evaluator used internally | `cargo add datalogic-rs` |

📖 **Documentation:** [User Guide & API Reference](https://goplasmatic.github.io/dataflow-rs/) · [Interactive Playground](https://goplasmatic.github.io/dataflow-rs/playground.html) · [Visual Debugger](https://goplasmatic.github.io/dataflow-rs/debugger/)

## Contributing

We welcome contributions! Here's how to get started:

1. **Fork** the repository and clone your fork
2. **Run tests:** `cargo test` to ensure everything passes
3. **Make changes** and add tests for any new features
4. **Run the benchmark** before and after: `cargo run --example benchmark --release`
5. **Submit a pull request** with a clear description of your changes

See the [CHANGELOG](CHANGELOG.md) for recent changes and release history.

## About Plasmatic

Dataflow-rs is developed by the team at [Plasmatic](https://github.com/GoPlasmatic). We're passionate about building open-source tools for data processing and automation.

## License

This project is licensed under the Apache License, Version 2.0. See the [LICENSE](LICENSE) file for more details.
