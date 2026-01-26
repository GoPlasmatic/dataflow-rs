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
- **Built-in Functions** - parse_json, parse_xml, map, validation, publish_json, publish_xml
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
const engine = new WasmEngine(workflows);

// Process a message
const message = { data: { input: 'hello' }, metadata: {} };
const result = await engine.process(message);
console.log(result); // { data: { input: 'hello', output: 'hello' }, ... }
```

## API

### WasmEngine

```typescript
class WasmEngine {
  constructor(workflows: Workflow[]);

  // Process a message through all workflows
  process(message: Message): Promise<Message>;

  // Process with execution trace for debugging
  processWithTrace(message: Message): Promise<ExecutionTrace>;
}
```

### Types

```typescript
interface Workflow {
  id: string;
  name: string;
  condition?: JsonLogicValue;
  tasks: Task[];
}

interface Task {
  id: string;
  name: string;
  condition?: JsonLogicValue;
  function: FunctionConfig;
}

interface Message {
  data: object;
  metadata: object;
  payload?: object;
  temp_data?: object;
}

interface ExecutionTrace {
  steps: ExecutionStep[];
  initial_message: Message;
  final_message: Message;
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
