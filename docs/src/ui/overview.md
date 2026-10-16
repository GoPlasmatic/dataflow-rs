# UI Package

The `@goplasmatic/dataflow-ui` package provides React components for
visualizing and debugging dataflow-rs 4.0 workflows: a tree of workflows,
tasks and conditions, a flow diagram per workflow, and a debugger that runs
a message through the engine and steps through its trace.

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

Viewing definitions needs no engine. Debugging does; see
[Debug Mode](#debug-mode).

## Components

### WorkflowVisualizer

The main component for displaying workflows in an interactive tree view.

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
  /** A message to display in the result panel */
  executionResult?: Message | null;
  /** Integrated debug mode; see Debug Mode below */
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

Step-by-step execution visualization. The simplest form is `debugConfig`. The
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
        onExecutionComplete: ({ message, run, trace }) => console.log(run.status, trace?.steps.length, 'steps'),
        onExecutionError: (error) => console.error(error.code, error.message, error.issues),
      }}
      debugPayload={{ greeting: 'hello' }}
    />
  );
}
```

`engineFactory` enables execution: without it the run button is disabled.
`defaultEngineFactory` uses the WASM engine from `@goplasmatic/dataflow-wasm`.

`debugPayload` is the **payload**, passed to the engine as the object it is.
It is not part of the JSONLogic evaluation context, so the workflow needs a
`parse_json` task (`{"source": "payload", "target": "input"}`) before any
expression can read it as `data.input.…`; `parse_json` copies an object
payload as it is.

### Initialising the engine

The engine's module must be instantiated **once** before anything runs, and
before a trace is rendered. `defaultEngineFactory` builds a
`WasmEngineAdapter`, which calls into `@goplasmatic/dataflow-wasm` as soon as
it is constructed; and the debugger replays every trace through that
package's `contexts`, so the requirement holds with a custom engine too. The
package is a `--target web` wasm-bindgen build: await its default export
first, or the first call throws a bare `TypeError` from the uninitialised
glue, before the version handshake below can say anything useful.

Withhold `engineFactory` until it resolves; until then the run button is
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
`defaultEngineFactory` in the shape above.

### DebugConfig

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
  code: string;        // "REJECTED", "INVALID_ARGUMENT", "INVALID_TRACE" (a custom engine's
                       // trace the package cannot replay), or "ERROR" for anything else thrown
  message: string;
  issues?: Issue[];    // for a refused definition: every issue, at its authored path
}
```

`onExecutionComplete` fires on every status with the engine's `RunResult`
(`{ message, run, trace? }`): a run that stopped or timed out completes too,
so read `result.run.status`. `onExecutionError` fires when a run could not
start: the engine refused a definition or an argument, or something else
threw on the way.

### What the debugger shows

A dataflow-rs 4.0 trace holds `initial`, the context before any workflow,
and one step per call, skip, failure and engine write, each with its
changes. There is no per-step message snapshot: the context at a step is
rebuilt by replaying the changes, which the engine does (`contexts(trace)`
from the wasm package), so the UI and the engine never disagree on the rules.

- **The toolbar** reads `run.status` and the final message's `errors`.
- **The context at step `i`** is `contexts[i]`, and before the first step
  `trace.initial`. The state panel shows it beside the step's `status` and
  `errors`.
- **A failing step** (`result: "failed"`) renders as an error, and so does an
  executed step that recorded errors, such as a `validation` whose rules
  failed. A workflow whose condition or rollout refused it renders as skipped.
  The engine's own steps (a loop sweep entered) belong to no task.
- **A `map` step** shows the context after each of its mappings
  (`mapping_contexts(trace, i)`).
- **At the end**, the final message's `errors` and `audit_trail`.

A trace whose byte budget was crossed (`trace.truncated`) is not replayed: the
debugger shows `trace.initial` at every step and says so in the state panel;
`traceIsReplayable(trace)` is the test.

### `debugConfig` is what turns debug mode on

Wrapping `WorkflowVisualizer` in a `DebuggerProvider` does **not** enable debug
mode. The visualizer derives it from `debugConfig.enabled` alone, and when that
is true it creates its *own* `DebuggerProvider` internally, and that provider
shadows any ambient one for everything it renders.

The practical consequences:

- `<DebuggerProvider><WorkflowVisualizer workflows={…} /></DebuggerProvider>`
  with no `debugConfig` renders in plain, non-debug mode.
- Panels you place **outside** the visualizer read the ambient provider, while
  the visualizer reads its own. Two providers means two independent states, so
  the external panels will not follow the visualizer's playback.

To drive your own layout, use the components standalone under one provider and
leave the visualizer out of the debug path, or keep everything inside
`debugConfig` and let the built-in toolbar drive it.

### Engine version handshake

`WasmEngineAdapter` calls `assertEngineVersion()` on construction and **throws**
when the loaded WASM engine is older than the UI build expects. Versions
compare as semver with any pre-release tag dropped. Do not catch it blindly:
an older engine reads a definition key it predates as an unknown key, kept in
the node's `ext` and reported, so a workflow using a newer feature loads and
does something other than what it says. A newer engine passes silently, since
the package declares a caret range on the wasm dependency.

## Custom Engine

Use a custom engine, such as a server-side dataflow-rs with its registered
functions, by implementing `DataflowEngine`. The interface carries the wasm
package's `RunResult` and nothing of its own:

```tsx
import type { RunOptions, RunResult } from '@goplasmatic/dataflow-wasm';

interface DataflowEngine {
  /** Run one message and resolve with the message, the run and its trace. */
  run(payload: unknown, options?: RunOptions): Promise<RunResult>;
  /** Release the engine's resources. Called when the workflows change. */
  dispose?(): void;
}

type EngineFactory = (workflows: Workflow[]) => DataflowEngine;
```

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

Three rules for a custom engine:

- `run` resolves on every status, as the wasm engine does. A stopped run
  resolves with `run.status: "stopped"` and its trace; only a malformed
  argument rejects.
- The factory may throw an `EngineError` for a refused definition; the
  debugger shows its `issues`.
- `@goplasmatic/dataflow-wasm` must still be initialised, since the debugger
  replays the trace through it.

`DebuggerProvider` calls `engineFactory` whenever workflows change, so the
engine always has the latest workflow definitions, and calls `dispose` on the
previous one.

### Debugger Controls

```tsx
import { DebuggerControls } from '@goplasmatic/dataflow-ui';

// Provides playback controls: play, pause, step forward/back, reset
<DebuggerControls />
```

### useDebugger Hook

Access debugger state programmatically. The state is the engine's result and
the replay of its trace:

```tsx
import { useDebugger, contextAtStep } from '@goplasmatic/dataflow-ui';

function MyComponent() {
  const { state, stepForward, stepBackward, goToStep, play, pause, reset } = useDebugger();

  // state: DebuggerState
  //   trace: Trace | null           the trace of the last run
  //   run: Run | null               how it ended and what each workflow did
  //   message: Message | null       the message after the run
  //   contexts: Context[]           the context after each step, replayed once per run
  //   currentStepIndex: number      -1 before the first step
  //   executionError: ExecutionError | null
  //   isExecuting, playbackState, playbackSpeed, inputPayload, skipFailedConditions

  const step = state.trace && state.currentStepIndex >= 0
    ? state.trace.steps[state.currentStepIndex]
    : null;
  const context = state.trace
    ? contextAtStep(state.trace, state.contexts, state.currentStepIndex)
    : null;

  // ...
}
```

`useDebugger` throws outside a `DebuggerProvider`. For a component that should
work in both contexts, use `useDebuggerOptional`, which returns `null` instead.
`TreeView` uses it to render with and without the debugger attached.

## Definitions

`Workflow` and `Task` carry the 4.0 policies, `on_failure` and
`on_rejection`, beside the 3.x spellings `continue_on_error` and `halt_on`,
which the engine still reads and rewrites. Cards and flow nodes render
whichever is present. A key the engine does not know ends up in the node's
`ext`; the folder tree reads `workflow.ext?.path ?? workflow.path`.

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

`taskContinuesOnFailure`, `taskHaltsOnFailure` and
`workflowContinuesOnFailure` read a policy from either spelling. The
built-in function names and the loop defaults come from the engine
(`engine_facts()`), not from a list kept here: `validation` is the one
spelling the badge names, and `validate` renders as a custom function. The
three 3.x integration configs (`http_call`, `enrich`, `publish_kafka`) left
the engine; a host that registers them renders them as its own functions.

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
| `RulesListView` | Flat list of workflows |
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
| `WasmEngineAdapter` | The default engine, over `@goplasmatic/dataflow-wasm`; traces every run unless the options say otherwise |
| `defaultEngineFactory` | Factory producing `WasmEngineAdapter` |
| `assertEngineVersion` | Throws when the loaded engine is older than this UI build |
| `engineFacts` | The loaded engine's facts (the built-in names, the group depth, the loop and fan-out defaults), read once after `init()` |
| `DataflowEngine`, `EngineFactory` | Types for custom engines |

### Helpers

Exported alongside the types, for code that walks definitions or traces:

- **Steps:** `isTaskGroup`, `groupMembers`, `flattenSteps`, `countLeafSteps`.
  A workflow's `tasks` array holds *steps*, so an element may be a task or a
  nested group. `countLeafSteps` is `flattenSteps(..).length` without the list.
- **Folders:** `workflowFolderPath`, a workflow's `ext.path` as the engine
  keeps it, else its `path`
- **Functions:** `isBuiltinFunction`, `getFunctionDisplayInfo`
- **Policies:** `taskContinuesOnFailure`, `taskHaltsOnFailure`,
  `workflowContinuesOnFailure`
- **Loops:** `loopBadgeLabel`, `loopGuardLabel`, `loopStepLabel`, `loopDescription`
- **Fan-out:** `forEachBadgeLabel`, `forEachDescription`
- **Debug:** `contextAtStep`, `stepState`, `stepErrors`, `isWorkflowStep`,
  `isEngineStep`, `traceIsReplayable`, `findTaskStepIndex`,
  `getWorkflowState`, `getTaskState`, `isTaskCurrent`, `toExecutionError`

### Types

- **Definitions:** `Workflow`, `Task`, `TaskGroup`, `Step`, `FunctionConfig`,
  `JsonLogicValue`, `MapMapping`, `MappingItem`, `MapFunctionInput`,
  `ValidationRule`, `ValidationFunctionInput`, `BuiltinFunctionType`,
  `LoopConfig`, `ForEach`, `WorkflowStatus`, `Rollout`
- **The engine's wire types,** re-exported from `@goplasmatic/dataflow-wasm`:
  `Message`, `Context`, `Change`, `AuditEntry`, `ErrorRecord`, `Trace`,
  `TraceStep`, `StepResult`, `Run`, `RunStatus`, `WorkflowRecord`,
  `Selection`, `WorkflowExit`, `DataflowError`, `Issue`, `JsonValue`,
  `RunOptions`, `TraceOptions`, `AuditMode`, `RunResult`, `EngineError`,
  `EngineFacts`, `ParseResult`
- **Debugger:** `DebugConfig`, `DebuggerState`, `DebuggerAction`,
  `DebugNodeState`, `PlaybackState`, `ExecutionError`, `Theme`,
  `TreeNodeDebugState`, `WorkflowVisualizerProps`, `TreeSelectionType`

## From 3.x

| 3.x | 4.0 |
|---|---|
| `DataflowEngine.processWithTrace(payload)` | `run(payload, options?)`, resolving with the engine's `RunResult` on every status |
| `onExecutionComplete(trace: ExecutionTrace)` | `onExecutionComplete(result: RunResult)`; the trace is `result.trace` |
| `onExecutionError(error: string)` | `onExecutionError(error: ExecutionError)`, with `code`, `message` and `issues` |
| `ExecutionTrace`, `ExecutionStep` | `Trace`, `TraceStep` from the wasm package |
| `ErrorInfo`, `AuditTrail` | `ErrorRecord`, `AuditEntry` |
| `getMessageAtStep(trace, i)` | `contextAtStep(trace, contexts, i)`: the context, replayed, not a message |
| `getChangesAtStep(trace, i)` | `trace.steps[i].changes` |
| `traceHasSnapshots(trace)` | `traceIsReplayable(trace)` |
| `createEmptyMessage`, `cloneMessage` | Removed |
| `INTEGRATION_FUNCTION_NAMES`; `validate`, `http_call`, `enrich`, `publish_kafka` in `BuiltinFunctionType` | Removed; `isBuiltinFunction` reads the engine's `engine_facts()` |
| The payload sent to the engine as a string | Sent as the object it is |

Debug mode now requires `@goplasmatic/dataflow-wasm` to be initialised even
with a custom engine, since the trace is replayed through it.

## Building from Source

```bash
cd ui
npm install
npm run wasm:local   # build ../wasm and overlay it, when working in this repository
npm test             # the helpers and a rendered debugger, against that engine
npm run build:lib
```

Output will be in `ui/dist/`.

## Next Steps

- [WASM Package](../wasm/overview.md) - Run workflows in the browser
- [Core Concepts](../core-concepts/overview.md) - Understand workflows and tasks
