<div align="center">
  <img src="https://avatars.githubusercontent.com/u/207296579?s=200&v=4" alt="Plasmatic Logo" width="120" height="120">

  # @goplasmatic/dataflow-ui

  **React visualization library for dataflow-rs workflow engine**

  [![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
  [![npm](https://img.shields.io/npm/v/@goplasmatic/dataflow-ui.svg)](https://www.npmjs.com/package/@goplasmatic/dataflow-ui)
  [![TypeScript](https://img.shields.io/badge/TypeScript-5.0+-blue.svg)](https://www.typescriptlang.org/)
</div>

---

React components for visualizing and debugging [dataflow-rs](https://github.com/GoPlasmatic/dataflow-rs) 4.0 workflows: a tree of workflows, tasks and conditions, a flow diagram per workflow, a JSONLogic viewer, and a debugger that runs a message through the engine and steps through its trace.

## Features

- **Workflow Visualization**: tree view of workflows, tasks, groups and conditions, with a flow diagram per workflow
- **Loops and fan-outs**: a looping workflow carries a badge and its diagram shows the bound check, the counter advance and a back-edge; a `for_each` task shows its element and concurrency
- **Execution Debugging**: run a message, then step through the trace; the context at each step is rebuilt by replaying the trace's changes through the engine
- **Failing runs render**: a stopped, timed-out or failing run shows its status, the step that failed and each step's errors
- **Typed to the engine**: the wire types (`Message`, `Trace`, `Run`, `Issue` and the rest) come from `@goplasmatic/dataflow-wasm`, generated from the crate
- **JSONLogic Viewer** via [@goplasmatic/datalogic-ui](https://www.npmjs.com/package/@goplasmatic/datalogic-ui)
- **Theme Support**: light, dark and system
- **Monaco Editor Integration**: JSON editing with syntax highlighting

Logic editors and workflow diagrams load when opened, with a loading state
while their code arrives. The debugger keeps one engine replay cursor and
requests selected contexts on demand. When copying a built library, keep
its generated `chunks/` directory alongside `index.js` and `index.cjs`.

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

Viewing definitions needs no engine. Debugging does.

## Components

### WorkflowVisualizer

The main component for displaying workflows.

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

Debug mode runs the message through an engine and steps through the trace. The engine's module must be instantiated **once** before anything runs, and before a trace is rendered: `defaultEngineFactory` builds a `WasmEngineAdapter` over `@goplasmatic/dataflow-wasm`, and the debugger replays every trace through that package's `contexts`, with a custom engine too. Withhold `engineFactory` until the default export resolves; until then the run button is disabled:

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

`debugPayload` is the **payload**, passed to the engine as the object it is. It is not part of the JSONLogic evaluation context, so the workflow needs a `parse_json` task (`{"source": "payload", "target": "input"}`) before any expression can read it as `data.input.…`; `parse_json` copies an object payload as it is.

`onExecutionComplete` fires on every status with the engine's `RunResult`: a run that stopped or timed out completes too, so read `result.run.status`. `onExecutionError` fires when a run could not start: the engine refused a definition (`code: "REJECTED"`, with its `issues`) or an argument (`"INVALID_ARGUMENT"`), a custom engine's trace could not be replayed (`"INVALID_TRACE"`), or something else threw (`"ERROR"`).

The visualizer header shows the debug controls (play, pause, step forward and backward) when `debugConfig.enabled` is true.

#### What the debugger shows

- The toolbar reads `run.status` and the final message's `errors`.
- The context at a step is replayed on demand from the trace through a `TraceReplay` cursor (`contextAt(i)` in `useDebugger()`, or `contexts(trace)[i]` in batch): `trace.initial` before the first step, then the context after step `i`. There is no per-step message snapshot.
- A step shows its `status` and its own `errors`; a `failed` step, and an executed step that recorded errors, render as errors. A workflow whose condition or rollout refused it renders as skipped.
- A `map` step shows the context after each of its mappings (`mappingContextAt(i, m)` in `useDebugger()`, or `mapping_contexts(trace, i)` in batch).
- At the end, the final message's `errors` and `audit_trail`.

### Custom Engine

Use a custom engine, such as a server-side dataflow-rs with its registered functions, by implementing `DataflowEngine`. It carries the wasm package's `RunResult` and nothing of its own:

```tsx
import type { RunOptions, RunResult } from '@goplasmatic/dataflow-wasm';
import { WorkflowVisualizer, type DataflowEngine, type EngineFactory, type Workflow } from '@goplasmatic/dataflow-ui';

class ServerEngine implements DataflowEngine {
  constructor(private workflows: Workflow[]) {}

  async run(payload: unknown, options?: RunOptions): Promise<RunResult> {
    const response = await fetch('/run', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      // The debugger needs a trace to step through: ask for one unless the caller said otherwise.
      body: JSON.stringify({ workflows: this.workflows, payload, options: { trace: true, ...options } }),
    });
    // A Rust host's Message, Run and Trace serialize to the package's wire types.
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

`run` must resolve on every status, as the wasm engine does: a stopped run resolves with `run.status: "stopped"` and its trace. The factory may throw an `EngineError` for a refused definition; the debugger shows its `issues`. `@goplasmatic/dataflow-wasm` must still be initialised, since the debugger replays the trace through it.

### Engine version handshake

`WasmEngineAdapter` calls `assertEngineVersion()` on construction and throws when the loaded wasm engine is older than this UI build. Versions compare as semver with any pre-release tag dropped. An older engine reads a definition key it predates as an unknown key, kept in `ext` and reported, so a workflow using a newer feature loads and does something other than what it says. A newer engine passes.

## Definitions

`Workflow` and `Task` carry the 4.0 policies, `on_failure` and `on_rejection`, beside the 3.x spellings `continue_on_error` and `halt_on`, which the engine still reads and rewrites. Cards and flow nodes render whichever is present. A key the engine does not know ends up in the node's `ext`; the folder tree reads `workflow.ext?.path ?? workflow.path`.

```ts
interface Workflow {
  on_failure?: 'fail' | 'continue';            // what a task failure does to the run; "fail" stops it
  continue_on_error?: boolean;                 // 3.x: true is on_failure "continue"
  ext?: Record<string, unknown>;               // keys the engine does not know, such as `path`
  // ...
}

interface Task {
  on_failure?: 'fail' | 'halt' | 'continue';   // "fail" passes to the workflow's policy; "halt" ends the workflow
  on_rejection?: 'continue' | 'halt';          // a validation that failed its rules; "continue" by default
  continue_on_error?: boolean;                 // 3.x
  halt_on?: 'never' | 'failure';               // 3.x
  ext?: Record<string, unknown>;
  // ...
}
```

The built-in function names and the loop defaults come from the engine (`engine_facts()`), not from a list kept here. `validation` is the one spelling the badge names; `validate` renders as a custom function. The three 3.x integration configs (`http_call`, `enrich`, `publish_kafka`) left the engine; a host that registers them renders them as its own functions.

## Exports

### Components
- `WorkflowVisualizer`: main visualization component with integrated debug controls
- `TreeView`, `RulesListView`, `WorkflowFlowView`: standalone views
- `WorkflowCard`, `TaskRow`, `FunctionTypeBadge`, `ConditionBadge`: card primitives
- `DebuggerProvider`: debug state context provider (for advanced use cases)
- `DebuggerControls`, `IntegratedDebugToolbar`: playback controls
- `MessageInputPanel`, `MessageStatePanel`: debug input and state panels
- `DebugInfoBubble`, `DebugStateBadge`: per-node debug indicators
- `JsonViewer`, `SearchInput`, `ErrorBoundary`: common building blocks

### Hooks
- `useTheme`, `ThemeProvider`: theme state
- `useDebugger`: debugger state and controls (throws outside a provider)
- `useDebuggerOptional`: as above, returns `null` outside a provider
- `useTreeNodeDebugState`: debug state for any tree node
- `useWorkflowDebugState`, `useWorkflowConditionDebugState`: per-workflow state
- `useTaskDebugState`, `useTaskConditionDebugState`: per-task state

### Engine
- `WasmEngineAdapter`: the default engine, over `@goplasmatic/dataflow-wasm`; traces every run unless the options say otherwise
- `defaultEngineFactory`: factory producing `WasmEngineAdapter`
- `assertEngineVersion`: throws when the loaded engine predates this UI build
- `engineFacts`: the loaded engine's facts (the built-in names, the group depth, the loop and fan-out defaults), read once after `init()`
- `DataflowEngine`, `EngineFactory`: types for custom engines

### Helpers
- Steps: `isTaskGroup`, `groupMembers`, `flattenSteps`, `countLeafSteps`
- Folders: `workflowFolderPath`, a workflow's `ext.path` as the engine keeps it, else its `path`
- Functions: `isBuiltinFunction`, `getFunctionDisplayInfo`
- Policies: `taskContinuesOnFailure`, `taskHaltsOnFailure`, `workflowContinuesOnFailure`, reading either spelling
- Loops: `loopBadgeLabel`, `loopGuardLabel`, `loopStepLabel`, `loopDescription`
- Fan-out: `forEachBadgeLabel`, `forEachDescription`
- Debug: `contextAtStep`, `stepState`, `stepErrors`, `isWorkflowStep`, `isEngineStep`, `traceIsReplayable`, `findTaskStepIndex`, `getWorkflowState`, `getTaskState`, `isTaskCurrent`, `toExecutionError`

### Types
- Definitions: `Workflow`, `Task`, `TaskGroup`, `Step`, `FunctionConfig`, `JsonLogicValue`, `MapMapping`, `MappingItem`, `MapFunctionInput`, `ValidationRule`, `ValidationFunctionInput`, `BuiltinFunctionType`, `LoopConfig`, `ForEach`, `WorkflowStatus`, `Rollout`
- The engine's wire types, re-exported from `@goplasmatic/dataflow-wasm`: `Message`, `Context`, `Change`, `AuditEntry`, `ErrorRecord`, `Trace`, `TraceStep`, `StepResult`, `Run`, `RunStatus`, `WorkflowRecord`, `Selection`, `WorkflowExit`, `DataflowError`, `Issue`, `JsonValue`, `RunOptions`, `TraceOptions`, `AuditMode`, `RunResult`, `EngineError`, `EngineFacts`, `ParseResult`
- Debugger: `DebugConfig`, `DebuggerState`, `DebuggerAction`, `DebugNodeState`, `PlaybackState`, `ExecutionError`, `Theme`, `TreeNodeDebugState`, `WorkflowVisualizerProps`, `TreeSelectionType`, `VisualizerSelection`, `InitialVisualizerSelection`

## From 3.x

| 3.x | 4.0 |
|---|---|
| `DataflowEngine.processWithTrace(payload)` | `run(payload, options?)`, resolving with the engine's `RunResult` on every status |
| `onExecutionComplete(trace: ExecutionTrace)` | `onExecutionComplete(result: RunResult)`; the trace is `result.trace` |
| `onExecutionError(error: string)` | `onExecutionError(error: ExecutionError)`, with `code`, `message` and `issues` |
| `ExecutionTrace`, `ExecutionStep` | `Trace`, `TraceStep` from the wasm package |
| `ErrorInfo`, `AuditTrail` | `ErrorRecord`, `AuditEntry` |
| `getMessageAtStep(trace, i)` | `useDebugger().contextAt(i)` (or `contextAtStep(trace, contexts, i)`): the context, replayed, not a message |
| `getChangesAtStep(trace, i)` | `trace.steps[i].changes` |
| `traceHasSnapshots(trace)` | `traceIsReplayable(trace)` |
| `createEmptyMessage`, `cloneMessage` | Removed |
| `INTEGRATION_FUNCTION_NAMES`; `validate`, `http_call`, `enrich`, `publish_kafka` in `BuiltinFunctionType` | Removed; `isBuiltinFunction` reads the engine's `engine_facts()` |
| The payload sent to the engine as a string | Sent as the object it is |

Debug mode now requires `@goplasmatic/dataflow-wasm` to be initialised even with a custom engine, since the trace is replayed through it.

## Peer Dependencies

- React 18.x or 19.x
- React DOM 18.x or 19.x

## Related Packages

- [dataflow-rs](https://crates.io/crates/dataflow-rs) - Core Rust workflow engine
- [@goplasmatic/dataflow-wasm](https://www.npmjs.com/package/@goplasmatic/dataflow-wasm) - WebAssembly bindings
- [@goplasmatic/datalogic-ui](https://www.npmjs.com/package/@goplasmatic/datalogic-ui) - JSONLogic visualization

## License

This project is licensed under the Apache License, Version 2.0. See the [LICENSE](../LICENSE) file for details.
