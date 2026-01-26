# WebAssembly Package

The `@goplasmatic/dataflow-wasm` package provides WebAssembly bindings for dataflow-rs, enabling you to run the same workflow engine in the browser that powers your Rust backend.

## Installation

```bash
npm install @goplasmatic/dataflow-wasm
```

## Quick Start

```typescript
import init, { WasmEngine } from '@goplasmatic/dataflow-wasm';

// Initialize the WASM module (required once)
await init();

// Define your workflows
const workflows = [
  {
    id: 'my-workflow',
    name: 'My Workflow',
    tasks: [
      {
        id: 'transform',
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

// Create the engine
const engine = new WasmEngine(workflows);

// Process a message
const message = {
  data: { input: 'hello world' },
  metadata: {}
};

const result = await engine.process(message);
console.log(result.data.output); // 'hello world'
```

## API Reference

### WasmEngine

The main class for executing workflows.

```typescript
class WasmEngine {
  constructor(workflows: Workflow[]);

  // Process a message through all matching workflows
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
  condition?: JsonLogicValue;  // Optional condition to run this workflow
  tasks: Task[];
}

interface Task {
  id: string;
  name: string;
  condition?: JsonLogicValue;  // Optional condition to run this task
  function: FunctionConfig;
}

interface FunctionConfig {
  name: string;  // 'map' or 'validation'
  input: object;
}

interface Message {
  data: object;
  metadata: object;
  payload?: object;
  temp_data?: object;
}
```

## Execution Tracing

For debugging, use `processWithTrace` to get step-by-step execution details:

```typescript
const trace = await engine.processWithTrace(message);

console.log('Steps executed:', trace.steps.length);
console.log('Initial message:', trace.initial_message);
console.log('Final message:', trace.final_message);

for (const step of trace.steps) {
  console.log(`Task: ${step.task_name}`);
  console.log(`Changes: ${step.changes.length}`);
}
```

## Building from Source

Requirements:
- Rust 1.70+
- wasm-pack

```bash
cd wasm
wasm-pack build --target web --out-dir pkg
```

The output will be in `wasm/pkg/`.

## Browser Compatibility

The WASM package works in all modern browsers that support WebAssembly:
- Chrome 57+
- Firefox 52+
- Safari 11+
- Edge 16+

## Next Steps

- [UI Package](../ui/overview.md) - React visualization components
- [Built-in Functions](../built-in-functions/overview.md) - Map and validation functions
