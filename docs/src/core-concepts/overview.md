# Core Concepts Overview

Dataflow-rs is built around a small set of core concepts that work together to evaluate rules and execute actions efficiently.

## The Big Picture

```text
┌─────────────────────────────────────────────────────────────────────┐
│                     Rules Engine (Engine)                            │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │                    Compiled Logic Cache                        │  │
│  │  (All JSONLogic pre-compiled at startup)                       │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                      │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                 │
│  │  Rule 1     │  │  Rule 2     │  │  Rule N     │                 │
│  │ ┌─────────┐ │  │ ┌─────────┐ │  │ ┌─────────┐ │                 │
│  │ │Action 1 │ │  │ │Action 1 │ │  │ │Action 1 │ │                 │
│  │ │Action 2 │ │  │ │Action 2 │ │  │ │Action 2 │ │                 │
│  │ │ ...     │ │  │ │ ...     │ │  │ │ ...     │ │                 │
│  │ └─────────┘ │  │ └─────────┘ │  │ └─────────┘ │                 │
│  └─────────────┘  └─────────────┘  └─────────────┘                 │
└─────────────────────────────────────────────────────────────────────┘
                              │
                              v
┌─────────────────────────────────────────────────────────────────────┐
│                          Message                                     │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌────────────┐ │
│  │    data     │  │  metadata   │  │  temp_data  │  │ audit_trail│ │
│  └─────────────┘  └─────────────┘  └─────────────┘  └────────────┘ │
└─────────────────────────────────────────────────────────────────────┘
```

## Component Summary

| Rules Engine | Workflow Engine | Purpose | Key Features |
|---|---|---------|--------------|
| **RulesEngine** | **Engine** | Orchestrates processing | Pre-compiled logic, rule management |
| **Rule** | **Workflow** | Groups related actions | Priority ordering, conditions |
| **Action** | **Task** | Individual processing unit | Built-in or custom functions |

The [Message](./message.md) carries the data through them: a `payload`, the
`{data, metadata, temp_data}` context every condition is evaluated against, an
audit trail and a list of errors.


## Processing Flow

1. **Engine Initialization**
   - Parse rule definitions
   - Compile all JSONLogic expressions
   - Store in indexed cache

2. **Message Processing**
   - Create message with input data
   - Engine evaluates each rule's condition against the full context
   - Matching rules execute in priority order

3. **Action Execution**
   - Actions run sequentially within each rule
   - Each action can modify message data
   - Changes recorded in audit trail

4. **Result**
   - Message contains transformed data
   - Audit trail shows all modifications
   - Errors collected (if any)

## Key Design Principles

### Pre-compilation
All JSONLogic expressions are compiled once at engine creation. This eliminates runtime parsing overhead and ensures consistent, predictable performance.

### Immutability
Rules are immutable after engine creation. This enables safe concurrent processing and eliminates race conditions.

### Separation of Concerns
- **LogicCompiler** handles all compilation
- **WorkflowExecutor** orchestrates a rule's task list — conditions, groups, loops, audit trail
- **TaskExecutor** dispatches one action; sync built-ins run inside the executor's arena scope
- **Engine** orchestrates the flow

### Audit Trail
Every data modification is recorded, providing complete visibility into processing steps for debugging and compliance.

## Detailed Documentation

- [Rules Engine](./engine.md) - The central orchestrator
- [Rules (Workflows)](./workflow.md) - Condition + actions bundles
- [Actions (Tasks)](./task.md) - Individual processing units
- [Message](./message.md) - Data container with audit trail
- [Error Handling](./error-handling.md) - Managing failures gracefully
