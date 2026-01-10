# Basic Concepts

Understanding the core components of dataflow-rs.

## Architecture Overview

Dataflow-rs follows a two-phase architecture:

1. **Compilation Phase** (Startup) - All JSONLogic expressions are compiled once
2. **Execution Phase** (Runtime) - Messages are processed using compiled logic

```
┌─────────────────────────────────────────────────────────────┐
│                    Compilation Phase                         │
│  ┌──────────┐    ┌──────────────┐    ┌──────────────────┐  │
│  │ Workflows│ -> │ LogicCompiler│ -> │ Compiled Cache   │  │
│  └──────────┘    └──────────────┘    └──────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                              │
                              v
┌─────────────────────────────────────────────────────────────┐
│                    Execution Phase                           │
│  ┌─────────┐    ┌────────┐    ┌──────────────────────────┐ │
│  │ Message │ -> │ Engine │ -> │ Processed Message        │ │
│  └─────────┘    └────────┘    │ (data + audit trail)     │ │
│                               └──────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

## Core Components

### Engine

The central orchestrator that processes messages through workflows.

```rust
use dataflow_rs::Engine;

// Create engine with workflows (compiled at creation)
let engine = Engine::new(workflows, custom_functions);

// Process messages (uses pre-compiled logic)
engine.process_message(&mut message).await?;
```

### Workflow

A collection of tasks executed sequentially. Workflows can have:

- **Priority** - Determines execution order (lower = first)
- **Conditions** - JSONLogic expression to control when workflow runs

```json
{
    "id": "my_workflow",
    "name": "My Workflow",
    "priority": 1,
    "condition": { "==": [{"var": "metadata.type"}, "user"] },
    "tasks": [...]
}
```

### Task

An individual processing unit within a workflow. Tasks can:

- Execute built-in functions (map, validation)
- Execute custom functions
- Have conditions for conditional execution

```json
{
    "id": "transform_data",
    "name": "Transform Data",
    "condition": { "!!": {"var": "data.name"} },
    "function": {
        "name": "map",
        "input": { ... }
    }
}
```

### Message

The data structure that flows through workflows. Contains:

- **context.data** - Main data payload
- **context.metadata** - Message metadata
- **context.temp_data** - Temporary processing data
- **audit_trail** - Change history
- **errors** - Collected errors

```rust
use dataflow_rs::Message;
use serde_json::json;

let mut message = Message::new(&json!({
    "name": "John",
    "email": "john@example.com"
}));

// Access after processing
println!("Data: {:?}", message.context["data"]);
println!("Audit: {:?}", message.audit_trail);
```

## Data Flow

1. **Input** - Message created with initial data
2. **Workflow Selection** - Engine selects matching workflows by condition
3. **Task Execution** - Tasks run sequentially within each workflow
4. **Output** - Message contains transformed data and audit trail

```
Message (input)
    │
    v
┌─────────────────────────────────────────┐
│ Workflow 1 (priority: 1)                │
│   Task 1 -> Task 2 -> Task 3            │
└─────────────────────────────────────────┘
    │
    v
┌─────────────────────────────────────────┐
│ Workflow 2 (priority: 2)                │
│   Task 1 -> Task 2                      │
└─────────────────────────────────────────┘
    │
    v
Message (output with audit trail)
```

## JSONLogic

Dataflow-rs uses [JSONLogic](https://jsonlogic.com/) for:

- **Conditions** - Control when workflows/tasks execute
- **Data Access** - Read values from message context
- **Transformations** - Transform and combine data

Common operations:

```json
// Access data
{"var": "data.name"}

// String concatenation
{"cat": ["Hello, ", {"var": "data.name"}]}

// Conditionals
{"if": [{"var": "data.premium"}, "VIP", "Standard"]}

// Comparisons
{"==": [{"var": "metadata.type"}, "user"]}
```

## Next Steps

- [Engine](../core-concepts/engine.md) - Deep dive into the engine
- [JSONLogic](../advanced/jsonlogic.md) - Advanced JSONLogic usage
- [Custom Functions](../advanced/custom-functions.md) - Extend with custom logic
