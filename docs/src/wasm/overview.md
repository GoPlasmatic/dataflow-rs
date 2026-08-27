# WebAssembly Package

The `@goplasmatic/dataflow-wasm` package provides WebAssembly bindings for dataflow-rs, enabling you to run the same rules engine in the browser that powers your Rust backend.

## Installation

```bash
npm install @goplasmatic/dataflow-wasm
```

## Quick Start

Everything crossing the WASM boundary is a **string**. Workflows go in as a
JSON string, the payload goes in as a raw string, and both `process` methods
resolve to a JSON string you parse yourself.

```typescript
import init, { WasmEngine } from '@goplasmatic/dataflow-wasm';

// Instantiate the module once, before any other call.
await init();

const workflows = JSON.stringify([
  {
    id: 'my-workflow',
    name: 'My Workflow',
    tasks: [
      // The payload arrives as a raw string, so parse it into `data` first.
      {
        id: 'parse',
        name: 'Parse Payload',
        function: { name: 'parse_json', input: { source: 'payload', target: 'input' } }
      },
      {
        id: 'transform',
        name: 'Transform Data',
        function: {
          name: 'map',
          input: { mappings: [{ path: 'data.output', logic: { var: 'data.input.greeting' } }] }
        }
      }
    ]
  }
]);

const engine = new WasmEngine(workflows);

const result = JSON.parse(await engine.process('{"greeting": "hello world"}'));
console.log(result.context.data.output); // 'hello world'
```

Two things trip people up here, and both are structural rather than cosmetic:

- **The payload is not parsed for you.** `process(payload)` stores the string
  verbatim as the message payload. Without a `parse_json` (or `parse_xml`) task,
  `data` stays empty.
- **`payload` is not in the JSONLogic evaluation context.** Conditions and
  mappings see `{data, metadata, temp_data}` only, so `{"var": "payload.x"}`
  never resolves — parse into `data` and read from there.

## API Reference

### WasmEngine

```typescript
class WasmEngine {
  /** Throws if the JSON is invalid, is not an array, or any workflow fails to load. */
  constructor(workflowsJson: string);

  /**
   * As the constructor, with a JSON object of secrets the workflows read through
   * `{"secret": "name"}`. Held by the engine, never by a message — see the
   * Secrets page. Throws on invalid JSON, a non-object store, or a workflow
   * that reads an undeclared name.
   */
  static with_secrets(workflowsJson: string, secretsJson: string): WasmEngine;

  /** Resolves to a serialized Message; rejects with an error string. */
  process(payload: string): Promise<string>;

  /** Resolves to a serialized ExecutionTrace; rejects with an error string. */
  process_with_trace(payload: string): Promise<string>;

  /** Number of registered workflows. */
  workflow_count(): number;

  /** JSON array of workflow ids, as a string. */
  workflow_ids(): string;

  /** Release the WASM memory held by this engine. */
  free(): void;
}
```

`process_with_trace` is snake_case — it is the Rust name passed straight
through `wasm_bindgen`, not a camelCase JavaScript alias.

### Module functions

```typescript
/** Instantiate the module. Also installs a panic hook so Rust panics surface in the console. */
export default function init(): Promise<InitOutput>;

/** Engine version compiled into this module, e.g. "3.7.0". */
export function engine_version(): string;

/** One-off convenience: build an engine, process one payload, discard it. */
export function process_message(workflowsJson: string, payload: string): Promise<string>;
```

Pair `engine_version()` with the version your frontend was built against and
fail loudly on a mismatch. Workflow definitions do **not** set
`deny_unknown_fields`, so an older engine silently *ignores* a field it predates
rather than rejecting it — the workflow runs and quietly does something other
than what it says.

### What `process` resolves to

A serialized `Message`:

```typescript
interface Message {
  id: string;
  payload: unknown;                // the raw string you passed in
  context: {
    data: Record<string, unknown>;
    metadata: Record<string, unknown>;
    temp_data: Record<string, unknown>;
  };
  audit_trail: AuditTrail[];
  errors: ErrorInfo[];
}
```

Note `data` lives under `context`, not at the top level.

### Error behaviour

There are two distinct failure channels, and a resolved Promise does not mean
"no errors":

- The Promise **rejects** with a string when the engine stopped early — a task
  failed with `continue_on_error: false`.
- The Promise **resolves** with a message whose `errors` array is non-empty when
  failures were tolerated. Always check `result.errors.length`.

## Operator availability

This package is built with `all-operators`, so every optional operator family —
`ext-string`, `ext-array`, `ext-object`, `ext-math`, `ext-control`,
`error-handling` and `datetime` — is live in the browser.

A default `cargo add dataflow-rs` build enables **none** of them. Because the
engine evaluates in templating mode, an operator whose family is off is not an
error: it passes through as literal data. An expression using `length` or
`switch` therefore works in the browser and is silently inert in a default Rust
build. See [JSONLogic](../advanced/jsonlogic.md#operator-families-cargo-features).

## Execution Tracing

`process_with_trace` returns the same execution trace the debugger UI consumes:

```typescript
const trace = JSON.parse(await engine.process_with_trace('{"greeting": "hi"}'));

console.log('Steps recorded:', trace.steps.length);

for (const step of trace.steps) {
  // `result` is "executed" or "skipped" (lowercase on the wire).
  // `task_id` is null for a workflow-level step.
  console.log(step.workflow_id, step.task_id, step.result, step.duration_us);
}
```

Optional step fields are **omitted**, not set to `null`: `message`,
`mapping_contexts`, `started_at`, `duration_us`, `changes` and `loop_counter`
are absent unless that data was captured. Guard with `if (step.message)` rather
than comparing against `null`.

`trace.truncated` is `true` when the snapshot budget was exceeded — later steps
are still recorded, but without their `message`. It is omitted when `false`.

Steps carry `loop_counter` for workflows that loop, so repeated sweeps of the
same task are distinguishable.

## Building from Source

Requirements:
- Rust 1.85+ (the workspace MSRV)
- wasm-pack

```bash
cd wasm
wasm-pack build --target web --out-dir pkg
node scripts/verify-wasm.mjs
```

The output will be in `wasm/pkg/`. The verification step is not optional in CI:
it checks the emitted binary still carries the features the glue depends on, so
a `wasm-opt` regression fails the build instead of shipping a package that
throws on `init()`.

## Browser Compatibility

The published binary is not baseline WebAssembly. It is compiled with reference
types, bulk memory, non-trapping float-to-int and sign extension enabled (the
`wasm-opt` profile in `wasm/Cargo.toml`), and the generated glue grows an
externref table during init. Reference types is the binding constraint:

- Chrome / Edge 96+
- Firefox 79+
- Safari 15+

These are hard requirements, not a degradation floor: an engine without
reference types throws `RangeError` on the very first `init()` call rather than
falling back. `wasm/scripts/verify-wasm.mjs` exists to catch a build that
regresses this.

## Next Steps

- [UI Package](../ui/overview.md) - React visualization components
- [Built-in Functions](../built-in-functions/overview.md) - Map, validation, and more
