<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  # @goplasmatic/dataflow-wasm

  **WebAssembly bindings for dataflow-rs workflow engine**

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![npm](https://img.shields.io/npm/v/@goplasmatic/dataflow-wasm.svg)](https://www.npmjs.com/package/@goplasmatic/dataflow-wasm)
</div>

---

WebAssembly bindings for [dataflow-rs](https://github.com/GoPlasmatic/dataflow-rs), enabling workflow execution in the browser. Run the same workflow engine that powers your Rust backend directly in JavaScript/TypeScript applications.

## Features

- **Browser Execution** - Run dataflow-rs workflows directly in the browser
- **Same Engine** - The native Rust engine compiled to WebAssembly, built with `all-operators` (every optional operator family except `tensor`)
- **Built-in Functions** - `parse_json`, `parse_xml`, `map`, `validation`, `filter`, `log`, `publish_json`, `publish_xml`
- **TypeScript Support** - Full type definitions included
- **Execution Tracing** - Debug workflows with step-by-step execution traces and message snapshots

## Installation

```bash
npm install @goplasmatic/dataflow-wasm
```

## Quick Start

```typescript
import init, { WasmEngine } from '@goplasmatic/dataflow-wasm';

// Initialize WASM module
await init();

// Define workflows
const workflows = [
  {
    id: 'my-workflow',
    name: 'My Workflow',
    tasks: [
      {
        // Parse the raw payload string into data.input
        id: 'parse-payload',
        name: 'Parse Payload',
        function: {
          name: 'parse_json',
          input: { source: 'payload', target: 'input' }
        }
      },
      {
        id: 'task-1',
        name: 'Transform Data',
        function: {
          name: 'map',
          input: {
            mappings: [
              { path: 'data.output', logic: { var: 'data.input.greeting' } }
            ]
          }
        }
      }
    ]
  }
];

// Create engine
const engine = new WasmEngine(JSON.stringify(workflows));

// Process a payload (a raw string, parsed by the parse_json task)
const payload = '{"greeting": "hello"}';
const result = await engine.process(payload);
const parsed = JSON.parse(result);
console.log(parsed.context.data); // { input: { greeting: 'hello' }, output: 'hello' }
```

## API

### WasmEngine

```typescript
class WasmEngine {
  // Create engine from JSON string of workflow definitions
  constructor(workflows_json: string);

  // As the constructor, with a JSON object of secrets the workflows read
  // through {"secret": "name"}. Held by the engine, never by a message.
  static with_secrets(workflows_json: string, secrets_json: string): WasmEngine;

  // Process a raw payload string through all workflows
  // The payload is stored as-is; parse it with a parse_json or parse_xml task
  process(payload: string): Promise<string>;

  // Process with execution trace for debugging
  process_with_trace(payload: string): Promise<string>;

  // Get number of registered workflows
  workflow_count(): number;

  // Get list of workflow IDs as JSON array string
  workflow_ids(): string;
}
```

### Standalone Function

```typescript
// Process a payload through a one-off engine (convenience function)
// Use WasmEngine class for better performance when processing multiple payloads
function process_message(workflows_json: string, payload: string): Promise<string>;
```

### Payload Handling

The payload is stored as a **raw string** and is not automatically parsed, and it is not part of the JSONLogic evaluation context, so `{"var": "payload.x"}` resolves to nothing. Make `parse_json` (or `parse_xml`) the first task in your workflow to parse the payload into `context.data`:

```typescript
{
  id: 'parse-payload',
  name: 'Parse Payload',
  function: {
    name: 'parse_json',       // or 'parse_xml'
    input: {
      source: 'payload',      // required: where to read from
      target: 'input'         // required: stored at data.input
    }
  }
}
```

### Message Structure

The processed message has the following structure:

```typescript
interface Message {
  id: string;
  payload: string;              // Raw payload string
  context: {
    data: object;               // Parsed data (populated by parse_json / parse_xml)
    metadata: object;           // Workflow metadata
    temp_data: object;          // Temporary data during processing
  };
  audit_trail: AuditEntry[];    // Execution history
  errors: ErrorInfo[];          // Any errors that occurred
}
```

## Building from Source

Requirements:
- Rust 1.98+
- wasm-pack

```bash
# Build WASM package
cd wasm
wasm-pack build --target web

# The output will be in wasm/pkg/
```

## Related Packages

- [dataflow-rs](https://crates.io/crates/dataflow-rs) - Core Rust workflow engine
- [@goplasmatic/dataflow-ui](https://www.npmjs.com/package/@goplasmatic/dataflow-ui) - React visualization library

## License

This project is licensed under the Apache License, Version 2.0. See the [LICENSE](../LICENSE) file for details.
