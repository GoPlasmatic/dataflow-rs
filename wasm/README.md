<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  # @goplasmatic/dataflow-wasm

  **WebAssembly bindings for dataflow-rs workflow engine**

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![npm](https://img.shields.io/npm/v/@goplasmatic/dataflow-wasm.svg)](https://www.npmjs.com/package/@goplasmatic/dataflow-wasm)
</div>

---

WebAssembly bindings for [dataflow-rs](https://github.com/GoPlasmatic/dataflow-rs), enabling high-performance workflow execution in the browser. Run the same workflow engine that powers your Rust backend directly in JavaScript/TypeScript applications.

## Features

- **Browser Execution** - Run dataflow-rs workflows directly in the browser
- **Full Feature Parity** - Same workflow engine as the native Rust version including all built-in functions
- **Built-in Functions** - parse, map, validation, publish
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
        // Parse the raw payload string into data
        id: 'parse-payload',
        name: 'Parse Payload',
        function: {
          name: 'parse',
          input: {}
        }
      },
      {
        id: 'task-1',
        name: 'Transform Data',
        function: {
          name: 'map',
          input: {
            mappings: [
              { path: 'data.output', logic: { var: 'data.input' } }
            ]
          }
        }
      }
    ]
  }
];

// Create engine
const engine = new WasmEngine(JSON.stringify(workflows));

// Process a payload (raw string - parsed by the parse plugin)
const payload = '{"input": "hello"}';
const result = await engine.process(payload);
const parsed = JSON.parse(result);
console.log(parsed.context.data); // { input: 'hello', output: 'hello' }
```

## API

### WasmEngine

```typescript
class WasmEngine {
  // Create engine from JSON string of workflow definitions
  constructor(workflows_json: string);

  // Process a raw payload string through all workflows
  // The payload is stored as-is and should be parsed by the parse plugin
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

The payload is stored as a **raw string** and is not automatically parsed. Use the `parse` plugin as the first task in your workflow to parse JSON/XML payloads into `context.data`:

```typescript
{
  id: 'parse-payload',
  name: 'Parse Payload',
  function: {
    name: 'parse',
    input: {
      source: 'payload',      // default
      target: 'data',         // default
      format: 'json'          // default, or 'xml'
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
    data: object;               // Parsed data (populated by parse plugin)
    metadata: object;           // Workflow metadata
    temp_data: object;          // Temporary data during processing
  };
  audit_trail: AuditEntry[];    // Execution history
  errors: ErrorInfo[];          // Any errors that occurred
}
```

## Building from Source

Requirements:
- Rust 1.70+
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
