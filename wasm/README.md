<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  # @goplasmatic/dataflow-wasm

  **WebAssembly bindings for the dataflow-rs workflow engine**

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![npm](https://img.shields.io/npm/v/@goplasmatic/dataflow-wasm.svg)](https://www.npmjs.com/package/@goplasmatic/dataflow-wasm)
</div>

---

The [dataflow-rs](https://github.com/GoPlasmatic/dataflow-rs) 4.0 rules engine compiled to WebAssembly for browsers and Node.js. `WasmEngine` processes messages through JSON workflow definitions and returns the updated message, a run report, and an optional execution trace. The exact same workflow definitions run unchanged on a Rust backend.

## Features

- **Compiled Rust engine**: Built from the root crate for `wasm32-unknown-unknown` with `datetime` and `ExprEngineBuilder::all_operators` (all optional JSONLogic operator families except `tensor`).
- **Built-in functions**: `parse_json`, `parse_xml`, `map`, `validation`, `filter`, `log`, `publish_json`, `publish_xml`.
- **End-to-end TypeScript types**: `.d.ts` declarations include the engine's wire types (`Message`, `Run`, `Trace`, `Issue`, and related types) generated directly from the Rust crate.
- **Resolves on every status**: Stopped, timed-out, and failed runs resolve normally with `run.status` describing how the run finished.
- **Replayable traces**: Traces record per-step context diffs, and `contexts(trace)` or `TraceReplay` reconstructs the full context after any step.
- **Definition validation**: `check` and `parse` report definition issues at their exact JSON paths before execution.

## Installation

```bash
npm install @goplasmatic/dataflow-wasm
```

## Quick Start

```typescript
import init, { WasmEngine } from '@goplasmatic/dataflow-wasm';

// Initialize the WebAssembly module once before calling any other API.
await init();

const engine = new WasmEngine([
  {
    id: 'greeting',
    tasks: [
      {
        // Copy the incoming payload into `data.input` so JSONLogic expressions can read it.
        id: 'parse',
        function: { name: 'parse_json', input: { source: 'payload', target: 'input' } },
      },
      {
        id: 'greet',
        function: {
          name: 'map',
          input: {
            mappings: [{ path: 'data.greeting', logic: { cat: ['Hello, ', { var: 'data.input.name' }] } }],
          },
        },
      },
    ],
  },
]);

const { message, run } = await engine.run({ name: 'World' });

console.log(run.status);                    // "completed"
// Narrow the `JsonValue` context root to access typed fields under strict TypeScript.
const data = message.context.data as { greeting: string };
console.log(data.greeting);                 // "Hello, World"
```

The `payload` can be any JSON value. If you pass an object, it is stored as an object and `parse_json` copies it into `data`; if you pass a JSON string, `parse_json` parses it first. Note that `payload` is separate from the JSONLogic evaluation context (`{ data, metadata, temp_data }`), so `{"var": "payload.name"}` will not resolve to the payload. If you want to skip the `parse_json` step, pass your initial data directly via `options`: `engine.run(null, { data: { name: 'World' } })`.

## API

### `new WasmEngine(workflows, options?)`

Accepts workflow definitions as a JSON string or an array of workflow objects, plus an optional `options` object that can provide secrets:

```typescript
const engine = new WasmEngine(workflows, {
  secrets: { api_key: 'k-123' },
});
```

Workflows read secrets using the `{"secret": "api_key"}` operator. Secrets are stored on the engine and never enter the message, so they never appear in serialized messages, traces, or error records. If a workflow references a secret that is not in the store, or attempts to write a secret into the message, `new WasmEngine` rejects the definition at build time.

If any workflow definition is invalid, `new WasmEngine` throws an `EngineError` (a JavaScript `Error` with `code: "REJECTED"` and an `issues` array listing every problem at its authored JSON path). Passing a malformed argument throws an `EngineError` with `code: "INVALID_ARGUMENT"`:

```typescript
import type { EngineError } from '@goplasmatic/dataflow-wasm';

try {
  new WasmEngine(workflows);
} catch (e) {
  const err = e as EngineError;
  if (err.code === 'REJECTED') {
    for (const issue of err.issues ?? []) {
      console.error(issue.severity, issue.code, 'at', issue.path, ':', issue.message);
      // "REJECTED UNKNOWN_FUNCTION at tasks[0].function.name : ..."
    }
  }
}
```

Each `Issue` has the shape `{ code, severity, path, step_kind, step_id, field, message }`, where `severity` is `"REJECTED"`, `"DEFECT"`, or `"ADVISORY"`. Advisories do not prevent a workflow from loading, though they are included in `issues` if another issue causes a rejection.

### `engine.run(payload, options?)`

Runs a single message through the engine and resolves with `{ message, run, trace? }` regardless of the run outcome. The promise rejects only when given a malformed argument (`"INVALID_ARGUMENT"`).

| Option | Type | Description |
|---|---|---|
| `id` | `string` | Message ID (defaults to a generated UUID v7). |
| `data` | `JsonValue` | Initial `data` object (ignored if not an object). |
| `metadata` | `JsonValue` | Initial `metadata` object (ignored if not an object). |
| `channel` | `string` | Run only active workflows in this channel (runs all active workflows when omitted). |
| `routing_bucket` | `number` | Canary rollout bucket from `0` to `99` (all rollouts match when omitted). |
| `capture_changes` | `boolean` | Record old/new values for each write in the audit trail (`true` by default). |
| `audit` | `"full" \| "off" \| { last: number }` | How many audit entries to keep on the message (`"full"` by default). |
| `trace` | `boolean \| TraceOptions` | Record an execution trace (`false` by default; `true` uses default `TraceOptions`). |
| `timeout_ms` | `number` | Deadline in milliseconds; if exceeded, the run finishes with status `"timed_out"`. |

`TraceOptions` accepts `{ timings?, changes?, redact?, max_bytes? }`:
- `timings`: Measure each task's duration in `elapsed_us` (`false` by default).
- `changes`: Record per-step changes so contexts can be replayed (`true` by default).
- `redact`: Array of context paths (such as `"metadata.headers"`) whose values are replaced with `null` in the trace while keeping their real values on the message.
- `max_bytes`: Maximum byte budget for trace changes. Once exceeded, subsequent steps omit their `changes` and `trace.truncated` is set to `true`.

The returned `message` has the shape `{ id, payload, context, audit_trail, errors }`, where `context` contains `data`, `metadata`, and `temp_data`. At the start of every run, the engine sets `metadata.processed_at` and `metadata.engine_version`, and updates `metadata.progress` after each executed task.

The returned `run` summarizes how execution finished:

```typescript
interface Run {
  status: 'completed' | 'completed_with_errors' | 'stopped' | 'timed_out' | 'cancelled';
  error?: DataflowError;        // present only when status is "stopped"
  retryable: boolean;           // true for timeouts and retryable stop errors
  partial: boolean;             // true if execution stopped before evaluating all workflows
  matched: number;              // number of workflows that matched and ran
  workflows: WorkflowRecord[];  // per-workflow execution records in priority order
}

interface WorkflowRecord {
  id: string;
  index: number;
  selection: 'ran' | 'skipped_by_condition' | 'condition_failed' | 'skipped_by_rollout';
  error?: DataflowError;        // present only when selection is "condition_failed"
  sweeps: number;               // 1 for normal runs, or the iteration count for a loop
  exit: 'completed' | 'halted' | 'failed' | 'stopped' | null;  // null when not selected
}
```

A status of `completed_with_errors` means the run finished without stopping, but one or more tasks recorded errors on `message.errors` (for example, failed `validation` rules).

### Inspecting a Failed Run

Under the default failure policy, an unhandled task error stops the run. `engine.run` still resolves normally—inspect `run.status`, `run.error`, and `trace` to see what went wrong:

```typescript
const { message, run, trace } = await engine.run(payload, { trace: true });

if (run.status === 'stopped' && trace) {
  console.error(run.error?.code, run.error?.message);

  const failed = trace.steps.find((step) => step.result === 'failed');
  if (failed) {
    console.error('in', failed.workflow_id, failed.task_id, failed.errors);
  }
}

for (const record of run.workflows) {
  console.log(record.id, record.selection, record.exit);
}

// Errors from tasks that continued (such as validation rejections) are on `message.errors`.
console.log(message.errors);
```

### Replaying Execution Traces

A `Trace` has the shape `{ initial, steps, truncated }`, containing the initial context before any workflow ran (with any configured redactions applied) and one step per task execution, skip, failure, or engine write. Rather than storing full message snapshots at every step, each step stores its `changes`, which you can replay to reconstruct the context at any point:

```typescript
import { contexts, mapping_contexts } from '@goplasmatic/dataflow-wasm';

const { trace } = await engine.run(payload, { trace: true });
if (!trace) throw new Error('the run was not traced');

const after = contexts(trace);   // Context[]: the context after each step, in order
after[0];                        // context after step 0
trace.initial;                   // context before step 0
after.at(-1);                    // final context (matches message.context unless trace.truncated)

for (const [i, step] of trace.steps.entries()) {
  // result: "executed" | "skipped" | "failed"; task_id is null for workflow-level steps
  console.log(step.workflow_id, step.task_id, step.result, step.status, step.errors ?? []);
  if (step.mapping_ends) {
    // For a `map` step, reconstruct the context after each individual mapping:
    const perMapping = mapping_contexts(trace, i);
  }
}
```

Each `TraceStep` has the shape `{ workflow, workflow_id, task, task_id, result, status, changes, mapping_ends?, errors?, elapsed_us?, loop_counter?, element_index? }`. `errors` contains any error records added during that step, `loop_counter` identifies the loop iteration, and `element_index` identifies the `for_each` element. Traces can be serialized, stored, and replayed later; `contexts` and `mapping_contexts` validate the trace structure and throw `code: "INVALID_TRACE"` if it is malformed.

For long traces or interactive UIs, use `TraceReplay` to reconstruct only the specific step you are viewing:

```typescript
import { TraceReplay } from '@goplasmatic/dataflow-wasm';

const replay = new TraceReplay(trace); // parses and validates the trace once
try {
  replay.context_at(-1);              // initial context
  replay.context_at(0);               // context after step 0
  replay.mapping_context_at(0, 0);    // context after step 0's first mapping (or null)
} finally {
  replay.free();
}
```

`TraceReplay` maintains a single working context in WASM memory: seeking forward applies only the intervening diffs, while seeking backward replays from `initial`. Each call returns a fresh JavaScript object so mutating the returned value never corrupts the replay state.

### `engine.check(definition)`

Validates a single workflow definition against this engine's registered functions and secrets, returning all issues (including advisories) at their authored JSON paths. An empty array means the definition is valid:

```typescript
const issues = engine.check({ id: 'draft', tasks: [{ id: 't', function: { name: 'nope', input: {} } }] });
// [{ code: "UNKNOWN_FUNCTION", severity: "REJECTED", path: "tasks[0].function.name", ... }]
```

### `engine.workflow_count()`, `engine.workflow_ids()`, `engine.free()`

- `workflow_count()`: Returns the number of loaded workflows.
- `workflow_ids()`: Returns the loaded workflow IDs in priority execution order as a `string[]`.
- `free()`: Releases the engine's WebAssembly memory.

### Module Functions

```typescript
import init, { engine_version, engine_facts, parse, contexts, mapping_contexts, TraceReplay } from '@goplasmatic/dataflow-wasm';

await init();              // default export: initialize the WASM module once

engine_version();          // "4.0.0": version of the compiled Rust engine
engine_facts();            // { version, builtin_functions, max_group_depth,
                           //   loop_defaults: { init, increment },
                           //   for_each_defaults: { max_concurrency },
                           //   definition_keys: { workflow, task, group, loop, for_each, rollout, function } }

parse(definition);         // { definition, issues }: normalized 4.0 definition (or null if rejected)
                           // along with all issues at their authored paths
```

- `parse(definition)` parses and normalizes a workflow definition. Legacy 3.x keys (such as `continue_on_error: true` or `validate`) are automatically rewritten to their 4.0 equivalents (`on_failure: "continue"`, `validation`) and reported as `V3_SPELLING` advisories. Unrecognized keys are moved into the node's `ext` map and reported.
- `engine_facts()` returns the engine's built-in function names, limits, defaults, and recognized schema keys so editor and UI tools do not need to hardcode them.
- `engine_version()` returns the semantic version of the loaded WASM module so frontend packages can verify compatibility.

### Types

`dataflow_wasm.d.ts` exports all wire types generated from the Rust crate (`Message`, `Context`, `Change`, `AuditEntry`, `ErrorRecord`, `Trace`, `TraceStep`, `StepResult`, `Run`, `RunStatus`, `WorkflowRecord`, `Selection`, `WorkflowExit`, `DataflowError`, `Issue`, `Severity`, `JsonValue`) alongside the WASM wrapper types (`WasmEngine`, `TraceReplay`, `EngineOptions`, `RunOptions`, `TraceOptions`, `AuditMode`, `RunResult`, `EngineError`, `EngineFacts`, `ParseResult`).

## Migrating from 3.x

| 3.x | 4.0 |
|---|---|
| `new WasmEngine(workflowsJson)` | Unchanged (now also accepts an array of workflow objects) |
| `WasmEngine.with_secrets(workflowsJson, secretsJson)` | `new WasmEngine(workflows, { secrets })` |
| `engine.process(payload)` returning a JSON string | `engine.run(payload)` resolving with `{ message, run }` objects |
| `engine.process_with_trace(payload)` | `engine.run(payload, { trace: true })` (`result.trace`) |
| `process_message(workflowsJson, payload)` | Construct a `WasmEngine` and call `engine.run(payload)` |
| `engine.workflow_ids()` returning a JSON string | Returns `string[]` |
| Payload coerced to a string | Accepts any JSON value; strings remain strings |
| Failed runs rejected the promise with a string | Promise resolves; check `run.status` and `run.error` |
| Invalid definitions threw a string | Throws an `EngineError` with `code` and `issues` |
| Per-step `message` snapshots in traces | Reconstruct via `contexts(trace)` and `mapping_contexts(trace, step)` |
| Trace step `duration_us` | `elapsed_us` when run with `trace: { timings: true }` |
| `message.errors` as the only error channel | `message.errors` holds all error records; `run.status` indicates if the run stopped |

Existing 3.x workflow JSON loads automatically: `continue_on_error`, `halt_on`, and `validate` are rewritten to `on_failure`, `on_rejection`, and `validation` with `V3_SPELLING` advisories. Call `parse(definition)` to obtain the migrated 4.0 JSON.

## Building from Source

Requirements:
- Rust 1.98+
- `wasm-pack`

```bash
cd wasm
wasm-pack test --node                          # run tests on wasm32 under Node.js
wasm-pack build --target web --out-dir pkg     # build the package into wasm/pkg/
node scripts/verify-wasm.mjs pkg               # verify exports and TypeScript definitions
```

## Related Packages

- [dataflow-rs](https://crates.io/crates/dataflow-rs) — Core Rust workflow engine
- [@goplasmatic/dataflow-ui](https://www.npmjs.com/package/@goplasmatic/dataflow-ui) — React visualization and debugging components

## License

This project is licensed under the Apache License, Version 2.0. See the [LICENSE](../LICENSE) file for details.
