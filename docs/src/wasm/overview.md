# WebAssembly Package

The `@goplasmatic/dataflow-wasm` package is the `dataflow-rs` 4.0 rules engine compiled to WebAssembly (`wasm32-unknown-unknown`). It provides a `WasmEngine` class that runs messages through JSON workflow definitions in browsers and Node.js using the exact same engine and wire types as a Rust backend.

## Installation

```bash
npm install @goplasmatic/dataflow-wasm
```

## Quick Start

```typescript
import init, { WasmEngine } from '@goplasmatic/dataflow-wasm';

// Initialize the WebAssembly module once before calling any other function.
await init();

const engine = new WasmEngine([
  {
    id: 'greeting',
    tasks: [
      {
        // Copy `payload` into `data.input` so JSONLogic expressions can access it.
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
// Narrow the `JsonValue` context root to read typed properties under strict TypeScript.
const data = message.context.data as { greeting: string };
console.log(data.greeting);                 // "Hello, World"
```

Data crosses the JS/WASM boundary as native JavaScript values rather than serialized JSON strings: you can pass workflow definitions as an array of objects (or a JSON string), pass any JSON value as the `payload`, and receive `{ message, run, trace? }` as typed JavaScript objects.

Two important notes about `payload`:
- **Objects stay objects; strings stay strings.** A `parse_json` task reading `payload` copies an object directly and parses a JSON string, so the same workflow handles both seamlessly.
- **`payload` is separate from the JSONLogic evaluation context.** Conditions and mappings evaluate against `{ data, metadata, temp_data }`, so `{"var": "payload.x"}` always resolves to `null`. Either copy the payload into `data` with `parse_json`, or seed `data` directly via `RunOptions`: `engine.run(null, { data: { name: 'World' } })`.

## API Reference

### `new WasmEngine(workflows, options?)`

```typescript
class WasmEngine {
  /**
   * Compiles workflow definitions (passed as a JSON string or an array of objects).
   * `options.secrets`: optional secret store read via `{"secret": "name"}`.
   * Throws an `EngineError` with `code: "REJECTED"` and `issues` when any
   * definition is invalid, or `code: "INVALID_ARGUMENT"` when an argument is malformed.
   */
  constructor(workflows: string | JsonValue[], options?: EngineOptions);

  /** Runs a message through the engine; resolves on every run status with `{ message, run, trace? }`. */
  run(payload: JsonValue | undefined, options?: RunOptions): Promise<RunResult>;

  /** Validates a single workflow definition against this engine and returns all issues. */
  check(definition: JsonValue): Issue[];

  /** Returns the number of compiled workflows in the engine. */
  workflow_count(): number;

  /** Returns the IDs of all compiled workflows in priority execution order. */
  workflow_ids(): string[];

  /** Frees the WebAssembly memory held by this engine instance. */
  free(): void;
}
```

#### Configuring Secrets

```typescript
const engine = new WasmEngine(workflows, {
  secrets: { api_key: 'k-123' },
});
```

Workflows read secrets using `{"secret": "api_key"}`. Secret values are held inside the engine and never enter the `Message`, so they cannot leak into serialized messages, traces, or error records. Referencing an unknown secret name or attempting to write a secret into the message causes `new WasmEngine` to reject the definition at build time. See [Secrets](../advanced/secrets.md) for details.

#### Handling Rejected Definitions

When a workflow definition fails validation, `new WasmEngine` throws an `EngineError`—a JavaScript `Error` with `code: "REJECTED"` and an `issues` array containing every problem at its exact JSON path:

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

Each `Issue` has the shape `{ code, severity, path, step_kind, step_id, field, message }`, where `severity` is `"REJECTED"`, `"DEFECT"`, or `"ADVISORY"` and `path` is the authored JSON path (such as `"tasks[1].tasks[0].id"`). Advisories do not prevent a definition from loading on their own, though they are included in `issues` when another issue rejects the definition. See [Authoring-Time Validation](../advanced/authoring-validation.md) for all issue codes.

### `engine.run(payload, options?)`

Runs a message through the engine and resolves with `{ message, run, trace? }` on every run status. The promise rejects only if passed a malformed argument (`"INVALID_ARGUMENT"`); runs that stop on a task failure or hit a timeout still resolve normally with `run.status` describing the outcome.

```typescript
interface RunOptions {
  id?: string;                   // Message ID (defaults to a generated UUID v7)
  data?: JsonValue;              // Initial `data` root object (ignored if not an object)
  metadata?: JsonValue;          // Initial `metadata` root object (ignored if not an object)
  channel?: string;              // Run only active workflows in this channel (runs all active workflows when omitted)
  routing_bucket?: number;       // Canary rollout bucket from 0 to 99 (all rollouts match when omitted)
  capture_changes?: boolean;     // Record old/new values in audit trail entries (true by default)
  audit?: 'full' | 'off' | { last: number };  // Audit retention policy ("full" by default)
  trace?: boolean | TraceOptions;             // Capture an execution trace (false by default)
  timeout_ms?: number;           // Execution deadline in milliseconds; ends the run as "timed_out" if exceeded
}

interface TraceOptions {
  timings?: boolean;    // Record per-step execution time in `elapsed_us` (false by default)
  changes?: boolean;    // Record per-step context diffs so the trace can be replayed (true by default)
  redact?: string[];    // Context paths whose values are replaced with `null` in the trace
  max_bytes?: number;   // Maximum byte budget for trace changes (sets `trace.truncated` when exceeded)
}

interface RunResult {
  message: Message;     // The message after execution
  run: Run;             // Execution status and per-workflow summary
  trace?: Trace;        // Execution trace (present when `trace` was enabled)
}
```

`timeout_ms` enforces a per-run deadline checked at every loop iteration and every 64th expression operation. If exceeded, the run finishes with `status: "timed_out"`, `retryable: true`, `partial: true`, and the partial message and trace up to that point.

#### `Message` Shape

```typescript
interface Message {
  id: string;
  payload: JsonValue;             // The input payload passed to `run`, unchanged
  context: {
    data: JsonValue;
    metadata: JsonValue;
    temp_data: JsonValue;
  };
  audit_trail: AuditEntry[];      // One entry per executed task (subject to `audit` retention)
  errors: ErrorRecord[];          // All error records added during the run
}
```

Note that `data`, `metadata`, and `temp_data` are nested under `message.context`. At the start of every run, the engine stamps `metadata.processed_at` and `metadata.engine_version` (plus `metadata.channel` when a channel is specified), and updates `metadata.progress` (`{ workflow_id, task_id, status_code }`) after each task.

#### `Run` Shape

```typescript
interface Run {
  status: 'completed' | 'completed_with_errors' | 'stopped' | 'timed_out' | 'cancelled';
  error?: DataflowError;        // Present only when status is "stopped"
  retryable: boolean;           // True for timeouts and retryable stop errors
  partial: boolean;             // True if the run ended before evaluating all workflows
  matched: number;              // Number of workflows that matched and ran
  workflows: WorkflowRecord[];  // Per-workflow execution records in priority order
}

interface WorkflowRecord {
  id: string;
  index: number;
  selection: 'ran' | 'skipped_by_condition' | 'condition_failed' | 'skipped_by_rollout';
  error?: DataflowError;        // Present only when selection is "condition_failed"
  sweeps: number;               // 1 for normal workflows, or the iteration count for a loop
  exit: 'completed' | 'halted' | 'failed' | 'stopped' | null;  // Null when not selected
}

interface DataflowError {
  code: string;
  message: string;
  retryable: boolean;
}
```

- `completed_with_errors`: Every matching workflow finished, and one or more tasks recorded errors on `message.errors` (such as failed `validation` rules or tasks configured with `on_failure: "continue"`).
- `stopped`: An unhandled task failure stopped the run early (`run.error` contains the stopping error).

#### Inspecting a Failed Run

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

// Errors from tasks that continued (e.g., validation rejections) are on `message.errors`.
console.log(message.errors);
```

### `engine.check(definition)`

Parses and validates a single workflow definition against this engine's functions and secrets, returning all issues (including advisories) at their authored paths. An empty array means the definition is valid and ready to load:

```typescript
const issues = engine.check({
  id: 'draft',
  tasks: [{ id: 't', function: { name: 'nope', input: {} } }],
});
// [{ code: "UNKNOWN_FUNCTION", severity: "REJECTED", path: "tasks[0].function.name", ... }]
```

### Module-Level Functions

```typescript
import init, {
  engine_version,
  engine_facts,
  parse,
  contexts,
  mapping_contexts,
  TraceReplay,
} from '@goplasmatic/dataflow-wasm';

/** Initializes the WASM module. Call once before any other API. */
await init();

/** Returns the semantic version of the compiled engine (e.g., "4.0.0"). */
engine_version(): string;

/** Returns engine metadata so UIs and linters do not need to hardcode schema constants. */
engine_facts(): {
  version: string;
  builtin_functions: string[];
  max_group_depth: number;
  loop_defaults: { init: number; increment: number };
  for_each_defaults: { max_concurrency: number };
  definition_keys: {
    workflow: string[];
    task: string[];
    group: string[];
    loop: string[];
    for_each: string[];
    rollout: string[];
    function: string[];
  };
};

/** Parses and normalizes a definition into 4.0 format and returns all parse issues. */
parse(definition: JsonValue): { definition: JsonValue | null; issues: Issue[] };

/** Reconstructs the context after each step of a trace. */
contexts(trace: Trace): Context[];

/** Reconstructs the context after each individual mapping of a `map` step. */
mapping_contexts(trace: Trace, step: number): Context[];
```

- `parse(definition)` rewrites legacy 3.x keys (`continue_on_error: true` → `on_failure: "continue"`, `halt_on: "failure"` → `on_rejection: "halt"`, `validate` → `validation`) and reports a `V3_SPELLING` advisory at each 3.x key. Unrecognized keys are moved into `ext` and reported. If the definition is rejected, `definition` is `null`.
- Compare `engine_version()` against the version your frontend expects so you fail early if an outdated WASM binary is loaded.

## Execution Traces and Replay

A `Trace` has the shape `{ initial, steps, truncated }`: the initial context before any workflow ran (with any configured `redact` paths replaced by `null`), an ordered list of `TraceStep` records, and a boolean `truncated` flag indicating whether `max_bytes` was exceeded. Rather than storing full message snapshots at every step, each step stores its `changes`:

```typescript
interface TraceStep {
  workflow: number;               // Workflow index in program order
  workflow_id: string;
  task: number | null;            // Null for workflow-level steps (skips, condition failures, engine writes)
  task_id: string | null;
  result: 'executed' | 'skipped' | 'failed';
  status: number | null;          // HTTP-style status code (null for skips and engine steps)
  changes: Change[];              // Ordered context diffs produced by this step
  mapping_ends?: number[];        // For `map` steps: end index in `changes` for each mapping
  errors?: ErrorRecord[];         // Error records added during this step
  elapsed_us?: number;            // Step duration in microseconds (when `timings: true`)
  loop_counter?: number;          // Loop iteration counter (inside a loop)
  element_index?: number;         // Fan-out element index (inside a `for_each` task)
}
```

### Replaying Contexts

Use `contexts` and `mapping_contexts` (or `TraceReplay`) to reconstruct the context at any step:

```typescript
import { contexts, mapping_contexts } from '@goplasmatic/dataflow-wasm';

const { trace } = await engine.run(payload, { trace: true });
if (!trace) throw new Error('the run was not traced');

const after = contexts(trace);   // One Context per step, in order
trace.initial;                   // Context before step 0
after[0];                        // Context after step 0
after.at(-1);                    // Final context (matches message.context unless trace.truncated)

for (const [i, step] of trace.steps.entries()) {
  if (step.mapping_ends) {
    // Reconstruct the context after each individual mapping in a `map` step:
    const perMapping = mapping_contexts(trace, i);
  }
}
```

`contexts` and `mapping_contexts` validate the trace structure before replaying and throw `code: "INVALID_TRACE"` if a stored trace is malformed.

For long traces or interactive step debuggers, use `TraceReplay` to retain a single working context in WASM memory instead of materializing an array of snapshots for every step:

```typescript
import { TraceReplay } from '@goplasmatic/dataflow-wasm';

const replay = new TraceReplay(trace);
try {
  const before = replay.context_at(-1); // initial context
  const after = replay.context_at(0);   // context after step 0
  const mapping = replay.mapping_context_at(0, 0); // after step 0's first mapping (or null)
} finally {
  replay.free();
}
```

Seeking forward applies only the intervening step diffs, while seeking backward replays from `trace.initial`. Each call returns an independent JavaScript object.

## TypeScript Types

`dataflow_wasm.d.ts` provides end-to-end TypeScript definitions for all wire types generated from the Rust crate (`Message`, `Context`, `Change`, `AuditEntry`, `ErrorRecord`, `Trace`, `TraceStep`, `StepResult`, `Run`, `RunStatus`, `WorkflowRecord`, `Selection`, `WorkflowExit`, `DataflowError`, `Issue`, `Severity`, `JsonValue`) as well as the WASM package's own types (`WasmEngine`, `TraceReplay`, `EngineOptions`, `RunOptions`, `TraceOptions`, `AuditMode`, `RunResult`, `EngineError`, `EngineFacts`, `ParseResult`). Because a Rust backend serializes the exact same wire formats, you can use these types for server responses as well.

## Migrating from 3.x

| 3.x | 4.0 |
|---|---|
| `new WasmEngine(workflowsJson)` | Unchanged (also accepts an array of workflow objects) |
| `WasmEngine.with_secrets(workflowsJson, secretsJson)` | `new WasmEngine(workflows, { secrets })` |
| `engine.process(payload)` resolving to a JSON string | `engine.run(payload)` resolving to `{ message, run }` objects |
| `engine.process_with_trace(payload)` | `engine.run(payload, { trace: true })` (`result.trace`) |
| `process_message(workflowsJson, payload)` | Construct a `WasmEngine` and call `engine.run(payload)` |
| `engine.workflow_ids()` returning a JSON string | Returns `string[]` |
| Payload coerced to a string | Accepts any JSON value; strings stay strings |
| Failed runs rejected the promise with a string | Promise resolves; inspect `run.status` and `run.error` |
| Invalid definitions threw a string | Throws an `EngineError` with `code` and `issues` |
| Per-step `message` snapshots in traces | Reconstruct via `contexts(trace)` and `mapping_contexts(trace, step)` |
| Trace step `duration_us` and `started_at` | `elapsed_us` when run with `trace: { timings: true }` |
| `trace.truncated` omitted when `false` | Always present as a boolean |

## Enabled JSONLogic Operators

`@goplasmatic/dataflow-wasm` is compiled with the `datetime` feature and `ExprEngineBuilder::all_operators()`, enabling `ExtString`, `ExtArray`, `ExtObject`, `ExtMath`, `ExtControl`, `ErrorHandling`, and `Datetime` out of the box (`Tensor` is excluded because operator names like `shape`, `cast`, and `concat` collide with common JSON keys).

In Rust, `ExprEngine::default()` enables only core JSONLogic operators by default. If you share workflows between `@goplasmatic/dataflow-wasm` and a Rust backend, enable the same operator families on `ExprEngineBuilder` in your Rust service so expressions evaluate identically in both environments. See [JSONLogic → Operator Families](../advanced/jsonlogic.md#operator-families).

## Building from Source

Requirements:
- Rust 1.98+
- `wasm-pack`

```bash
cd wasm
wasm-pack test --node                          # run tests on wasm32 under Node.js
wasm-pack build --target web --out-dir pkg     # build package into wasm/pkg/
node scripts/verify-wasm.mjs pkg               # verify WASM features and .d.ts exports
```

## Browser Compatibility

The published WebAssembly binary is compiled with WebAssembly reference types, bulk memory, non-trapping float-to-int, and sign-extension instructions enabled:

- Chrome / Edge 96+
- Firefox 79+
- Safari 15+

## Next Steps

- [UI Package](../ui/overview.md) — React visualization and step-by-step debugging components
- [Built-in Functions](../built-in-functions/overview.md) — Reference for `map`, `validation`, and other built-ins
