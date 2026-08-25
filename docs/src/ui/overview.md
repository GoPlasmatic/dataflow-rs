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
  /** Additional CSS class for the root element */
  className?: string;
  /** Execution result to display in the result panel */
  executionResult?: Message | null;
  /** Integrated debug mode — see Debug Mode below */
  debugConfig?: DebugConfig;
  /** Payload for debugging; takes precedence over debugConfig.initialPayload */
  debugPayload?: Record<string, unknown>;
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

Step-by-step execution visualization. The simplest form is `debugConfig` — the
visualizer then wraps itself in a `DebuggerProvider` and renders the controls
in its own header, so you do not assemble the pieces yourself:

```tsx
import { WorkflowVisualizer, defaultEngineFactory } from '@goplasmatic/dataflow-ui';

function DebugView() {
  return (
    <WorkflowVisualizer
      workflows={workflows}
      debugConfig={{
        enabled: true,
        engineFactory: defaultEngineFactory,
        autoExecute: true,
        onExecutionComplete: (trace) => console.log(trace.steps.length, 'steps'),
        onExecutionError: (error) => console.error(error),
      }}
      debugPayload={{ greeting: 'hello' }}
    />
  );
}
```

`engineFactory` is what makes execution possible: without it the run button is
disabled. `defaultEngineFactory` uses the WASM engine from
`@goplasmatic/dataflow-wasm`.

### Initialising the engine

`defaultEngineFactory` builds a `WasmEngineAdapter`, which calls into
`@goplasmatic/dataflow-wasm` as soon as it is constructed. That package is a
`--target web` wasm-bindgen build, so its default export must be awaited **once**
before any other export is touched — otherwise the constructor throws a bare
`TypeError` from the uninitialised glue, before the version handshake below can
say anything useful.

Withhold `engineFactory` until it resolves; until then the run button is simply
disabled:

```tsx
import { useEffect, useState } from 'react';
import initWasm from '@goplasmatic/dataflow-wasm';
import { defaultEngineFactory } from '@goplasmatic/dataflow-ui';

function useEngineFactory() {
  const [ready, setReady] = useState(false);
  useEffect(() => {
    initWasm().then(() => setReady(true));
  }, []);
  return ready ? defaultEngineFactory : undefined;
}
```

Then pass `engineFactory: useEngineFactory()` instead of the bare
`defaultEngineFactory` in either of the shapes above.

```tsx
interface DebugConfig {
  enabled: boolean;
  engineFactory?: EngineFactory;
  initialPayload?: Record<string, unknown>;
  autoExecute?: boolean;                                 // default: false
  onExecutionComplete?: (trace: ExecutionTrace) => void;
  onExecutionError?: (error: string) => void;
}
```

### `debugConfig` is what turns debug mode on

Wrapping `WorkflowVisualizer` in a `DebuggerProvider` does **not** enable debug
mode. The visualizer derives it from `debugConfig.enabled` alone, and when that
is true it creates its *own* `DebuggerProvider` internally — which shadows any
ambient one for everything it renders.

The practical consequences:

- `<DebuggerProvider><WorkflowVisualizer workflows={…} /></DebuggerProvider>`
  with no `debugConfig` renders in plain, non-debug mode.
- Panels you place **outside** the visualizer read the ambient provider, while
  the visualizer reads its own. Two providers means two independent states, so
  the external panels will not follow the visualizer's playback.

To drive your own layout, use the components standalone under one provider and
leave the visualizer out of the debug path — or keep everything inside
`debugConfig` and let the built-in toolbar drive it.

### Engine version handshake

`WasmEngineAdapter` calls `assertEngineVersion()` on construction and **throws**
when the loaded WASM engine is older than the UI build expects. This is worth
understanding rather than catching blindly: workflow definitions do not reject
unknown fields, so an older engine silently ignores a field it predates — the
workflow appears to run while doing something else. A newer engine passes
silently, since the package declares a caret range on the wasm dependency.

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
    // State
    state,            // Full debugger state
    hasTrace,         // Whether a trace is loaded
    currentStep,      // ExecutionStep | null
    currentMessage,   // Message at the current step
    currentChanges,   // Changes recorded at the current step
    isAtStart,
    isAtEnd,
    progress,         // 0..1
    totalSteps,
    isEngineReady,

    // Playback
    play,
    pause,
    stop,
    reset,
    stepForward,
    stepBackward,
    goToStep,         // (index: number) => void
    setSpeed,         // (speed: number) => void

    // Execution
    runExecution,     // (workflows, payload) => Promise<ExecutionTrace | null>
    executeTrace,     // (trace) => void — load a trace you already have
    setInputPayload,
  } = useDebugger();

  // ...
}
```

`useDebugger` throws outside a `DebuggerProvider`. For a component that should
work in both contexts, use `useDebuggerOptional`, which returns `null` instead —
that is how `TreeView` renders with and without the debugger attached.

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

| Export | Purpose |
|--------|---------|
| `WorkflowVisualizer` | Main visualization component |
| `TreeView` | Standalone tree view |
| `RulesListView` | Flat list of rules |
| `WorkflowFlowView` | Flow-diagram view of one workflow |
| `WorkflowCard`, `TaskRow` | Card and row primitives |
| `FunctionTypeBadge`, `ConditionBadge` | Badges for a task's function and condition |
| `DebuggerControls` | Playback controls |
| `IntegratedDebugToolbar` | Toolbar used by `debugConfig` mode |
| `MessageInputPanel`, `MessageStatePanel` | Debug input and state panels |
| `DebugInfoBubble`, `DebugStateBadge` | Per-node debug indicators |
| `JsonViewer`, `SearchInput`, `ErrorBoundary` | Common building blocks |

### Providers and hooks

| Export | Purpose |
|--------|---------|
| `ThemeProvider`, `useTheme` | Theme state and controls |
| `DebuggerProvider`, `useDebugger` | Debugger state and controls |
| `useDebuggerOptional` | As `useDebugger`, but returns `null` outside a provider |
| `useTreeNodeDebugState` | Debug state for any tree node |
| `useWorkflowDebugState`, `useWorkflowConditionDebugState` | Per-workflow debug state |
| `useTaskDebugState`, `useTaskConditionDebugState` | Per-task debug state |

### Engine

| Export | Purpose |
|--------|---------|
| `WasmEngineAdapter` | Default WASM engine adapter |
| `defaultEngineFactory` | Factory producing `WasmEngineAdapter` |
| `assertEngineVersion` | Throws when the loaded engine is older than this UI build |
| `DataflowEngine`, `EngineFactory` | Types for custom engines |

### Helpers

Exported alongside the types, for code that walks definitions or traces:

- **Steps:** `isTaskGroup`, `groupMembers`, `flattenSteps`, `countLeafSteps` —
  a workflow's `tasks` array holds *steps*, so an element may be a task or a
  nested group. `countLeafSteps` is `flattenSteps(..).length` without the list.
- **Functions:** `isBuiltinFunction`, `getFunctionDisplayInfo`,
  `INTEGRATION_FUNCTION_NAMES` — the three config-only built-ins that need a
  handler registered by the host.
- **Loops:** `loopBadgeLabel`, `loopGuardLabel`, `loopStepLabel`, `loopDescription`
- **Debug:** `createEmptyMessage`, `cloneMessage`, `getMessageAtStep`,
  `getChangesAtStep`, `getWorkflowState`, `getTaskState`, `traceHasSnapshots`

### Types

`Workflow`, `Task`, `TaskGroup`, `Step`, `FunctionConfig`, `JsonLogicValue`,
`MapMapping`, `MappingItem`, `MapFunctionInput`, `ValidationRule`,
`ValidationFunctionInput`, `BuiltinFunctionType`, `LoopConfig`,
`WorkflowStatus`, `Rollout`, `Message`,
`ErrorInfo`, `Change`, `AuditTrail`, `DebugNodeState`, `ConditionResult`,
`ExecutionStep`, `ExecutionTrace`, `StepResult`, `PlaybackState`,
`DebuggerState`, `DebuggerAction`, `DataflowEngine`, `EngineFactory`,
`DebugConfig`, `Theme`, `TreeNodeDebugState`, `WorkflowVisualizerProps`,
`TreeSelectionType`.

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
