# WebAssembly Package

The `@goplasmatic/dataflow-wasm` package is the dataflow-rs 4.0 engine
compiled to WebAssembly: the same crate a Rust backend runs, built for
`wasm32-unknown-unknown`, with one class, `WasmEngine`, that runs messages
through a set of workflow definitions.

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

JSON crosses the boundary as values, not text: the definitions may be an
array (JSON text is accepted too), the payload is any JSON value, and the
result is an object. Two things to know about the payload:

- **An object is stored as the object; a string stays a string.** A
  `parse_json` task reading `payload` copies an object and parses a string,
  so a workflow written for a string payload reads an object one unchanged.
- **`payload` is not in the JSONLogic evaluation context.** Conditions and
  mappings see `{data, metadata, temp_data}` only, so `{"var": "payload.x"}`
  never resolves. Parse into `data` and read from there, or seed `data`
  directly: `engine.run(null, { data: { name: 'World' } })`.

## API Reference

### `new WasmEngine(workflows, options?)`

```typescript
class WasmEngine {
  /**
   * `workflows`: the definitions, as JSON text or an array.
   * `options.secrets`: a store the workflows read through {"secret": "name"}.
   * Throws an EngineError: "REJECTED" with `issues` when a definition does not
   * load, "INVALID_ARGUMENT" when an argument is not what this takes.
   */
  constructor(workflows: string | JsonValue[], options?: EngineOptions);

  /** Runs one message; resolves on every status with { message, run, trace? }. */
  run(payload: JsonValue | undefined, options?: RunOptions): Promise<RunResult>;

  /** Every issue a definition has, checked against this engine's functions. */
  check(definition: JsonValue): Issue[];

  /** How many workflows the engine holds. */
  workflow_count(): number;

  /** The workflows' ids, in program order. */
  workflow_ids(): string[];

  /** Release the WebAssembly memory held by this engine. */
  free(): void;
}
```

#### Secrets

```typescript
const engine = new WasmEngine(workflows, {
  secrets: { api_key: 'k-123' },
});
```

Workflows read a secret through `{"secret": "api_key"}`. The values are held
by the engine, never by a message: nothing of the store appears in a message,
a trace or an error this engine returns. A workflow that reads a name the store
does not have is refused when the engine is built. See
[Secrets](../advanced/secrets.md) for the rules on where a secret may be read.

#### Refused definitions

A definition the engine refuses throws an `EngineError`: a JS `Error` whose
`code` is `"REJECTED"` and whose `issues` are every issue the definitions
have, each at its authored path. A malformed argument throws
`code: "INVALID_ARGUMENT"`, with a message naming the argument.

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

An `Issue` is `{ code, severity, path, step_kind, step_id, field, message }`.
`severity` is `"REJECTED"`, `"DEFECT"` or `"ADVISORY"`; `path` is the
authored path, spelled `tasks[1].tasks[0].id`. Advisories do not keep a
definition from loading; they are listed with the rest when another issue
does. See [Authoring-Time Validation](../advanced/authoring-validation.md)
for the codes.

### `engine.run(payload, options?)`

Runs one message and resolves, on every status, with `{ message, run,
trace? }`. The promise rejects only for a malformed argument
(`"INVALID_ARGUMENT"`); a run that stops, times out or fails resolves, with
`run.status` saying how.

```typescript
interface RunOptions {
  id?: string;                   // the message id; a UUID v7 unless set
  data?: JsonValue;              // the `data` root the message starts with; an object, else ignored
  metadata?: JsonValue;          // the `metadata` root the message starts with; an object, else ignored
  channel?: string;              // the channel whose workflows run; every active workflow when unset
  routing_bucket?: number;       // the rollout bucket, 0 to 255; every rollout admits an unset one
  capture_changes?: boolean;     // record each task's changes in the audit trail; on by default
  audit?: 'full' | 'off' | { last: number };  // which audit entries the message keeps; "full" by default
  trace?: boolean | TraceOptions;             // trace the run; off by default
  timeout_ms?: number;           // a deadline from the call; the run then ends "timed_out"
}

interface TraceOptions {
  timings?: boolean;    // time each call into its step's `elapsed_us`; off by default
  changes?: boolean;    // record each step's changes, so the trace replays; on by default
  redact?: string[];    // context paths whose values read null in the trace; the message keeps them
  max_bytes?: number;   // a ceiling on what the trace holds; see `trace.truncated`
}

interface RunResult {
  message: Message;     // the message after the run
  run: Run;             // how the run ended and what each workflow did
  trace?: Trace;        // when `trace` was asked for
}
```

`timeout_ms` is the one limit a browser host can set. The deadline is checked
at every loop sweep and every 64th operation; a run that crosses it ends
`"timed_out"` with `retryable: true`, `partial: true`, and the message and
trace as they stood.

#### The message

```typescript
interface Message {
  id: string;
  payload: JsonValue;             // what the run was given, unchanged
  context: {
    data: JsonValue;
    metadata: JsonValue;
    temp_data: JsonValue;
  };
  audit_trail: AuditEntry[];      // one entry per task that ran, as `audit` kept them
  errors: ErrorRecord[];          // every record the run added
}
```

`data` lives under `context`, not at the top level. The engine stamps
`metadata.processed_at` and `metadata.engine_version` at the start of every
run, `metadata.channel` under a channel, and writes `metadata.progress`
(`{workflow_id, task_id, status_code}`) after every task.

#### The run

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

interface DataflowError {
  code: string;
  message: string;
  retryable: boolean;
}
```

`completed_with_errors` is a run that was not stopped but recorded errors on
the message: a `validation` task's failed rules, or a task failure a policy
told the workflow to continue past. `stopped` is a run a failure ended under
the default policies; its `error` is the failure.

#### Reading a failing run

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

A resolved promise does not mean "no errors". Read `run.status` for how the
run ended and `message.errors` for what it recorded on the way.

### `engine.check(definition)`

Parses one definition and checks it against this engine's functions. Gives
every issue, each at its authored path; an empty array means it would load.

```typescript
const issues = engine.check({
  id: 'draft',
  tasks: [{ id: 't', function: { name: 'nope', input: {} } }],
});
// [{ code: "UNKNOWN_FUNCTION", severity: "REJECTED", path: "tasks[0].function.name", ... }]
```

### Module functions

```typescript
import init, {
  engine_version,
  engine_facts,
  parse,
  contexts,
  mapping_contexts,
} from '@goplasmatic/dataflow-wasm';

/** The default export: instantiate the module. Call it once, before anything else. */
await init();

/** The engine compiled into this module, e.g. "4.0.0". */
engine_version(): string;

/** The engine's facts, so a tool need not keep a copy of them. */
engine_facts(): {
  version: string;
  builtin_functions: string[];
  max_group_depth: number;
  loop_defaults: { init: number; increment: number };
  for_each_defaults: { max_concurrency: number };
};

/** A definition as the engine reads it: in the 4.0 spelling, with its issues. */
parse(definition: JsonValue): { definition: JsonValue | null; issues: Issue[] };

/** The context after each step of a trace. */
contexts(trace: Trace): Context[];

/** The context after each mapping of a `map` step. */
mapping_contexts(trace: Trace, step: number): Context[];
```

`parse` rewrites a 3.x spelling and reports it as a `V3_SPELLING` advisory at
the 3.x key: `continue_on_error: true` becomes `on_failure: "continue"`,
`halt_on: "failure"` becomes `on_rejection: "halt"`, and `validate` becomes
`validation`. A key the engine does not know is moved into the node's `ext`
and reported. `definition` is `null` when the definition was rejected.

Pair `engine_version()` with the version your frontend was built against and
fail loudly on a mismatch. An older engine reads a key it predates as an
unknown key, kept in `ext` and reported, so a workflow using a newer feature
loads and does something other than what it says.

## The trace

A trace is `{ initial, steps, truncated }`: the context before any workflow
(redacted as asked), one step per call, skip, failure and engine write, and
whether the byte budget was crossed. A step carries its changes, not a
snapshot of the message:

```typescript
interface TraceStep {
  workflow: number;               // the workflow's position in the program
  workflow_id: string;
  task: number | null;            // null for the workflow's own step: its skip, its failure, or the engine's writes
  task_id: string | null;
  result: 'executed' | 'skipped' | 'failed';
  status: number | null;          // the call's status; null for a skip and for the engine's step
  changes: Change[];              // the step's writes, in order
  mapping_ends?: number[];        // a `map` step: where each mapping's changes end in `changes`
  errors?: ErrorRecord[];         // every record the step added to the message
  elapsed_us?: number;            // with `timings: true`
  loop_counter?: number;          // the sweep, for a loop
  element_index?: number;         // the element, for a fan-out
}
```

Optional fields are absent, not `null`, when there is nothing to record:
`mapping_ends` and `errors` when empty, `elapsed_us`, `loop_counter` and
`element_index` when there is none. A `failed` step is where a run stopped
or a workflow's policy took over; its `errors` hold the failure's record.

### Replaying

The package replays a trace's changes from `initial`, so the UI and the
engine never disagree on the rules:

```typescript
import { contexts, mapping_contexts } from '@goplasmatic/dataflow-wasm';

const { trace } = await engine.run(payload, { trace: true });
if (!trace) throw new Error('the run was not traced'); // `trace` is optional on RunResult

const after = contexts(trace);   // one Context per step, in order
trace.initial;                   // before the first step
after[0];                        // after the first step
after.at(-1);                    // the message's context after the run, unless `trace.truncated`

for (const [i, step] of trace.steps.entries()) {
  if (step.mapping_ends) {
    // A `map` step: the context after each of its mappings, in order.
    const perMapping = mapping_contexts(trace, i);
  }
}
```

`contexts` and `mapping_contexts` read the trace back and check it before
indexing anything by it, so a trace that was stored and edited, or came from
another build, throws `code: "INVALID_TRACE"` rather than failing the page.
`mapping_contexts` gives an empty array for a step that is not a `map`, and
throws `"INVALID_ARGUMENT"` for a step past the trace's last.

`trace.truncated` is `true` when `max_bytes` was crossed. From the step that
crossed it on, every step's changes are empty, so the contexts replayed from
there on are not whole.

## Types

The package's `dataflow_wasm.d.ts` is typed end to end. It declares the
engine's wire types, generated from the crate: `Message`, `Context`,
`Change`, `AuditEntry`, `ErrorRecord`, `Trace`, `TraceStep`, `StepResult`,
`Run`, `RunStatus`, `WorkflowRecord`, `Selection`, `WorkflowExit`,
`DataflowError`, `Issue`, `Severity`, `JsonValue`. Beside them it declares
the package's own: `EngineOptions`, `RunOptions`, `TraceOptions`,
`AuditMode`, `RunResult`, `EngineError`, `EngineFacts`, `ParseResult`. The
same shapes are what a Rust host's `Message`, `Run` and `Trace` serialize
to, so a result from a server reads with the same types.

## From 3.x

| 3.x | 4.0 |
|---|---|
| `new WasmEngine(workflowsJson)` | Unchanged; an array of definitions is accepted too |
| `WasmEngine.with_secrets(workflowsJson, secretsJson)` | `new WasmEngine(workflows, { secrets })` |
| `engine.process(payload)`, resolving to a JSON string | `engine.run(payload)`, resolving to `{ message, run }` as objects |
| `engine.process_with_trace(payload)` | `engine.run(payload, { trace: true })`; the trace is `result.trace` |
| `process_message(workflowsJson, payload)` | Build an engine and call `run` |
| `engine.workflow_ids()`, a JSON string | A `string[]` |
| The payload forced to a string | Any value; a string stays a string |
| A failing run rejected the promise with a string | Resolves; read `run.status` and `run.error` |
| A refused definition threw a string | Throws an `EngineError` with `code` and `issues` |
| A step's `message` snapshot and `mapping_contexts` | `contexts(trace)` and `mapping_contexts(trace, step)` |
| A step's `duration_us` and `started_at` | `elapsed_us`, with `trace: { timings: true }`; no start time |
| `trace.truncated` omitted when `false` | Always present |

Definitions written for 3.x load as they are: `continue_on_error`, `halt_on`
and `validate` are rewritten to `on_failure`, `on_rejection` and `validation`
and reported as `V3_SPELLING` advisories. `parse(definition)` shows the
rewritten form.

## Operator availability

This package is built with `all-operators`, so every optional operator family
(`ext-string`, `ext-array`, `ext-object`, `ext-math`, `ext-control`,
`error-handling` and `datetime`) is live in the browser. `tensor` is not
included: its operator names (`shape`, `cast`, `concat`) are ordinary JSON
keys in workflows that already ship.

A default `cargo add dataflow-rs` build enables **none** of them. Because the
engine evaluates in templating mode, an operator whose family is off is not an
error: it passes through as literal data. An expression using `length` or
`switch` therefore works in the browser and is silently inert in a default Rust
build. See [JSONLogic](../advanced/jsonlogic.md#operator-families-cargo-features).

## Building from Source

Requirements:
- Rust 1.98+ (the workspace MSRV)
- wasm-pack

```bash
cd wasm
wasm-pack test --node                          # the package's tests, on wasm32 under Node
wasm-pack build --target web --out-dir pkg     # the package, in wasm/pkg/
node scripts/verify-wasm.mjs pkg               # the binary's features and the .d.ts's types
```

The verification step is not optional in CI: it checks the emitted binary
still carries the features the glue depends on, so a `wasm-opt` regression
fails the build instead of shipping a package that throws on `init()`, and
that the `.d.ts` declares the wire types the UI imports.

## Browser Compatibility

The published binary is not baseline WebAssembly. It is compiled with reference
types, bulk memory, non-trapping float-to-int and sign extension enabled (the
`wasm-opt` profile in `wasm/Cargo.toml`), and the generated glue grows an
externref table during init. Reference types is the binding constraint:

- Chrome / Edge 96+
- Firefox 79+
- Safari 15+

These are hard requirements: an engine without reference types throws
`RangeError` on the first `init()` call instead of falling back.
`wasm/scripts/verify-wasm.mjs` exists to catch a build that regresses this.

## Next Steps

- [UI Package](../ui/overview.md) - React visualization and debugging components
- [Built-in Functions](../built-in-functions/overview.md) - Map, validation, and more
