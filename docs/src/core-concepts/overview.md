# Core Concepts Overview

Dataflow-rs has a small set of core concepts that together build a program from definitions and run messages over it.

## The Big Picture

```text
┌─────────────────────────────────────────────────────────────────────┐
│  Build                                                              │
│                                                                     │
│  ┌────────────────┐    ┌──────────────────────────────────────────┐ │
│  │ Registry       │    │ Compiler                                 │ │
│  │  built-ins     │ →  │  one workflow at a time: every           │ │
│  │  host functions│    │  expression and config compiled, every   │ │
│  │  ExprEngine    │    │  issue at its authored path              │ │
│  └────────────────┘    └──────────────────────────────────────────┘ │
│                                           │ link                    │
│                                           v                         │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ Program (immutable)                                          │   │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐           │   │
│  │  │ Workflow 1  │  │ Workflow 2  │  │ Workflow N  │           │   │
│  │  │  Task 1     │  │  Task 1     │  │  Task 1     │           │   │
│  │  │  Task 2     │  │  Task 2     │  │  ...        │           │   │
│  │  └─────────────┘  └─────────────┘  └─────────────┘           │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                           │ + EngineConfig          │
│                                           v                         │
│                                   ┌──────────────┐                  │
│                                   │    Engine    │                  │
│                                   └──────────────┘                  │
└─────────────────────────────────────────────────────────────────────┘
                                            │ run(&mut message)
                                            v
┌─────────────────────────────────────────────────────────────────────┐
│  Message                                            Run             │
│  ┌──────┐ ┌─────────┐ ┌─────────┐ ┌───────────┐     status          │
│  │ data │ │metadata │ │temp_data│ │audit_trail│     matched         │
│  └──────┘ └─────────┘ └─────────┘ └───────────┘     workflows       │
│  ┌────────┐ ┌────────┐                              trace (opt.)    │
│  │payload │ │ errors │                                              │
│  └────────┘ └────────┘                                              │
└─────────────────────────────────────────────────────────────────────┘
```

## Component Summary

| Component | Purpose | Key Features |
|---|---------|--------------|
| **Workflow** (`WorkflowDef`) | Groups related tasks | Priority ordering, condition, failure policy, channel, lifecycle |
| **Task** (`TaskDef`) | One function call | Built-in or host function, condition, outcome policies, fan-out |
| **Registry** | The functions a program may call | The eight built-ins, host functions, layers, the expression engine |
| **Compiler** and **Program** | Compile and link definitions | One workflow at a time, every issue at its authored path, an immutable result |
| **Engine** | Runs messages over a program | Run options, observers, traces, the error context, reload |
| **Message** | Carries the data | A payload, the `{data, metadata, temp_data}` context, an audit trail, errors |
| **Run** | Reports one run | Status, matched workflows, each workflow's selection and exit, the trace |

The [Message](./message.md) carries the data through them: a `payload`, the
`{data, metadata, temp_data}` context every condition evaluates against, an
audit trail and a list of errors.

## Processing Flow

1. **Build**
   - Parse each definition; the parser is also the linter, and reports every issue at the path the author typed
   - Compile every expression and every function config, one workflow at a time
   - Link the compiled workflows into a program, sorted by priority
   - Pair the program with an `EngineConfig` to get an `Engine`

2. **Run**
   - Create a message with a payload
   - The engine stamps `metadata.processed_at` and `metadata.engine_version`
   - It evaluates each active workflow's condition against the full context, in priority order

3. **Task Execution**
   - Tasks run in order within each matching workflow
   - A built-in runs inline; a host function gets a `Ctx` and writes to an overlay that lands when it returns `Ok`
   - Each task's writes are recorded on the audit trail, and `metadata.progress` is rewritten after it

4. **Result**
   - The `Run` says how the run ended and what each workflow did
   - The message holds the transformed context, the audit trail and every error recorded

## Key Design Principles

### Compile Once
Every JSONLogic expression and every function config is compiled when the
program is built. A malformed definition fails the build with its issues,
never the first message, and a constant expression is evaluated once and kept.

### Immutability
A program is immutable once linked. An engine is an `Arc` around its program
and config, so cloning one is cheap and clones share everything; a reload is a
new engine (`Engine::with_program`), and the old one keeps serving the runs in
flight.

### Separation of Concerns
- **`def`** parses and lints definitions into plain data (`WorkflowDef`)
- **`expr`** compiles and evaluates JSONLogic (`ExprEngine`, `Expr`, `Path`)
- **`function`** holds the functions a task calls (`Function`, `Ctx`, `Registry`, layers)
- **`program`** compiles and links (`Compiler`, `Program`, the issue vocabulary)
- **`run`** runs messages (`Engine`, `RunOptions`, `Message`, `Run`)
- **`observe`** reports what a run did (`Observer`, `Event`, `Trace`)

### Audit Trail
The engine records every write a task makes, so you can see each processing
step for debugging and compliance. A trace adds the context before any step
and each step's changes, replayable to the context after any step.

## Detailed Documentation

- [Engine](./engine.md) - Building, running, observing and reloading
- [Workflows](./workflow.md) - Condition plus tasks bundles
- [Tasks](./task.md) - Individual processing units
- [Message](./message.md) - Data container with audit trail
- [Error Handling](./error-handling.md) - Failures, rejections and the policies
