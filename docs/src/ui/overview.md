# UI Package

The `@goplasmatic/dataflow-ui` package provides React components for visualizing and debugging dataflow-rs rules and workflows.

## Installation

```bash
npm install @goplasmatic/dataflow-ui
```

## Peer Dependencies

```bash
npm install react react-dom
```

Supports React 18.x and 19.x.

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
      onTaskSelect={(task, workflow) => {
        console.log('Selected task:', task.name);
      }}
    />
  );
}
```

## Components

### WorkflowVisualizer

The main component for displaying rules (workflows) in an interactive tree view.

```tsx
interface WorkflowVisualizerProps {
  /** Array of workflow definitions to display */
  workflows: Workflow[];
  /** Callback when a workflow is selected */
  onWorkflowSelect?: (workflow: Workflow) => void;
  /** Callback when a task is selected */
  onTaskSelect?: (task: Task, workflow: Workflow) => void;
  /** Theme: 'light', 'dark', or 'system' */
  theme?: Theme;
  /** Additional CSS class */
  className?: string;
  /** Execution result to display */
  executionResult?: Message | null;
  /** Enable debug mode */
  debugMode?: boolean;
}
```

### TreeView

Standalone tree view component for custom layouts.

```tsx
import { TreeView } from '@goplasmatic/dataflow-ui';

<TreeView
  workflows={workflows}
  selection={currentSelection}
  onSelect={handleSelect}
  debugMode={false}
/>
```

## Debug Mode

Enable step-by-step execution visualization with the debugger components.

```tsx
import {
  WorkflowVisualizer,
  DebuggerProvider,
  DebuggerControls,
  defaultEngineFactory,
  useDebugger
} from '@goplasmatic/dataflow-ui';

function DebugView() {
  return (
    <DebuggerProvider engineFactory={defaultEngineFactory}>
      <WorkflowVisualizer
        workflows={workflows}
        debugMode={true}
      />
      <DebuggerControls />
    </DebuggerProvider>
  );
}
```

## Custom WASM Engine

Use a custom WASM engine with plugins or custom functions for debugging. Implement the `DataflowEngine` interface:

```tsx
import {
  WorkflowVisualizer,
  DebuggerProvider,
  DataflowEngine,
  Workflow
} from '@goplasmatic/dataflow-ui';
import { MyCustomWasmEngine } from './my-custom-wasm';

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

function CustomDebugView() {
  return (
    <DebuggerProvider engineFactory={(workflows) => new MyEngineAdapter(workflows)}>
      <WorkflowVisualizer workflows={workflows} debugMode={true} />
    </DebuggerProvider>
  );
}
```

The `engineFactory` is called whenever workflows change, ensuring the engine always has the latest workflow definitions.

### Debugger Controls

```tsx
import { DebuggerControls } from '@goplasmatic/dataflow-ui';

// Provides playback controls: play, pause, step forward/back, reset
<DebuggerControls />
```

### useDebugger Hook

Access debugger state programmatically:

```tsx
import { useDebugger } from '@goplasmatic/dataflow-ui';

function MyComponent() {
  const {
    state,           // Current playback state
    hasTrace,        // Whether a trace is loaded
    currentMessage,  // Message at current step
    currentChanges,  // Changes made at current step
    loadTrace,       // Load an execution trace
    stepForward,     // Go to next step
    stepBackward,    // Go to previous step
    play,            // Start auto-playback
    pause,           // Pause playback
    reset,           // Reset to beginning
  } = useDebugger();

  // ...
}
```

## Theming

The visualizer supports light, dark, and system themes.

```tsx
// Light theme
<WorkflowVisualizer workflows={workflows} theme="light" />

// Dark theme
<WorkflowVisualizer workflows={workflows} theme="dark" />

// System preference (default)
<WorkflowVisualizer workflows={workflows} theme="system" />
```

### Custom Theme Access

```tsx
import { useTheme } from '@goplasmatic/dataflow-ui';

function MyComponent() {
  const { theme, setTheme, resolvedTheme } = useTheme();
  // resolvedTheme is 'light' or 'dark' (resolved from 'system')
}
```

## Exports

### Components

- `WorkflowVisualizer` - Main visualization component
- `TreeView` - Standalone tree view
- `DebuggerControls` - Debug playback controls
- `DebuggerProvider` - Debug context provider
- `MessageInputPanel` - Message input for debugging
- `MessageStatePanel` - Message state display
- `JsonViewer` - JSON display component
- `ErrorBoundary` - Error boundary wrapper

### Hooks

- `useTheme` - Theme state and controls
- `useDebugger` - Debugger state and controls
- `useTaskDebugState` - Debug state for a specific task
- `useWorkflowDebugState` - Debug state for a workflow

### Engine

- `WasmEngineAdapter` - Default WASM engine adapter
- `defaultEngineFactory` - Factory function for default engine
- `DataflowEngine` - Interface for custom engines
- `EngineFactory` - Type for engine factory functions

### Types

All TypeScript types are exported for workflow definitions, tasks, messages, and execution traces.

## Building from Source

```bash
cd ui
npm install
npm run build:lib
```

Output will be in `ui/dist/`.

## Next Steps

- [WASM Package](../wasm/overview.md) - Run rules in the browser
- [Core Concepts](../core-concepts/overview.md) - Understand rules and actions
