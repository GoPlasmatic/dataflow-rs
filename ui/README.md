<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  # @goplasmatic/dataflow-ui

  **React visualization and debugging library for the dataflow-rs workflow engine**

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![npm](https://img.shields.io/npm/v/@goplasmatic/dataflow-ui.svg)](https://www.npmjs.com/package/@goplasmatic/dataflow-ui)
  [![TypeScript](https://img.shields.io/badge/TypeScript-5.0+-blue.svg)](https://www.typescriptlang.org/)
</div>

---

React components for visualizing and debugging [dataflow-rs](https://github.com/GoPlasmatic/dataflow-rs) 4.0 workflows: an interactive tree of workflows, tasks, and conditions; a flow diagram per workflow; a JSONLogic viewer; and a step-by-step debugger that runs messages through the engine and replays execution traces.

## Features

- **Workflow Visualization**: Tree view of workflows, tasks, groups, and conditions, plus an interactive flow diagram for each workflow.
- **Loops and Fan-Outs**: Looping workflows display a badge along with their iteration guard, counter update, and back-edge in the diagram; `for_each` tasks display their source array, element binding, and concurrency limit.
- **Step-by-Step Debugging**: Run a message and step forward or backward through the trace. The context at each step is reconstructed on demand by replaying trace diffs through `@goplasmatic/dataflow-wasm`.
- **Error Inspection**: Stopped, timed-out, or failed runs highlight their status, the failing step, and any recorded error details.
- **End-to-End TypeScript Types**: Wire types (`Message`, `Trace`, `Run`, `Issue`, and more) are imported directly from `@goplasmatic/dataflow-wasm`.
- **JSONLogic Viewer**: Visual expression trees powered by [@goplasmatic/datalogic-ui](https://www.npmjs.com/package/@goplasmatic/datalogic-ui).
- **Theme Support**: Light, dark, and system themes.
- **Monaco Editor Integration**: Built-in JSON editing with syntax highlighting.

Heavy subcomponents (such as Monaco editors and workflow diagrams) are lazy-loaded when opened. When copying a built bundle, keep the generated `chunks/` directory alongside `index.js` and `index.cjs`.

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

Viewing workflow definitions does not require initializing the WebAssembly engine; interactive debugging does.

## Components

### WorkflowVisualizer

The primary component for displaying and debugging workflows.

```tsx
interface WorkflowVisualizerProps {
  workflows: Workflow[];
  initialSelection?: InitialVisualizerSelection;
  selection?: VisualizerSelection | null;
  onSelectionChange?: (selection: VisualizerSelection | null) => void;
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
  onExecutionComplete?: (result: RunResult) => void;
  onExecutionError?: (error: ExecutionError) => void;
}
```

### Debug Mode

Debug mode executes a message against an engine and lets you step through the resulting trace. Call `initWasm()` once before running workflows or replaying traces. `defaultEngineFactory` wraps `@goplasmatic/dataflow-wasm` in a `WasmEngineAdapter`, and the debugger uses that package's `TraceReplay` cursor even when you supply a custom remote engine. Pass `undefined` for `engineFactory` until `initWasm()` resolves so the Run button stays disabled until initialization completes:

```tsx
import { useEffect, useState } from 'react';
import initWasm from '@goplasmatic/dataflow-wasm';
import { WorkflowVisualizer, defaultEngineFactory } from '@goplasmatic/dataflow-ui';

function DebugView() {
  const [ready, setReady] = useState(false);
  useEffect(() => {
    initWasm().then(() => setReady(true));
  }, []);

  return (
    <WorkflowVisualizer
      workflows={workflows}
      debugConfig={{
        enabled: true,
        engineFactory: ready ? defaultEngineFactory : undefined,
        autoExecute: true,
        onExecutionComplete: ({ message, run, trace }) => console.log(run.status, message.errors),
        onExecutionError: (error) => console.error(error.code, error.message, error.issues),
      }}
      debugPayload={{ input: 'hello' }}
    />
  );
}
```

`debugPayload` sets the initial message **payload**. Because `payload` is not part of the JSONLogic evaluation context (`data`, `metadata`, `temp_data`), your workflow should include a `parse_json` task (`{"source": "payload", "target": "input"}`) so subsequent tasks can read `data.input.*`.

- `onExecutionComplete` is called whenever a run finishes—including runs with status `"stopped"`, `"timed_out"`, or `"completed_with_errors"`. Check `result.run.status` to see how the run ended.
- `onExecutionError` is called when a run could not start or its trace could not be loaded: for example, if a definition was rejected (`code: "REJECTED"`, with `error.issues`), an argument was invalid (`"INVALID_ARGUMENT"`), a custom engine returned an invalid trace (`"INVALID_TRACE"`), or an unexpected exception occurred (`"ERROR"`).

When `debugConfig.enabled` is `true`, playback controls (play, pause, step forward, step backward) appear in the visualizer header.

#### What the Debugger Displays

- **Toolbar**: Displays `run.status` and any errors on the final message.
- **Step Context**: Reconstructs the context on demand via `TraceReplay` (`contextAt(i)` in `useDebugger()`, or `contexts(trace)[i]` in batch), showing `trace.initial` before the first step and the updated context after each step `i`.
- **Step Status & Errors**: Shows each step's HTTP-style `status` code and any `errors` recorded during that step. Failed steps and executed steps that recorded errors are highlighted in red; workflows skipped by condition or rollout are shown as skipped.
- **Per-Mapping Inspection**: For `map` tasks, lets you inspect the context after each individual mapping (`mappingContextAt(i, m)` in `useDebugger()`, or `mapping_contexts(trace, i)` in batch).
- **Final State**: Displays the final message's `errors` and `audit_trail`.

### Custom Engine (Server-Side Execution)

To debug workflows against a backend `dataflow-rs` service (for example, when workflows call custom Rust host functions), implement `DataflowEngine`:

```tsx
import type { RunOptions, RunResult } from '@goplasmatic/dataflow-wasm';
import { WorkflowVisualizer, type DataflowEngine, type EngineFactory, type Workflow } from '@goplasmatic/dataflow-ui';

class ServerEngine implements DataflowEngine {
  constructor(private workflows: Workflow[]) {}

  async run(payload: unknown, options?: RunOptions): Promise<RunResult> {
    const response = await fetch('/run', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      // Request a trace by default so the debugger can step through execution.
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

Your `run` implementation should resolve on every run status (including `"stopped"` and `"timed_out"`) with the `RunResult` wire format, and throw an `EngineError` if workflow definitions are rejected. Note that `@goplasmatic/dataflow-wasm` must still be initialized with `initWasm()` because the UI uses its `TraceReplay` engine to replay trace steps locally.

### Engine Version Check

`WasmEngineAdapter` calls `assertEngineVersion()` when constructed and throws an error if the loaded `@goplasmatic/dataflow-wasm` binary is older than the `@goplasmatic/dataflow-ui` package version. Newer WASM minor/patch versions are accepted.

## Workflow & Task Definitions

`Workflow` and `Task` support both 4.0 policy fields (`on_failure`, `on_rejection`) and legacy 3.x fields (`continue_on_error`, `halt_on`). Unrecognized properties are stored in `ext` (for example, the folder tree checks `workflow.ext?.path ?? workflow.path`).

```ts
interface Workflow {
  on_failure?: 'fail' | 'continue';            // "fail" (default) stops the run; "continue" moves to the next workflow
  continue_on_error?: boolean;                 // 3.x equivalent: true maps to on_failure: "continue"
  ext?: Record<string, unknown>;               // unrecognized keys preserved by the parser (e.g., `path`)
  // ...
}

interface Task {
  on_failure?: 'fail' | 'halt' | 'continue';   // "fail" delegates to workflow policy; "halt" stops current workflow
  on_rejection?: 'continue' | 'halt';          // behavior on 4xx (such as failed validation); "continue" by default
  continue_on_error?: boolean;                 // 3.x equivalent
  halt_on?: 'never' | 'failure';               // 3.x equivalent
  ext?: Record<string, unknown>;
  // ...
}
```

Built-in function names and loop/fan-out defaults are queried dynamically from `engine_facts()`.

## Exports

### Components
- `WorkflowVisualizer`: Main visualization component with integrated debug controls
- `TreeView`, `RulesListView`, `WorkflowFlowView`: Standalone views
- `WorkflowCard`, `TaskRow`, `FunctionTypeBadge`, `ConditionBadge`: Card primitives
- `DebuggerProvider`: Debug state context provider (for custom layouts)
- `DebuggerControls`, `IntegratedDebugToolbar`: Playback controls
- `MessageInputPanel`, `MessageStatePanel`: Debug input and state panels
- `DebugInfoBubble`, `DebugStateBadge`: Per-node debug indicators
- `JsonViewer`, `SearchInput`, `ErrorBoundary`: Shared UI primitives

### Hooks
- `useTheme`, `ThemeProvider`: Theme state management
- `useDebugger`: Access debugger state and controls (throws if used outside `DebuggerProvider`)
- `useDebuggerOptional`: Same as `useDebugger`, but returns `null` outside a provider
- `useTreeNodeDebugState`: Debug state for any tree node
- `useWorkflowDebugState`, `useWorkflowConditionDebugState`: Workflow-level debug state
- `useTaskDebugState`, `useTaskConditionDebugState`: Task-level debug state

### Engine
- `WasmEngineAdapter`: Default engine adapter backed by `@goplasmatic/dataflow-wasm`
- `defaultEngineFactory`: Factory function returning a `WasmEngineAdapter`
- `assertEngineVersion`: Verifies that the loaded WASM module version is compatible with this UI build
- `engineFacts`: Cached engine metadata (`engine_facts()`), available after `initWasm()`
- `DataflowEngine`, `EngineFactory`: Interfaces for custom engine implementations

### Helpers
- Steps: `isTaskGroup`, `groupMembers`, `flattenSteps`, `countLeafSteps`
- Folders: `workflowFolderPath` (reads `workflow.ext?.path ?? workflow.path`)
- Functions: `isBuiltinFunction`, `getFunctionDisplayInfo`
- Policies: `taskContinuesOnFailure`, `taskHaltsOnFailure`, `workflowContinuesOnFailure` (supports both 4.0 and 3.x fields)
- Loops: `loopBadgeLabel`, `loopGuardLabel`, `loopStepLabel`, `loopDescription`
- Fan-out: `forEachBadgeLabel`, `forEachDescription`
- Debug: `contextAtStep`, `stepState`, `stepErrors`, `isWorkflowStep`, `isEngineStep`, `traceIsReplayable`, `findTaskStepIndex`, `getWorkflowState`, `getTaskState`, `isTaskCurrent`, `toExecutionError`

### Types
- Definitions: `Workflow`, `Task`, `TaskGroup`, `Step`, `FunctionConfig`, `JsonLogicValue`, `MapMapping`, `MappingItem`, `MapFunctionInput`, `ValidationRule`, `ValidationFunctionInput`, `BuiltinFunctionType`, `LoopConfig`, `ForEach`, `WorkflowStatus`, `Rollout`
- Re-exported wire types from `@goplasmatic/dataflow-wasm`: `Message`, `Context`, `Change`, `AuditEntry`, `ErrorRecord`, `Trace`, `TraceStep`, `StepResult`, `Run`, `RunStatus`, `WorkflowRecord`, `Selection`, `WorkflowExit`, `DataflowError`, `Issue`, `JsonValue`, `RunOptions`, `TraceOptions`, `AuditMode`, `RunResult`, `EngineError`, `EngineFacts`, `ParseResult`
- Debugger & UI: `DebugConfig`, `DebuggerState`, `DebuggerAction`, `DebugNodeState`, `PlaybackState`, `ExecutionError`, `Theme`, `TreeNodeDebugState`, `WorkflowVisualizerProps`, `TreeSelectionType`, `VisualizerSelection`, `InitialVisualizerSelection`

## Migrating from 3.x

| 3.x | 4.0 |
|---|---|
| `DataflowEngine.processWithTrace(payload)` | `run(payload, options?)`, resolving with `RunResult` on every status |
| `onExecutionComplete(trace: ExecutionTrace)` | `onExecutionComplete(result: RunResult)` (`result.trace`) |
| `onExecutionError(error: string)` | `onExecutionError(error: ExecutionError)` with `code`, `message`, and `issues` |
| `ExecutionTrace`, `ExecutionStep` | `Trace`, `TraceStep` from `@goplasmatic/dataflow-wasm` |
| `ErrorInfo`, `AuditTrail` | `ErrorRecord`, `AuditEntry` |
| `getMessageAtStep(trace, i)` | `useDebugger().contextAt(i)` or `contextAtStep(trace, contexts, i)` (returns the replayed `Context`) |
| `getChangesAtStep(trace, i)` | `trace.steps[i].changes` |
| `traceHasSnapshots(trace)` | `traceIsReplayable(trace)` |
| `createEmptyMessage`, `cloneMessage` | Removed |
| `INTEGRATION_FUNCTION_NAMES`; `validate`, `http_call`, `enrich`, `publish_kafka` in `BuiltinFunctionType` | Removed; `isBuiltinFunction` queries `engine_facts()` |
| Payload serialized to a JSON string before execution | Passed directly as a JavaScript value |

Debug mode now requires `@goplasmatic/dataflow-wasm` to be initialized even when using a custom server engine, because trace replay is performed by `TraceReplay` in WASM.

## Peer Dependencies

- React 18.x or 19.x
- React DOM 18.x or 19.x

## Related Packages

- [dataflow-rs](https://crates.io/crates/dataflow-rs) — Core Rust workflow engine
- [@goplasmatic/dataflow-wasm](https://www.npmjs.com/package/@goplasmatic/dataflow-wasm) — WebAssembly bindings
- [@goplasmatic/datalogic-ui](https://www.npmjs.com/package/@goplasmatic/datalogic-ui) — JSONLogic visualizer

## License

This project is licensed under the Apache License, Version 2.0. See the [LICENSE](../LICENSE) file for details.
