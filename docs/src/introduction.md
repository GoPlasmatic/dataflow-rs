<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  <h1>Dataflow-rs</h1>

  <strong>A high-performance workflow engine for building data processing pipelines in Rust with zero-overhead JSONLogic evaluation.</strong>

  <br><br>

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![Rust](https://img.shields.io/badge/rust-1.70+-orange.svg)](https://www.rust-lang.org)
  [![Crates.io](https://img.shields.io/crates/v/dataflow-rs.svg)](https://crates.io/crates/dataflow-rs)
</div>

---

**Dataflow-rs** is a high-performance workflow engine for building data processing pipelines in Rust with zero-overhead JSONLogic evaluation.

Whether you're building REST APIs, processing Kafka streams, or creating sophisticated data transformation pipelines, Dataflow-rs provides enterprise-grade performance with minimal complexity.

## Key Features

- **Async-First Architecture** - Native async/await support with Tokio for high-throughput processing
- **Zero Runtime Compilation** - All JSONLogic expressions pre-compiled at startup for optimal performance
- **Execution Tracing** - Step-by-step debugging with message snapshots after each task
- **Built-in Functions** - Parse (JSON/XML), Map, Validate, and Publish (JSON/XML) for complete data pipelines
- **Dynamic Workflows** - Use JSONLogic to control workflow execution based on your data
- **Extensible** - Easily add custom async processing steps (tasks) to the engine
- **WebAssembly Support** - Run workflows in the browser with `@goplasmatic/dataflow-wasm`
- **React UI Components** - Visualize and debug workflows with `@goplasmatic/dataflow-ui`
- **Auditing** - Track all changes to your data as it moves through the pipeline

## Try It Now

Experience the power of dataflow-rs directly in your browser. Define a workflow and message, then see the processing result instantly.

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution, breakpoints, and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"transform","name":"Transform Workflow","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"map_data","name":"Map Data","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"data.input.name"},"!"]}},{"path":"data.processed","logic":true}]}}}]}]' data-payload='{"name":"World"}'>
</div>

## How It Works

1. **Define Workflows** - Create JSON-based workflow definitions with tasks
2. **Create an Engine** - Initialize the engine with your workflows (compiled once at startup)
3. **Process Messages** - Send messages through the engine for processing
4. **Get Results** - Receive transformed data with full audit trail

## Next Steps

- [Installation](./getting-started/installation.md) - Add dataflow-rs to your project
- [Quick Start](./getting-started/quick-start.md) - Build your first workflow
- [Playground](./playground.md) - Experiment with workflows interactively
