<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  <h1>Dataflow-rs</h1>

  <strong>A high-performance rules engine for IFTTT-style automation in Rust with zero-overhead JSONLogic evaluation.</strong>

  <br><br>

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![Rust](https://img.shields.io/badge/rust-1.98+-orange.svg)](https://www.rust-lang.org)
  [![Crates.io](https://img.shields.io/crates/v/dataflow-rs.svg)](https://crates.io/crates/dataflow-rs)
</div>

**Dataflow-rs** is a lightweight, embeddable rules engine that lets you define **IF → THEN → THAT** automation in JSON. At build time, the engine parses, validates, and compiles your workflow definitions into an immutable program so runtime execution has zero parsing overhead.

Use it to route events, validate and transform payloads, power dynamic APIs, or build multi-step automation pipelines.

### ⚡ Performance

Dataflow-rs is designed for high-throughput workloads:

*   **Compiled at build time.** The compiler validates workflow definitions, folds constant expressions, and pre-splits field paths before any message is processed.
*   **Single context view per run.** Built-in functions run inline over a single arena view of the message context, while custom host functions write to a transactional per-call overlay.
*   **Single clock read per run.** `metadata.processed_at`, audit entries, and error records share a single timestamp. Additional clock reads happen only when an observer or trace explicitly requests per-task timings.
*   **Zero overhead when unwatched.** When tracing, observers, and error-context mirroring are disabled, execution runs on a streamlined fast path.

See [Performance](./advanced/performance.md) for benchmark results and methodology.

### 🧩 Why Choose dataflow-rs?

Dynamic business rules and user-customizable workflows quickly outgrow hardcoded `if/else` statements, while distributed workflow orchestrators (like Temporal or Zeebe) introduce extra infrastructure and network latency. Dataflow-rs sits cleanly between the two:

| Feature | Hardcoded Rust | dataflow-rs | Orchestrators (Temporal/Zeebe) |
|---|---|---|---|
| **Hot-Reload Rules** | ❌ Recompile & redeploy | ✅ Compile new JSON and swap the program | ❌ Deploy new worker code |
| **Execution Overhead** | None | **Zero (pre-compiled)** | ❌ DB reads/writes (tens of ms) |
| **Browser Execution** | ❌ Large WASM bundle | ✅ Run identical rules via WASM | ❌ Server round-trip required |
| **Visual Debugger** | ❌ Build your own UI | ✅ Included React UI components | ✅ Included Dashboard |
| **Infrastructure** | None | **None (embeddable library)** | ❌ Server clusters & DBs |

## Key Features

- **IF → THEN → THAT Model** — Define workflows with JSONLogic conditions, execute tasks sequentially, and chain workflows in priority order.
- **Parse and Lint in One Pass** — `def::parse` validates workflow JSON and reports issues at their exact JSON paths before compilation.
- **Independent Workflow Compilation** — `Compiler` compiles each workflow individually so you can quarantine invalid definitions while linking valid ones into an immutable `Program`.
- **Complete Run Reports** — `engine.run(&mut message).await` always returns a `Run` describing how execution finished, which workflows matched, and how each exited.
- **Full Context Access** — Conditions and mappings can read any field in `data`, `metadata`, and `temp_data`.
- **Typed Async Host Functions** — Implement `Function` with `#[derive(FunctionConfig)]`. Configs are compiled at build time, and writes are committed to the message only when the call returns `Ok`.
- **Call Layers (Middleware)** — Wrap host function calls at the registry or per-run level for logging, mocking, or dry runs.
- **Deadlines and Cancellation** — Enforce per-run timeouts (`TimedOut`) or cancellation tokens (`Cancelled`) that automatically race against in-flight async host calls.
- **Fan-Out (`for_each`)** — Run a host function across array elements with isolated element bindings and deterministic in-order folding.
- **Task Groups** — Nest tasks under a shared condition, with optional `terminal` groups for early exit (`if / else if / else`).
- **Outcome Policies** — Configure `on_failure` and `on_rejection` on tasks, and `on_failure` on workflows, to control error recovery.
- **Bounded Loops** — Repeat a workflow's task list while a condition holds, up to a configured maximum iteration count.
- **Traffic Splits (Canary Rollouts)** — Route a percentage of a channel's traffic to a workflow version using `rollout` bucket ranges.
- **Lightweight Observers** — Subscribe to execution events via a single `Observer::on(&Event)` callback with configurable `Interest` flags.
- **Replayable Traces** — Capture the initial context and per-step diffs on any run outcome, allowing step-by-step context replay in debuggers.
- **Error Context Mirroring** — Mirror task failure codes into the message context so subsequent conditions can branch on failure causes.
- **Isolated Secrets** — Read sensitive values via `{"secret": "name"}` from an engine-scoped store that never leaks into messages, traces, or logs.
- **Built-in Functions** — `parse_json`, `parse_xml`, `map`, `validation`, `filter`, `log`, `publish_json`, and `publish_xml`.
- **Channel Routing & Lifecycle** — Route runs to specific channels and manage `active`, `paused`, or `archived` workflows.
- **3.x Compatibility** — Automatically loads and rewrites 3.x workflow syntax with `V3_SPELLING` advisories.
- **WebAssembly & React UI** — Run workflows in browsers and Node.js with `@goplasmatic/dataflow-wasm` and visualize/debug them with `@goplasmatic/dataflow-ui`.
- **Audit Trails** — Track every context modification made by each task.

## Try It Now

Run dataflow-rs right in your browser. Edit the workflow or payload below to see the output update live.

> Open the [Full Debugger UI](/dataflow-rs/debugger/) for step-by-step execution and workflow visualization.

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

1. **Define Workflows** — Write JSON workflow definitions with conditions and tasks.
2. **Build an Engine** — `Engine::builder().workflow_json(&json).build()` parses, compiles, and links your definitions, rejecting invalid workflows at startup with detailed issues.
3. **Run Messages** — Call `engine.run(&mut message).await` to process a message through all matching workflows.
4. **Inspect the Result** — Check `Run` for the overall status and per-workflow outcomes, and read the updated context, audit trail, and errors from `Message`.

## Next Steps

- [Installation](./getting-started/installation.md) — Add `dataflow-rs` to your Rust project
- [Quick Start](./getting-started/quick-start.md) — Build and run your first workflow
- [Playground](./playground.md) — Experiment with workflows interactively in your browser
- [Migrating from 3.x](./migration/from-3.md) — Learn what changed in 4.0 and how the compatibility reader helps you upgrade
