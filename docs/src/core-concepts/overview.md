# Core Concepts Overview

Dataflow-rs is built around a small set of core concepts that work together to process data efficiently.

## The Big Picture

```
┌─────────────────────────────────────────────────────────────────────┐
│                           Engine                                     │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │                    Compiled Logic Cache                        │  │
│  │  (All JSONLogic pre-compiled at startup)                       │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                      │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                 │
│  │ Workflow 1  │  │ Workflow 2  │  │ Workflow N  │                 │
│  │ ┌─────────┐ │  │ ┌─────────┐ │  │ ┌─────────┐ │                 │
│  │ │ Task 1  │ │  │ │ Task 1  │ │  │ │ Task 1  │ │                 │
│  │ │ Task 2  │ │  │ │ Task 2  │ │  │ │ Task 2  │ │                 │
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

| Component | Purpose | Key Features |
|-----------|---------|--------------|
| **Engine** | Orchestrates processing | Pre-compiled logic, workflow management |
| **Workflow** | Groups related tasks | Priority ordering, conditions |
| **Task** | Individual processing unit | Built-in or custom functions |
| **Message** | Data container | Data, metadata, audit trail |

## Processing Flow

1. **Engine Initialization**
   - Parse workflow definitions
   - Compile all JSONLogic expressions
   - Store in indexed cache

2. **Message Processing**
   - Create message with input data
   - Engine evaluates workflow conditions
   - Matching workflows execute in priority order

3. **Task Execution**
   - Tasks run sequentially within workflows
   - Each task can modify message data
   - Changes recorded in audit trail

4. **Result**
   - Message contains transformed data
   - Audit trail shows all modifications
   - Errors collected (if any)

## Key Design Principles

### Pre-compilation
All JSONLogic expressions are compiled once at engine creation. This eliminates runtime parsing overhead and ensures consistent, predictable performance.

### Immutability
Workflows are immutable after engine creation. This enables safe concurrent processing and eliminates race conditions.

### Separation of Concerns
- **LogicCompiler** handles all compilation
- **InternalExecutor** executes built-in functions
- **Engine** orchestrates the flow

### Audit Trail
Every data modification is recorded, providing complete visibility into processing steps for debugging and compliance.

## Detailed Documentation

- [Engine](./engine.md) - The central orchestrator
- [Workflow](./workflow.md) - Task collections with conditions
- [Task](./task.md) - Individual processing units
- [Message](./message.md) - Data container with audit trail
- [Error Handling](./error-handling.md) - Managing failures gracefully
