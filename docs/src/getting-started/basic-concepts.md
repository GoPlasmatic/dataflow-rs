# Basic Concepts

Understanding the core components of dataflow-rs.

## The IF → THEN → THAT Model

Dataflow-rs follows an IFTTT-style rules engine pattern:

- **IF** — Define conditions using JSONLogic (evaluated against `data`, `metadata`, `temp_data`)
- **THEN** — Execute actions: data transformation, validation, or custom async logic
- **THAT** — Chain multiple actions and rules with priority ordering

## Architecture Overview

Dataflow-rs follows a two-phase architecture:

1. **Compilation Phase** (Startup) - All JSONLogic expressions are compiled once
2. **Execution Phase** (Runtime) - Messages are processed using compiled logic

```
┌─────────────────────────────────────────────────────────────┐
│                    Compilation Phase                         │
│  ┌──────────┐    ┌──────────────┐    ┌──────────────────┐  │
│  │  Rules   │ -> │ LogicCompiler│ -> │ Compiled Cache   │  │
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

| Rules Engine | Workflow Engine | Description |
|---|---|---|
| **RulesEngine** | **Engine** | Central async component that evaluates rules and executes actions |
| **Rule** | **Workflow** | A condition + actions bundle — IF condition THEN execute actions |
| **Action** | **Task** | An individual processing step (map, validate, or custom function) |

Both naming conventions work — use whichever fits your mental model.

### Engine (RulesEngine)

The central orchestrator that processes messages through rules.

```rust
use dataflow_rs::Engine;

// Create engine with rules (compiled at creation)
let engine = Engine::new(rules, custom_functions);

// Process messages (uses pre-compiled logic)
engine.process_message(&mut message).await?;
```

### Rule (Workflow)

A collection of actions executed sequentially. Rules can have:

- **Priority** - Determines execution order (lower = first)
- **Conditions** - JSONLogic expression evaluated against the full context (`data`, `metadata`, `temp_data`)

```json
{
    "id": "premium_order",
    "name": "Premium Order Processing",
    "priority": 1,
    "condition": { ">=": [{"var": "data.order.total"}, 1000] },
    "tasks": [...]
}
```

### Action (Task)

An individual processing unit within a rule. Actions can:

- Execute built-in functions (map, validation)
- Execute custom functions
- Have conditions for conditional execution

```json
{
    "id": "apply_discount",
    "name": "Apply Discount",
    "condition": { "!!": {"var": "data.order.total"} },
    "function": {
        "name": "map",
        "input": { ... }
    }
}
```

### Message

The data structure that flows through rules. Contains:

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
2. **Rule Selection** - Engine evaluates each rule's condition
3. **Action Execution** - Actions run sequentially within each matching rule
4. **Output** - Message contains transformed data and audit trail

```
Message (input)
    │
    v
┌─────────────────────────────────────────┐
│ Rule 1 (priority: 1)                    │
│   Action 1 -> Action 2 -> Action 3     │
└─────────────────────────────────────────┘
    │
    v
┌─────────────────────────────────────────┐
│ Rule 2 (priority: 2)                    │
│   Action 1 -> Action 2                 │
└─────────────────────────────────────────┘
    │
    v
Message (output with audit trail)
```

## JSONLogic

Dataflow-rs uses [JSONLogic](https://jsonlogic.com/) for:

- **Conditions** - Control when rules/actions execute (can access any context field)
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
{">=": [{"var": "data.order.total"}, 1000]}
```

## Next Steps

- [Rules Engine](../core-concepts/engine.md) - Deep dive into the engine
- [JSONLogic](../advanced/jsonlogic.md) - Advanced JSONLogic usage
- [Custom Functions](../advanced/custom-functions.md) - Extend with custom logic
