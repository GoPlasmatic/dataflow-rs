<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  # @goplasmatic/dataflow-wasm

  **WebAssembly bindings for dataflow-rs workflow engine**

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![npm](https://img.shields.io/npm/v/@goplasmatic/dataflow-wasm.svg)](https://www.npmjs.com/package/@goplasmatic/dataflow-wasm)
</div>

---

The [dataflow-rs](https://github.com/GoPlasmatic/dataflow-rs) 4.0 engine compiled to WebAssembly, for the browser and Node. One class, `WasmEngine`, runs messages through a set of workflow definitions and resolves with the message, a record of the run and, when asked, a trace. The same definitions run on a Rust backend unchanged.

## Features

- **The engine, not a port**: the Rust crate built for `wasm32-unknown-unknown` with `datetime` and `ExprEngineBuilder::all_operators`, every optional operator family except `tensor`
- **Built-in functions**: `parse_json`, `parse_xml`, `map`, `validation`, `filter`, `log`, `publish_json`, `publish_xml`
- **Typed end to end**: the package's `.d.ts` carries the engine's wire types (`Message`, `Run`, `Trace`, `Issue` and the rest), generated from the crate
- **A result on every status**: a run that stops, times out or fails resolves, with `run.status` saying how
- **Traces that replay**: a trace records each step's changes; `contexts(trace)` rebuilds the context after every step
- **Definition tools**: `check` and `parse` report a definition's issues at their authored paths before anything runs

## Installation

```bash
npm install @goplasmatic/dataflow-wasm
```

## Quick Start

```typescript
import init, { WasmEngine } from '@goplasmatic/dataflow-wasm';

// Instantiate the module once, before any other call.
await init();

const engine = new WasmEngine([
  {
    id: 'greeting',
    tasks: [
      {
        // The payload is not part of the evaluation context: copy it into `data` first.
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
// A context root is `JsonValue`: narrow it to read a key under strict TS.
const data = message.context.data as { greeting: string };
console.log(data.greeting);                 // "Hello, World"
```

The payload is any JSON value. An object is stored as the object it is, and a `parse_json` task reading `payload` copies it; a string stays a string, and `parse_json` parses it. Either way the payload is not in the JSONLogic evaluation context, which is `{data, metadata, temp_data}` only: `{"var": "payload.name"}` resolves to nothing. To skip the parse step, seed `data` directly: `engine.run(null, { data: { name: 'World' } })`.

## API

### `new WasmEngine(workflows, options?)`

Takes the definitions as JSON text or as an array, and `options` with an optional secret store:

```typescript
const engine = new WasmEngine(workflows, {
  secrets: { api_key: 'k-123' },
});
```

Workflows read a secret through `{"secret": "api_key"}`. The values are held by the engine and never by a message: nothing of the store appears in a message, a trace or an error. A workflow that reads a name the store does not have is refused when the engine is built.

A definition the engine refuses throws an `EngineError`, a JS `Error` with `code: "REJECTED"` and `issues`, every issue the definitions have, each at its authored path. A malformed argument throws `code: "INVALID_ARGUMENT"`:

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

An `Issue` is `{ code, severity, path, step_kind, step_id, field, message }`, with `severity` one of `"REJECTED"`, `"DEFECT"` or `"ADVISORY"`. Advisories do not stop a definition from loading; they are reported with the rest when another issue does.

### `engine.run(payload, options?)`

Runs one message and resolves with `{ message, run, trace? }` on every status. The promise rejects only for a malformed argument (`"INVALID_ARGUMENT"`).

| Option | Type | What it does |
|---|---|---|
| `id` | `string` | The message id. A UUID v7 unless set. |
| `data` | `JsonValue` | The `data` root the message starts with. An object, else ignored. |
| `metadata` | `JsonValue` | The `metadata` root the message starts with. An object, else ignored. |
| `channel` | `string` | The channel whose workflows run. Every active workflow when unset. |
| `routing_bucket` | `number` | The rollout bucket, 0 to 255. Every rollout admits an unset one. |
| `capture_changes` | `boolean` | Record each task's changes in the audit trail. On by default. |
| `audit` | `"full" \| "off" \| { last: number }` | Which audit entries the message keeps. `"full"` by default. |
| `trace` | `boolean \| TraceOptions` | Trace the run. Off by default. `true` takes the default options. |
| `timeout_ms` | `number` | A deadline from the call. The run then ends `"timed_out"` with its partial message and trace. |

`TraceOptions` is `{ timings?, changes?, redact?, max_bytes? }`: `timings` times each call into its step's `elapsed_us` (off by default); `changes` records each step's changes so the trace replays (on by default); `redact` names context paths whose values read `null` in the trace, such as `"metadata.headers"`, while the message keeps the real values; `max_bytes` caps what the trace holds, and the step that would cross it, and every later one, keeps everything but its changes, with `trace.truncated` set.

`message` is `{ id, payload, context, audit_trail, errors }`, with `context` holding `data`, `metadata` and `temp_data`. The engine stamps `metadata.processed_at` and `metadata.engine_version` at the start of every run and writes `metadata.progress` after every task.

`run` is how the run ended:

```typescript
interface Run {
  status: 'completed' | 'completed_with_errors' | 'stopped' | 'timed_out' | 'cancelled';
  error?: DataflowError;        // present only when status is "stopped"
  retryable: boolean;           // true for a timeout, and for a stop whose error says so
  partial: boolean;             // did the run end before every workflow was considered?
  matched: number;              // how many workflows were selected and ran
  workflows: WorkflowRecord[];  // every workflow considered, in program order
}

interface WorkflowRecord {
  id: string;
  index: number;
  selection: 'ran' | 'skipped_by_condition' | 'condition_failed' | 'skipped_by_rollout';
  error?: DataflowError;        // present only when selection is "condition_failed"
  sweeps: number;               // one for a workflow that ran once; one per sweep for a loop
  exit: 'completed' | 'halted' | 'failed' | 'stopped' | null;  // null when it did not run
}
```

`completed_with_errors` is a run that was not stopped but recorded errors on the message, such as a `validation` task's failed rules.

### Reading a failing run

A task failure under the default policies stops the run. The promise still resolves; the status and the trace say what happened:

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

// Errors tolerated by a policy are on the message, not the run.
console.log(message.errors);
```

### The trace and its replay

A trace is `{ initial, steps, truncated }`: the context before any workflow, redacted as asked, and one step per call, skip, failure and engine write. A step carries no message snapshot; it carries its `changes`, and the package replays them:

```typescript
import { contexts, mapping_contexts } from '@goplasmatic/dataflow-wasm';

const { trace } = await engine.run(payload, { trace: true });
if (!trace) throw new Error('the run was not traced'); // `trace` is optional on RunResult

const after = contexts(trace);   // Context[]: the context after each step, in order
after[0];                        // after the first step
trace.initial;                   // before it
after.at(-1);                    // the message's context after the run, unless `trace.truncated`

for (const [i, step] of trace.steps.entries()) {
  // result: "executed" | "skipped" | "failed"; task_id is null for a workflow's own step
  console.log(step.workflow_id, step.task_id, step.result, step.status, step.errors ?? []);
  if (step.mapping_ends) {
    // A `map` step: the context after each of its mappings, in order.
    const perMapping = mapping_contexts(trace, i);
  }
}
```

A step is `{ workflow, workflow_id, task, task_id, result, status, changes, mapping_ends?, errors?, elapsed_us?, loop_counter?, element_index? }`. `errors` holds every record the step added to the message; `loop_counter` tells a loop's sweeps apart; `element_index` a fan-out's elements. A trace can be stored and replayed later: `contexts` and `mapping_contexts` read it back and check it before indexing, and refuse a malformed one with `code: "INVALID_TRACE"`.

For long traces, use `TraceReplay` to read only the context being viewed:

```typescript
import { TraceReplay } from '@goplasmatic/dataflow-wasm';

const replay = new TraceReplay(trace); // reads and validates once
try {
  replay.context_at(-1);              // initial context
  replay.context_at(0);               // after step 0
  replay.mapping_context_at(0, 0);    // after its first mapping, or null
} finally {
  replay.free();
}
```

The cursor keeps one working context. Forward seeks apply intervening
changes; backward seeks restart from `initial`. It returns a new JS object
for each requested context, so editing that object cannot alter the replay.
The debugger uses this API instead of retaining every step's snapshot.

### `engine.check(definition)`

The issues one definition has, parsed and checked against this engine's functions, each at its authored path. An empty array means it would load:

```typescript
const issues = engine.check({ id: 'draft', tasks: [{ id: 't', function: { name: 'nope', input: {} } }] });
// [{ code: "UNKNOWN_FUNCTION", severity: "REJECTED", path: "tasks[0].function.name", ... }]
```

### `engine.workflow_count()`, `engine.workflow_ids()`, `engine.free()`

How many workflows the engine holds; their ids, in program order, as a `string[]`; and the release of the engine's WebAssembly memory once it is no longer used.

### Module functions

```typescript
import init, { engine_version, engine_facts, parse, contexts, mapping_contexts, TraceReplay } from '@goplasmatic/dataflow-wasm';

await init();              // the default export: instantiate the module, once

engine_version();          // "4.0.0": the engine compiled into this module
engine_facts();            // { version, builtin_functions, max_group_depth,
                           //   loop_defaults: { init, increment },
                           //   for_each_defaults: { max_concurrency },
                           //   definition_keys: { workflow, task, group, loop, for_each, rollout, function } }

parse(definition);         // { definition, issues }: the definition in the 4.0 spelling,
                           // null when it was rejected, and every issue at its authored path
```

`parse` reads a definition as the engine does. A 3.x spelling is rewritten (`continue_on_error: true` becomes `on_failure: "continue"`, `validate` becomes `validation`) and reported as a `V3_SPELLING` advisory at the 3.x key; a key the engine does not know is moved into the node's `ext` and reported. `engine_facts()` gives a tool the engine's built-in names, defaults and known definition keys so it need not keep a copy.

Pair `engine_version()` with the version your frontend was built against. An older engine reads a key it predates as an unknown key, kept in `ext` and reported, so a workflow using a newer feature loads and does something other than what it says.

### Types

The package's `dataflow_wasm.d.ts` declares the engine's wire types, generated from the crate (`Message`, `Context`, `Change`, `AuditEntry`, `ErrorRecord`, `Trace`, `TraceStep`, `StepResult`, `Run`, `RunStatus`, `WorkflowRecord`, `Selection`, `WorkflowExit`, `DataflowError`, `Issue`, `Severity`, `JsonValue`), and the package's own (`WasmEngine`, `TraceReplay`, `EngineOptions`, `RunOptions`, `TraceOptions`, `AuditMode`, `RunResult`, `EngineError`, `EngineFacts`, `ParseResult`). Every export is typed with them, so the result of `run` needs no cast.

## From 3.x

| 3.x | 4.0 |
|---|---|
| `new WasmEngine(workflowsJson)` | Unchanged; an array of definitions is accepted too |
| `WasmEngine.with_secrets(workflowsJson, secretsJson)` | `new WasmEngine(workflows, { secrets })` |
| `engine.process(payload)`, a JSON string | `engine.run(payload)`, resolving with `{ message, run }` as objects |
| `engine.process_with_trace(payload)` | `engine.run(payload, { trace: true })`; the trace is `result.trace` |
| `process_message(workflowsJson, payload)` | Build an engine and call `run` |
| `engine.workflow_ids()`, a JSON string | A `string[]` |
| The payload forced to a string | Any value; a string stays a string |
| A failing run rejected the promise with a string | Resolves; read `run.status` and `run.error` |
| A refused definition threw a string | Throws an `EngineError` with `code` and `issues` |
| A trace step's `message` snapshot and `mapping_contexts` | `contexts(trace)` and `mapping_contexts(trace, step)` |
| A trace step's `duration_us` | `elapsed_us`, with `trace: { timings: true }` |
| `message.errors` as the one error channel | Still on the message; `run.status` and `run.error` say whether the run stopped |

Definitions written for 3.x load as they are: `continue_on_error`, `halt_on` and `validate` are rewritten to `on_failure`, `on_rejection` and `validation` and reported as `V3_SPELLING` advisories. `parse(definition)` shows the rewritten form.

## Building from Source

Requirements:
- Rust 1.98+
- wasm-pack

```bash
cd wasm
wasm-pack test --node                          # the package's tests, on wasm32 under Node
wasm-pack build --target web --out-dir pkg     # the package, in wasm/pkg/
node scripts/verify-wasm.mjs pkg               # the binary's features and the .d.ts's types
```

## Related Packages

- [dataflow-rs](https://crates.io/crates/dataflow-rs) - Core Rust workflow engine
- [@goplasmatic/dataflow-ui](https://www.npmjs.com/package/@goplasmatic/dataflow-ui) - React visualization library

## License

This project is licensed under the Apache License, Version 2.0. See the [LICENSE](../LICENSE) file for details.
