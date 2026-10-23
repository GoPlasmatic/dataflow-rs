# UI Package

The `@goplasmatic/dataflow-ui` package provides React components for visualizing and debugging `dataflow-rs` 4.0 workflows: an interactive tree of workflows, tasks, and conditions; a flow diagram for each workflow; and a step-by-step debugger that runs messages through the engine and replays execution traces.

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

Viewing workflow definitions does not require an engine instance; interactive debugging does (see [Debug Mode](#debug-mode)).

## Components

### `WorkflowVisualizer`

The primary component for browsing, visualizing, and debugging workflows.

```tsx
interface WorkflowVisualizerProps {
  /** Array of workflow definitions to display */
  workflows: Workflow[];
  /** Initial selection when mounted in uncontrolled mode (default: none) */
  initialSelection?: InitialVisualizerSelection;
  /** Controlled selection state */
  selection?: VisualizerSelection | null;
  /** Callback fired when the selection changes */
  onSelectionChange?: (selection: VisualizerSelection | null) => void;
  /** Callback fired when a workflow is selected */
  onWorkflowSelect?: (workflow: Workflow) => void;
  /** Callback fired when a task is selected */
  onTaskSelect?: (task: Task, workflow: Workflow) => void;
  /** Color theme: 'light', 'dark', or 'system' */
  theme?: Theme;
  /** Additional CSS class for the root container */
  className?: string;
  /** Optional processed message to display in the result panel */
  executionResult?: Message | null;
  /** Integrated step-by-step debugger configuration; see Debug Mode below */
  debugConfig?: DebugConfig;
  /** Input payload for debugging (overrides debugConfig.initialPayload) */
  debugPayload?: Record<string, unknown>;
}
```

### `TreeView`

Standalone tree navigation component for custom layouts:

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

Pass `debugConfig` to `WorkflowVisualizer` to enable integrated step-by-step execution and trace playback. When `debugConfig.enabled` is `true`, `WorkflowVisualizer` sets up its own `DebuggerProvider` and renders playback controls in the header:

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
        onExecutionComplete: ({ message, run, trace }) => console.log(run.status, trace?.steps.length, 'steps'),
        onExecutionError: (error) => console.error(error.code, error.message, error.issues),
      }}
      debugPayload={{ greeting: 'hello' }}
    />
  );
}
```

- `engineFactory` provides the engine used to execute messages (when `undefined`, the Run button stays disabled). `defaultEngineFactory` uses `@goplasmatic/dataflow-wasm`.
- `debugPayload` is passed to the engine as the message **`payload`**. Because `payload` is separate from `{ data, metadata, temp_data }`, your workflow should start with a `parse_json` task (`{"source": "payload", "target": "input"}`) so expressions can read `data.input.*`.

### Initializing the WASM Module

Before running workflows or replaying traces, you must initialize `@goplasmatic/dataflow-wasm` once by awaiting its default export (`initWasm()`). This is required both when using `defaultEngineFactory` and when using a custom server engine, because the debugger replays trace diffs locally through `@goplasmatic/dataflow-wasm`'s `TraceReplay`.

Pass `undefined` for `engineFactory` until `initWasm()` resolves so the Run button remains disabled until the module is ready:

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

### `DebugConfig` and `ExecutionError`

```tsx
interface DebugConfig {
  enabled: boolean;
  engineFactory?: EngineFactory;
  initialPayload?: Record<string, unknown>;
  autoExecute?: boolean;                                   // default: false
  onExecutionComplete?: (result: RunResult) => void;
  onExecutionError?: (error: ExecutionError) => void;
}

interface ExecutionError {
  code: string;        // "REJECTED", "INVALID_ARGUMENT", "INVALID_TRACE", or "ERROR"
  message: string;
  issues?: Issue[];    // Present when `code === "REJECTED"`: all definition issues with JSON paths
}
```

- `onExecutionComplete` fires whenever a run finishes—even if it stopped on an error or timed out (`result.run.status` indicates how the run ended).
- `onExecutionError` fires when a run could not start (for example, if a definition was rejected with `"REJECTED"`, an argument was malformed, or a custom engine returned an invalid trace).

### What the Debugger Shows

A `dataflow-rs` 4.0 trace stores `initial` (the context before any workflow ran) and one `TraceStep` per task call, skip, failure, or engine write along with its `changes`. Instead of storing full message snapshots at every step, the UI reconstructs the context on demand using a `TraceReplay` cursor from `@goplasmatic/dataflow-wasm`:

- **Toolbar** — Displays `run.status` and any errors on the final message.
- **Step Context** — `contextAt(i)` reconstructs the context after step `i` (and `trace.initial` at index `-1` before the first step), displayed alongside the step's status code and error records.
- **Failed & Skipped Steps** — Steps with `result: "failed"` or executed steps that recorded errors (such as failed `validation` rules) are highlighted as errors. Workflows skipped due to a false condition or rollout mismatch are shown as skipped.
- **Per-Mapping Inspection** — For `map` tasks, `mappingContextAt(i, m)` displays the context after each individual mapping.
- **Final State** — Shows the final message's `errors` and `audit_trail`.

If a trace exceeded its `max_bytes` budget (`trace.truncated === true`), diffs are incomplete and `traceIsReplayable(trace)` returns `false`; the state panel displays `trace.initial` and notes that the trace was truncated.

### Using `debugConfig` vs. `DebuggerProvider`

`WorkflowVisualizer` enables debug mode only when `debugConfig.enabled` is `true`, creating its own internal `DebuggerProvider`. Wrapping `WorkflowVisualizer` in an external `<DebuggerProvider>` without setting `debugConfig` will not enable debug mode on the visualizer, and any external debug panels placed outside `WorkflowVisualizer` will not share state with the visualizer's internal provider.

- For standard use, pass `debugConfig` to `WorkflowVisualizer` and use its built-in toolbar and panels.
- For a completely custom layout, compose the standalone components (`TreeView`, `WorkflowFlowView`, `DebuggerControls`, `MessageInputPanel`, `MessageStatePanel`) inside a single `<DebuggerProvider>`.

### Engine Version Check

When `WasmEngineAdapter` is constructed, it calls `assertEngineVersion()` and throws an error if the loaded `@goplasmatic/dataflow-wasm` module is older than the `@goplasmatic/dataflow-ui` build. Newer WASM versions are accepted.

## Custom Engine (Server-Side Execution)

To run workflows on a Rust backend (for example, when workflows invoke custom server-side host functions) while stepping through the resulting trace in the UI, implement `DataflowEngine`:

```tsx
import type { RunOptions, RunResult } from '@goplasmatic/dataflow-wasm';

interface DataflowEngine {
  /** Execute a message and resolve with `{ message, run, trace }`. */
  run(payload: unknown, options?: RunOptions): Promise<RunResult>;
  /** Release any resources when workflows change or the debugger unmounts. */
  dispose?(): void;
}

type EngineFactory = (workflows: Workflow[]) => DataflowEngine;
```

Example implementation calling a backend `/run` endpoint:

```tsx
import type { RunOptions, RunResult } from '@goplasmatic/dataflow-wasm';
import {
  WorkflowVisualizer,
  type DataflowEngine,
  type EngineFactory,
  type Workflow,
} from '@goplasmatic/dataflow-ui';

class ServerEngine implements DataflowEngine {
  constructor(private workflows: Workflow[]) {}

  async run(payload: unknown, options?: RunOptions): Promise<RunResult> {
    const response = await fetch('/run', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      // Always request a trace unless the caller explicitly disabled it.
      body: JSON.stringify({ workflows: this.workflows, payload, options: { trace: true, ...options } }),
    });
    // Rust `Message`, `Run`, and `Trace` serialize directly to the expected wire types.
    return (await response.json()) as RunResult;
  }

  dispose() {}
}

const serverEngineFactory: EngineFactory = (workflows) => new ServerEngine(workflows);

function CustomDebugView() {
  return (
    <WorkflowVisualizer
      workflows={workflows}
      debugConfig={{ enabled: true, engineFactory: serverEngineFactory, autoExecute: true }}
      debugPayload={{ input: 'test' }}
    />
  );
}
```

Requirements for a custom `DataflowEngine`:
1. `run` must resolve on every run status (including `"stopped"` and `"timed_out"`) with `{ message, run, trace }`.
2. If workflow definitions are rejected, throw an `EngineError` with `code: "REJECTED"` and `issues` so the UI can display them.
3. `@goplasmatic/dataflow-wasm` must still be initialized with `initWasm()`, as the UI uses its `TraceReplay` cursor to replay the trace.

### `DebuggerControls` and `useDebugger`

For custom layouts under a `<DebuggerProvider>`, use `<DebuggerControls />` and the `useDebugger()` hook:

```tsx
import { useDebugger } from '@goplasmatic/dataflow-ui';

function MyComponent() {
  const {
    state,
    currentStep,
    currentContext,
    contextAt,
    mappingContextAt,
    stepForward,
    stepBackward,
    goToStep,
    play,
    pause,
    reset,
  } = useDebugger();

  // state: DebuggerState
  //   trace: Trace | null           Trace of the last run
  //   run: Run | null               Run status and per-workflow summary
  //   message: Message | null       Final message after the run
  //   replay: TraceReplay | null    On-demand WASM trace replay cursor
  //   currentStepIndex: number      Current step index (-1 is initial context)
  //   executionError: ExecutionError | null
  //   isExecuting, playbackState, playbackSpeed, inputPayload, skipFailedConditions

  const step = currentStep;
  const context = currentContext; // or contextAt(i) for any step (-1 is initial)

  // ...
}
```

`useDebugger()` throws if called outside a `DebuggerProvider`; use `useDebuggerOptional()` (which returns `null` outside a provider) for components that render in both static and debug modes.

## Workflow and Task Definition Types

`Workflow` and `Task` support both 4.0 policy fields (`on_failure`, `on_rejection`) and legacy 3.x fields (`continue_on_error`, `halt_on`). Unrecognized keys are preserved in `ext` (for example, the folder tree checks `workflow.ext?.path ?? workflow.path`):

```ts
interface Workflow {
  on_failure?: 'fail' | 'continue';            // "fail" (default) stops the run; "continue" moves to the next workflow
  continue_on_error?: boolean;                 // 3.x equivalent: true maps to on_failure: "continue"
  ext?: Record<string, unknown>;               // Unrecognized keys preserved by the parser (e.g., `path`)
  // ...
}

interface Task {
  on_failure?: 'fail' | 'halt' | 'continue';   // "fail" escalates to workflow policy; "halt" ends current workflow
  on_rejection?: 'continue' | 'halt';          // Behavior on 4xx rejection (e.g., failed validation); "continue" by default
  continue_on_error?: boolean;                 // 3.x equivalent
  halt_on?: 'never' | 'failure';               // 3.x equivalent
  ext?: Record<string, unknown>;
  // ...
}
```

Helper functions (`taskContinuesOnFailure`, `taskHaltsOnFailure`, `workflowContinuesOnFailure`) read policies from either 4.0 or 3.x fields, and built-in function metadata is loaded dynamically from `engine_facts()`.

## Theming

`WorkflowVisualizer` supports `'light'`, `'dark'`, and `'system'` themes:

```tsx
// Light theme
<WorkflowVisualizer workflows={workflows} theme="light" />

// Dark theme
<WorkflowVisualizer workflows={workflows} theme="dark" />

// System preference (default)
<WorkflowVisualizer workflows={workflows} theme="system" />
```

Access or update the current theme from custom components with `useTheme()`:

```tsx
import { useTheme } from '@goplasmatic/dataflow-ui';

function MyComponent() {
  const { theme, setTheme, resolvedTheme } = useTheme();
  // resolvedTheme is always 'light' or 'dark'
}
```

## Package Exports

### Components

| Export | Description |
|--------|---------|
| `WorkflowVisualizer` | Main visualization component with optional integrated debugger |
| `TreeView` | Standalone workflow and task tree view |
| `RulesListView` | Flat list view of workflows |
| `WorkflowFlowView` | Interactive flow diagram for a single workflow |
| `WorkflowCard`, `TaskRow` | Card and row UI primitives |
| `FunctionTypeBadge`, `ConditionBadge` | Badges displaying a task's function type and condition |
| `DebuggerControls` | Standalone playback controls |
| `IntegratedDebugToolbar` | Toolbar used inside `WorkflowVisualizer` debug mode |
| `MessageInputPanel`, `MessageStatePanel` | Input payload editor and context state inspector panels |
| `DebugInfoBubble`, `DebugStateBadge` | Per-node execution status indicators |
| `JsonViewer`, `SearchInput`, `ErrorBoundary` | Shared UI building blocks |

### Providers and Hooks

| Export | Description |
|--------|---------|
| `ThemeProvider`, `useTheme` | Theme context provider and hook |
| `DebuggerProvider`, `useDebugger` | Debugger state provider and hook |
| `useDebuggerOptional` | Same as `useDebugger`, but returns `null` outside a `DebuggerProvider` |
| `useTreeNodeDebugState` | Debug state hook for any tree node |
| `useWorkflowDebugState`, `useWorkflowConditionDebugState` | Per-workflow debug state hooks |
| `useTaskDebugState`, `useTaskConditionDebugState` | Per-task debug state hooks |

### Engine Adapters

| Export | Description |
|--------|---------|
| `WasmEngineAdapter` | Default `DataflowEngine` implementation backed by `@goplasmatic/dataflow-wasm` |
| `defaultEngineFactory` | Factory function returning a `WasmEngineAdapter` |
| `assertEngineVersion` | Verifies that the loaded WASM module version is compatible with this UI build |
| `engineFacts` | Cached `engine_facts()` metadata from the WASM module (available after `initWasm()`) |
| `DataflowEngine`, `EngineFactory` | Interfaces for custom engine implementations |

### Helpers and Types

- **Steps:** `isTaskGroup`, `groupMembers`, `flattenSteps`, `countLeafSteps`
- **Folders:** `workflowFolderPath`
- **Functions:** `isBuiltinFunction`, `getFunctionDisplayInfo`
- **Policies:** `taskContinuesOnFailure`, `taskHaltsOnFailure`, `workflowContinuesOnFailure`
- **Loops & Fan-Out:** `loopBadgeLabel`, `loopGuardLabel`, `loopStepLabel`, `loopDescription`, `forEachBadgeLabel`, `forEachDescription`
- **Debug:** `contextAtStep`, `stepState`, `stepErrors`, `isWorkflowStep`, `isEngineStep`, `traceIsReplayable`, `findTaskStepIndex`, `getWorkflowState`, `getTaskState`, `isTaskCurrent`, `toExecutionError`
- **Types:** All workflow/task definition types, debugger state types, and re-exported wire types from `@goplasmatic/dataflow-wasm`.

## Migrating from 3.x

| 3.x | 4.0 |
|---|---|
| `DataflowEngine.processWithTrace(payload)` | `run(payload, options?)` resolving with `RunResult` on every status |
| `onExecutionComplete(trace: ExecutionTrace)` | `onExecutionComplete(result: RunResult)` (`result.trace`) |
| `onExecutionError(error: string)` | `onExecutionError(error: ExecutionError)` with `code`, `message`, and `issues` |
| `ExecutionTrace`, `ExecutionStep` | `Trace`, `TraceStep` from `@goplasmatic/dataflow-wasm` |
| `ErrorInfo`, `AuditTrail` | `ErrorRecord`, `AuditEntry` |
| `getMessageAtStep(trace, i)` | `useDebugger().contextAt(i)` or `contextAtStep(trace, contexts, i)` (returns the replayed `Context`) |
| `getChangesAtStep(trace, i)` | `trace.steps[i].changes` |
| `traceHasSnapshots(trace)` | `traceIsReplayable(trace)` |
| `createEmptyMessage`, `cloneMessage` | Removed |
| `INTEGRATION_FUNCTION_NAMES`; `validate`, `http_call`, `enrich`, `publish_kafka` in `BuiltinFunctionType` | Removed; `isBuiltinFunction` queries `engine_facts()` |
| Payload serialized as a JSON string | Passed directly as a JavaScript value |

## Building from Source

```bash
cd ui
npm install
npm run wasm:local   # build ../wasm and copy it into node_modules
npm test             # run unit and component tests against the local WASM build
npm run build:lib
```

Built output is written to `ui/dist/`.

## Next Steps

- [WASM Package](../wasm/overview.md) — Run workflows in the browser and Node.js
- [Core Concepts](../core-concepts/overview.md) — Learn how workflows, tasks, and traces work
