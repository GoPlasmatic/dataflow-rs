<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  # @goplasmatic/dataflow-ui

  **React visualization library for dataflow-rs workflow engine**

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![npm](https://img.shields.io/npm/v/@goplasmatic/dataflow-ui.svg)](https://www.npmjs.com/package/@goplasmatic/dataflow-ui)
  [![TypeScript](https://img.shields.io/badge/TypeScript-5.0+-blue.svg)](https://www.typescriptlang.org/)
</div>

---

A React component library for visualizing and debugging [dataflow-rs](https://github.com/GoPlasmatic/dataflow-rs) workflows. Features an interactive tree view, step-by-step execution debugging, and JSONLogic visualization.

## Features

- **Workflow Visualization** - Interactive tree view of workflows, tasks, and conditions
- **Execution Debugging** - Step-by-step execution trace visualization with message snapshots
- **JSONLogic Viewer** - Visual representation of JSONLogic expressions via [@goplasmatic/datalogic-ui](https://www.npmjs.com/package/@goplasmatic/datalogic-ui)
- **Theme Support** - Light, dark, and system theme modes
- **TypeScript** - Full type definitions included
- **Monaco Editor Integration** - JSON editing with syntax highlighting
- **Change Highlighting** - Visual diff of message changes at each step

## Installation

```bash
npm install @goplasmatic/dataflow-ui
```

## Quick Start

```tsx
import { WorkflowVisualizer } from '@goplasmatic/dataflow-ui';
import '@goplasmatic/dataflow-ui/styles.css';

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

function App() {
  return (
    <WorkflowVisualizer
      workflows={workflows}
      theme="system"
      onTaskSelect={(task, workflow) => console.log('Selected:', task.name)}
    />
  );
}
```

## Components

### WorkflowVisualizer

The main component for displaying workflows.

```tsx
interface WorkflowVisualizerProps {
  workflows: Workflow[];
  onWorkflowSelect?: (workflow: Workflow) => void;
  onTaskSelect?: (task: Task, workflow: Workflow) => void;
  theme?: 'light' | 'dark' | 'system';
  className?: string;
  executionResult?: Message | null;
  debugConfig?: DebugConfig;
  debugPayload?: Record<string, unknown>;
}

interface DebugConfig {
  enabled: boolean;
  engineFactory?: EngineFactory;
  initialPayload?: Record<string, unknown>;
  autoExecute?: boolean;
  onExecutionComplete?: (trace: ExecutionTrace) => void;
  onExecutionError?: (error: string) => void;
}
```

### Debug Mode

Enable step-by-step execution visualization with integrated debug controls:

```tsx
import { WorkflowVisualizer, defaultEngineFactory } from '@goplasmatic/dataflow-ui';

function DebugView() {
  const payload = { data: { input: 'hello' } };

  return (
    <WorkflowVisualizer
      workflows={workflows}
      debugConfig={{
        enabled: true,
        engineFactory: defaultEngineFactory,
        autoExecute: true,
      }}
      debugPayload={payload}
    />
  );
}
```

The debug controls (play, pause, step forward/backward) are automatically displayed in the visualizer header when `debugConfig.enabled` is true.

### Custom WASM Engine

Use a custom WASM engine with plugins or custom functions:

```tsx
import { WorkflowVisualizer, DataflowEngine, EngineFactory } from '@goplasmatic/dataflow-ui';
import { MyCustomWasmEngine } from './my-custom-wasm';

// Implement the DataflowEngine interface
class MyEngineAdapter implements DataflowEngine {
  private engine: MyCustomWasmEngine;

  constructor(workflows: Workflow[]) {
    this.engine = new MyCustomWasmEngine(JSON.stringify(workflows));
  }

  async processWithTrace(payload: Record<string, unknown>) {
    const result = await this.engine.process_with_trace(JSON.stringify(payload));
    return JSON.parse(result);
  }

  dispose() {
    this.engine.free();
  }
}

const customEngineFactory: EngineFactory = (workflows) => new MyEngineAdapter(workflows);

function CustomDebugView() {
  return (
    <WorkflowVisualizer
      workflows={workflows}
      debugConfig={{
        enabled: true,
        engineFactory: customEngineFactory,
        autoExecute: true,
      }}
      debugPayload={{ data: { input: 'test' } }}
    />
  );
}
```

## Exports

### Components
- `WorkflowVisualizer` - Main visualization component with integrated debug controls
- `TreeView` - Standalone tree view
- `DebuggerProvider` - Debug state context provider (for advanced use cases)

### Hooks
- `useTheme` - Access theme state
- `useDebugger` - Access debugger state and controls
- `useTaskDebugState` - Get debug state for a specific task

### Engine
- `WasmEngineAdapter` - Default WASM engine adapter
- `defaultEngineFactory` - Factory function for default engine
- `DataflowEngine` - Interface for custom engines
- `EngineFactory` - Type for engine factory functions

### Types
- `Workflow`, `Task`, `Message` - Core workflow types
- `ExecutionTrace`, `ExecutionStep` - Execution trace types
- `DebugConfig` - Debug mode configuration

## Peer Dependencies

- React 18.x or 19.x
- React DOM 18.x or 19.x

## Related Packages

- [dataflow-rs](https://crates.io/crates/dataflow-rs) - Core Rust workflow engine
- [@goplasmatic/dataflow-wasm](https://www.npmjs.com/package/@goplasmatic/dataflow-wasm) - WebAssembly bindings
- [@goplasmatic/datalogic-ui](https://www.npmjs.com/package/@goplasmatic/datalogic-ui) - JSONLogic visualization

## License

This project is licensed under the Apache License, Version 2.0. See the [LICENSE](../LICENSE) file for details.
