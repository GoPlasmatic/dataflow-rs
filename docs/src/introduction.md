<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  <h1>Dataflow-rs</h1>

  <strong>A high-performance rules engine for IFTTT-style automation in Rust with zero-overhead JSONLogic evaluation.</strong>

  <br><br>

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![Rust](https://img.shields.io/badge/rust-1.98+-orange.svg)](https://www.rust-lang.org)
  [![Crates.io](https://img.shields.io/crates/v/dataflow-rs.svg)](https://crates.io/crates/dataflow-rs)
</div>

**Dataflow-rs** is a lightweight, embeddable rules engine that lets you define **IF → THEN → THAT** automation in JSON. A definition is parsed and linted once, compiled into a program, and run over messages; every JSONLogic expression is compiled at build time, and a run reads the clock once.

Use it to route events, validate data, power REST APIs, or build automation pipelines.

### ⚡ Performance

Dataflow-rs targets high-throughput hot paths:

*   **Every expression is compiled at build time.** A workflow that does not compile is rejected before any message runs; a constant expression is evaluated once and kept.
*   **One context view per run.** The built-ins run inline over one view of the message's context, and a host function reads that same view and writes to an overlay of its own, so no view is built per call.
*   **One clock read per run.** `metadata.processed_at`, every audit entry and every error record carry that one reading. The clock is read again only for an observer or a trace that asks for timings.
*   **No tracing cost unless asked for.** With no observer, trace or error context attached, a call takes the plain recorder.

See [Performance](./advanced/performance.md) for the benchmarks and how to run them.

### 🧩 Why Choose dataflow-rs?

If you need dynamic business rules or user-customizable workflows, writing hardcoded `if/else` checks makes your codebase rigid, while running heavy workflow orchestrators (like Temporal or Zeebe) adds infrastructure dependencies and milliseconds of database/network latency. Dataflow-rs sits between the two:

| Feature | Hardcoded Rust | dataflow-rs | Orchestrators (Temporal/Zeebe) |
|---|---|---|---|
| **Reload Rules** | ❌ Recompile & redeploy | ✅ Compile new JSON into a new engine | ❌ Deploy new worker code |
| **Execution Overhead** | None | **Zero (pre-compiled)** | ❌ DB reads/writes (tens of ms) |
| **Browser Execution** | ❌ WASM compile size | ✅ Run rules via WASM | ❌ Server round-trip required |
| **Visual Debugger** | ❌ Build your own UI | ✅ Included React UI components | ✅ Included Dashboard |
| **Infrastructure** | None | **None (embeddable library)** | ❌ Server clusters & DBs |

## Key Features

- **IF → THEN → THAT Model** - Define workflows with JSONLogic conditions, execute tasks, chain workflows with priority ordering
- **Parse and Lint in One Pass** - `def::parse` reads a definition and reports every issue at the path the author typed, before anything is compiled
- **Compiled Once** - A `Compiler` compiles each workflow on its own, so one rejected workflow never takes the others down, and links them into an immutable `Program`
- **A Report for Every Run** - `engine.run(&mut message).await` returns a `Run` on every outcome: how it ended, which workflows matched, and what each did
- **Full Context Access** - Conditions and mappings read any field of `data`, `metadata` and `temp_data`
- **Typed Host Functions** - Implement `Function` with a `#[derive(FunctionConfig)]` config; the config is compiled at build time, and a call's writes reach the message only when it returns `Ok`
- **Layers** - Wrap every host call at registry or run scope: logging, stubbing, dry runs
- **Deadlines and Cancellation** - A run ends `TimedOut` or `Cancelled` at the next check, and a host call is raced against both
- **Fan-Out** - Run one task's function once per element of an array, isolated, folded in element order
- **Task Groups** - Nest tasks under one shared condition, with a terminal group that ends a workflow early
- **Outcome Policies** - A task's `on_failure` and `on_rejection` say what a failure or a rejection does; a workflow's `on_failure` says whether a failure stops the run
- **Bounded Loops** - Re-run a workflow's task list a fixed number of times, with the sweep counter in `temp_data`
- **Traffic Splits** - Roll a workflow out to a slice of its channel's traffic with `rollout`
- **Observers** - One `on(&Event)` callback with a declared interest; the clock is read only for an observer that asks for timings
- **Traces** - The initial context plus each step's changes, on every status, replayable to the context after any step
- **Error Context** - Mirror each failure's code into the context, so a later condition can branch on why a task failed
- **Secrets Outside the Record** - `{"secret": "name"}` reads an engine-scoped store that no trace, message or log ever contains
- **Built-in Functions** - Parse, Map, Validation, Filter, Log and Publish for complete data pipelines
- **Channels and Lifecycle** - Route a run to one channel's workflows; pause or archive a workflow without deleting it
- **3.x Definitions Load** - The compat reader rewrites 3.x spellings and reports each one, so an installed base migrates at its own pace
- **WebAssembly Support** - Run rules in the browser with `@goplasmatic/dataflow-wasm`
- **React UI Components** - Visualize and debug rules with `@goplasmatic/dataflow-ui`
- **Auditing** - Every write a task makes is recorded on the message's audit trail

## Try It Now

Run dataflow-rs in your browser. Define a workflow and message, then see the processing result.

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution, breakpoints, and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"transform","name":"Transform","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"map_data","name":"Map Data","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"data.input.name"},"!"]}},{"path":"data.processed","logic":true}]}}}]}]' data-payload='{"name":"World"}'>
</div>

## How It Works

```text
┌─────────────────────────────────────────────────────────────────┐
│  Workflow                                                       │
│                                                                 │
│  IF    condition matches        →  JSONLogic against any field  │
│  THEN  execute tasks            →  map, validation, host logic  │
│  THAT  chain more workflows     →  priority-ordered execution   │
└─────────────────────────────────────────────────────────────────┘
```

1. **Define Workflows** - Write JSON definitions with conditions and tasks
2. **Build an Engine** - `Engine::builder().workflow_json(&json).build()` parses, compiles and links every definition; a rejected one fails the build with its issues
3. **Run Messages** - `engine.run(&mut message).await` runs the message through every matching workflow
4. **Read the Result** - The `Run` says how it ended and what each workflow did; the message carries the transformed context, its audit trail and its errors

## Next Steps

- [Installation](./getting-started/installation.md) - Add dataflow-rs to your project
- [Quick Start](./getting-started/quick-start.md) - Build your first workflow
- [Playground](./playground.md) - Experiment with workflows interactively
- [Migrating from 3.x](./migration/from-3.md) - What changed, and what the compat reader rewrites for you
