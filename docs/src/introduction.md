<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  <h1>Dataflow-rs</h1>

  <strong>A high-performance rules engine for IFTTT-style automation in Rust with zero-overhead JSONLogic evaluation.</strong>

  <br><br>

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![Rust](https://img.shields.io/badge/rust-1.85+-orange.svg)](https://www.rust-lang.org)
  [![Crates.io](https://img.shields.io/crates/v/dataflow-rs.svg)](https://crates.io/crates/dataflow-rs)
</div>

---

**Dataflow-rs** is a lightweight rules engine that lets you define **IF → THEN → THAT** automation in JSON. Rules are evaluated using pre-compiled JSONLogic for zero runtime overhead, and actions execute asynchronously for high throughput.

Whether you're routing events, validating data, building REST APIs, or creating automation pipelines, Dataflow-rs provides enterprise-grade performance with minimal complexity.

## Key Features

- **IF → THEN → THAT Model** - Define rules with JSONLogic conditions, execute actions, chain with priority ordering
- **Async-First Architecture** - Native async/await support with Tokio for high-throughput processing
- **Zero Runtime Compilation** - All JSONLogic expressions pre-compiled at startup for optimal performance
- **Full Context Access** - Conditions can access any field: `data`, `metadata`, `temp_data`
- **Execution Tracing** - Step-by-step debugging with message snapshots after each action
- **Built-in Functions** - Parse (JSON/XML), Map, Validate, and Publish (JSON/XML) for complete data pipelines
- **Extensible** - Easily add custom async actions to the engine
- **WebAssembly Support** - Run rules in the browser with `@goplasmatic/dataflow-wasm`
- **React UI Components** - Visualize and debug rules with `@goplasmatic/dataflow-ui`
- **Auditing** - Track all changes to your data as it moves through the pipeline

## Try It Now

Experience the power of dataflow-rs directly in your browser. Define a rule and message, then see the processing result instantly.

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution, breakpoints, and rule visualization.

<div class="playground-widget" data-workflows='[{"id":"transform","name":"Transform Rule","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"map_data","name":"Map Data","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"data.input.name"},"!"]}},{"path":"data.processed","logic":true}]}}}]}]' data-payload='{"name":"World"}'>
</div>

## How It Works

```
┌─────────────────────────────────────────────────────────────────┐
│  Rule (Workflow)                                                │
│                                                                 │
│  IF    condition matches        →  JSONLogic against any field  │
│  THEN  execute actions (tasks)  →  map, validate, custom logic  │
│  THAT  chain more rules         →  priority-ordered execution   │
└─────────────────────────────────────────────────────────────────┘
```

1. **Define Rules** - Create JSON-based rule definitions with conditions and actions
2. **Create an Engine** - Initialize the rules engine (all logic compiled once at startup)
3. **Process Messages** - Send messages through the engine for evaluation
4. **Get Results** - Receive transformed data with full audit trail

## Next Steps

- [Installation](./getting-started/installation.md) - Add dataflow-rs to your project
- [Quick Start](./getting-started/quick-start.md) - Build your first rule
- [Playground](./playground.md) - Experiment with rules interactively
