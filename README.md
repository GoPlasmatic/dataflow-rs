<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  # Dataflow-rs

  **A high-performance rules engine for IFTTT-style automation in Rust with zero-overhead JSONLogic evaluation.**

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![Rust](https://img.shields.io/badge/rust-1.85+-orange.svg)](https://www.rust-lang.org)
  [![Crates.io](https://img.shields.io/crates/v/dataflow-rs.svg)](https://crates.io/crates/dataflow-rs)
</div>

---

Dataflow-rs is a lightweight rules engine that lets you define **IF → THEN → THAT** automation in JSON. Rules are evaluated using pre-compiled JSONLogic for zero runtime overhead, and actions execute asynchronously for high throughput. Whether you're routing events, validating data, or building complex automation pipelines, Dataflow-rs gives you enterprise-grade performance with minimal complexity.

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

## Getting Started

### 1. Add to `Cargo.toml`

```toml
[dependencies]
dataflow-rs = "2.1"
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
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workflow = Workflow::from_json(r#"{ ... }"#)?; // Your rule JSON

    // Create engine — all JSONLogic compiled once here
    let engine = Engine::new(vec![workflow], None);

    // Process a message
    let payload = Arc::new(json!({"order": {"total": 1500}}));
    let mut message = Message::new(payload);
    engine.process_message(&mut message).await?;

    println!("Discount: {}", message.data()["order"]["discount"]); // 150
    Ok(())
}
```

### Using Rules Engine Aliases

```rust
use dataflow_rs::{RulesEngine, Rule, Action};

// These are type aliases — same types, rules-engine terminology
let rule = Rule::from_json(r#"{ ... }"#)?;
let engine = RulesEngine::new(vec![rule], None);
```

## Key Features

- **IF → THEN → THAT Model:** Define rules with JSONLogic conditions, execute actions, chain with priority ordering.
- **Zero Runtime Compilation:** All JSONLogic expressions pre-compiled at startup for optimal performance.
- **Full Context Access:** Conditions can access any field — `data`, `metadata`, `temp_data`.
- **Async-First Architecture:** Native async/await support with Tokio for high-throughput processing.
- **Execution Tracing:** Step-by-step debugging with message snapshots after each action.
- **Built-in Functions:** Parse (JSON/XML), Map, Validate, and Publish (JSON/XML) for complete data pipelines.
- **Extensible:** Add custom async actions by implementing the `AsyncFunctionHandler` trait.
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

- **Pre-Compilation:** All JSONLogic compiled at startup, zero runtime overhead
- **Arc-Wrapped Logic:** Zero-copy sharing of compiled expressions
- **Context Arc Caching:** 50% improvement via cached Arc context
- **Async I/O:** Non-blocking operations for external services
- **Predictable Latency:** No runtime allocations for logic evaluation

```bash
cargo run --example benchmark           # Performance benchmark
cargo run --example rules_engine        # IFTTT-style rules engine demo
cargo run --example complete_workflow   # Parse → Transform → Validate pipeline
```

## Custom Functions

Extend the engine with your own async actions:

```rust
use async_trait::async_trait;
use dataflow_rs::engine::{
    AsyncFunctionHandler, FunctionConfig,
    error::Result, message::{Change, Message}
};
use datalogic_rs::DataLogic;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

pub struct NotifyManager;

#[async_trait]
impl AsyncFunctionHandler for NotifyManager {
    async fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
        datalogic: Arc<DataLogic>,
    ) -> Result<(usize, Vec<Change>)> {
        // Your custom async logic here (HTTP calls, DB writes, etc.)
        Ok((200, vec![]))
    }
}

// Register when creating the engine:
let mut custom_functions: HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>> = HashMap::new();
custom_functions.insert("notify_manager".to_string(), Box::new(NotifyManager));

let engine = Engine::new(workflows, Some(custom_functions));
```

## Built-in Functions

| Function | Purpose | Modifies Data |
|----------|---------|---------------|
| `parse_json` | Parse JSON from payload into data context | Yes |
| `parse_xml` | Parse XML string into JSON data structure | Yes |
| `map` | Data transformation using JSONLogic | Yes |
| `validation` | Rule-based data validation | No (read-only) |
| `publish_json` | Serialize data to JSON string | Yes |
| `publish_xml` | Serialize data to XML string | Yes |

## Related Packages

| Package | Description |
|---------|-------------|
| [@goplasmatic/dataflow-wasm](https://www.npmjs.com/package/@goplasmatic/dataflow-wasm) | WebAssembly bindings for browser execution |
| [@goplasmatic/dataflow-ui](https://www.npmjs.com/package/@goplasmatic/dataflow-ui) | React components for rule visualization and debugging |

## Contributing

We welcome contributions! Feel free to fork the repository, make your changes, and submit a pull request. Please make sure to add tests for any new features.

## About Plasmatic

Dataflow-rs is developed by the team at [Plasmatic](https://github.com/GoPlasmatic). We're passionate about building open-source tools for data processing and automation.

## License

This project is licensed under the Apache License, Version 2.0. See the [LICENSE](LICENSE) file for more details.
