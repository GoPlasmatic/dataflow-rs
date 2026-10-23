# Core Concepts Overview

Dataflow-rs is built around a small set of core types that compile JSON definitions into an immutable program and run messages through it.

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
| **Task** (`TaskDef`) | Executes one function call | Built-in or host function, condition, outcome policies, `for_each` fan-out |
| **Registry** | Holds callable functions and operators | 8 built-in functions, custom host functions, call layers, `ExprEngine` |
| **Compiler** & **Program** | Compiles and links definitions | Compiles one workflow at a time, reports issues by JSON path, produces an immutable `Program` |
| **Engine** | Executes messages against a program | Run options, observers, traces, error-context mirroring, hot reload |
| **Message** | Carries input and runtime state | Raw `payload`, `{data, metadata, temp_data}` context, audit trail, error list |
| **Run** | Reports the outcome of a run | Final status, matched workflow count, per-workflow selection and exit, optional trace |

The [Message](./message.md) flows through the engine, carrying the raw `payload`, the `{data, metadata, temp_data}` context evaluated by JSONLogic conditions, an audit trail, and a list of recorded errors.

## Processing Flow

1. **Build**
   - Parse and lint each workflow definition with `def::parse`, which reports issues at their exact JSON paths.
   - Compile expressions and function configs one workflow at a time with `Compiler`.
   - Link the compiled workflows into an immutable `Program` sorted by priority.
   - Combine the `Program` with an `EngineConfig` to create an `Engine`.

2. **Run**
   - Create a `Message` with an input payload.
   - At the start of `run`, the engine stamps `metadata.processed_at` and `metadata.engine_version`.
   - The engine evaluates each active workflow's condition against `{data, metadata, temp_data}` in priority order.

3. **Task Execution**
   - Tasks inside each matching workflow execute sequentially.
   - Built-in functions run inline on the context view; custom host functions receive a `Ctx` and write to a per-call overlay that commits when the call returns `Ok`.
   - Each task's changes are appended to the message's audit trail, and `metadata.progress` is updated after every executed task.

4. **Result**
   - `Run` reports the final execution status and what each workflow did.
   - `Message` holds the updated context, audit trail, and any recorded errors.

## Key Design Principles

### Compile Once
The compiler validates JSONLogic expressions and function configs when building the program. Invalid definitions fail at build time with structured issues, and constant expressions are folded once ahead of time.

### Immutability & Hot Reload
A linked `Program` is immutable. `Engine` wraps its `Program` and `EngineConfig` in an `Arc`, making `Engine::clone` cheap and thread-safe. Hot-reloading creates a new `Engine` via `Engine::with_program` while in-flight runs finish undisturbed on the previous program.

### Separation of Concerns
- **`def`** — Parses and lints JSON definitions into typed data structures (`WorkflowDef`).
- **`expr`** — Compiles and evaluates JSONLogic expressions (`ExprEngine`, `Expr`, `Path`).
- **`function`** — Defines the custom function trait, context, registry, and call layers (`Function`, `Ctx`, `Registry`, `CallLayer`).
- **`program`** — Compiles and links workflows (`Compiler`, `Program`, `Issue`).
- **`run`** — Executes messages and reports outcomes (`Engine`, `RunOptions`, `Message`, `Run`).
- **`observe`** — Provides execution tracing and metrics hooks (`Observer`, `Event`, `Trace`).

### Auditability
The engine records every context write made by tasks so you can inspect each processing step for debugging or compliance. Enabling tracing also captures the initial context and per-step diffs so you can replay the exact state after any step.

## Detailed Documentation

- [Engine](./engine.md) — Building, running, observing, and reloading the engine
- [Workflows](./workflow.md) — Defining conditions, priorities, channels, and rollouts
- [Tasks](./task.md) — Configuring function calls, conditions, and outcome policies
- [Message](./message.md) — Working with payloads, context roots, audit trails, and errors
- [Error Handling](./error-handling.md) — Understanding failures, rejections, and recovery policies
