# Custom Functions

Extend dataflow-rs with your own custom processing logic by implementing the `AsyncFunctionHandler` trait.

## Overview

Custom functions allow you to:

- Add domain-specific processing logic
- Integrate with external systems
- Perform async operations (HTTP, database, etc.)
- Implement complex transformations

## Implementing AsyncFunctionHandler

```rust
use async_trait::async_trait;
use dataflow_rs::engine::{
    AsyncFunctionHandler,
    FunctionConfig,
    error::{DataflowError, Result},
    message::{Change, Message},
    utils::{get_nested_value, set_nested_value},
};
use datalogic_rs::Engine as DatalogicEngine;
use datavalue::OwnedDataValue;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct MyCustomFunction;

#[async_trait]
impl AsyncFunctionHandler for MyCustomFunction {
    async fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
        _engine: Arc<DatalogicEngine>,
    ) -> Result<(usize, Vec<Change>)> {
        // Access input configuration (Custom variant)
        let _input = match config {
            FunctionConfig::Custom { input, .. } => input,
            _ => return Err(DataflowError::Validation("Invalid config".into())),
        };

        // Capture old value before writing
        let old_value = get_nested_value(&message.context, "data.processed")
            .cloned()
            .unwrap_or(OwnedDataValue::Null);

        // Mutate message data via the path helper
        let new_value = OwnedDataValue::from(&json!(true));
        set_nested_value(&mut message.context, "data.processed", new_value.clone());

        // Track changes for audit trail (`Change` owns `OwnedDataValue`s
        // directly — no `Arc::new` wrapping needed)
        let changes = vec![Change {
            path: Arc::from("data.processed"),
            old_value,
            new_value,
        }];

        // Return status code and changes
        // 200 = success, 400 = validation error, 500 = execution error
        Ok((200, changes))
    }
}
```

## Registering Custom Functions

```rust
use std::collections::HashMap;
use dataflow_rs::{Engine, Workflow};
use dataflow_rs::engine::AsyncFunctionHandler;

// Create custom functions map
let mut custom_functions: HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>> =
    HashMap::new();

// Register your function
custom_functions.insert(
    "my_custom_function".to_string(),
    Box::new(MyCustomFunction)
);

// Create engine with custom functions
let engine = Engine::new(workflows, Some(custom_functions))?;
```

## Using Custom Functions in Rules

```json
{
    "id": "custom_rule",
    "tasks": [
        {
            "id": "custom_action",
            "function": {
                "name": "my_custom_function",
                "input": {
                    "option1": "value1",
                    "option2": 42
                }
            }
        }
    ]
}
```

## Accessing Configuration

For custom functions, extract configuration from the `FunctionConfig::Custom` variant:

```rust
async fn execute(
    &self,
    message: &mut Message,
    config: &FunctionConfig,
    _engine: Arc<DatalogicEngine>,
) -> Result<(usize, Vec<Change>)> {
    // Extract input from Custom variant
    let input = match config {
        FunctionConfig::Custom { input, .. } => input,
        _ => return Err(DataflowError::Validation("Invalid config".into())),
    };

    // Access input parameters
    let option1 = input
        .get("option1")
        .and_then(Value::as_str)
        .unwrap_or("default");

    let option2 = input
        .get("option2")
        .and_then(Value::as_i64)
        .unwrap_or(0);

    // Use the parameters...
    Ok((200, vec![]))
}
```

## Evaluating JSONLogic

Custom handlers can compile and evaluate ad-hoc JSONLogic against
`message.context` using the shared engine:

```rust
use bumpalo::Bump;

async fn execute(
    &self,
    message: &mut Message,
    _config: &FunctionConfig,
    engine: Arc<DatalogicEngine>,
) -> Result<(usize, Vec<Change>)> {
    // Compile the expression (use Arc<Logic> so it can be cached/shared)
    let compiled = engine.compile_arc(&json!({"var": "data.input"}))
        .map_err(|e| DataflowError::LogicEvaluation(e.to_string()))?;

    // Evaluate against the context. `engine.evaluate` takes a borrowed
    // `DataValue<'_>` plus a Bump arena for any allocations the eval needs.
    let arena = Bump::new();
    let ctx = message.context.to_arena(&arena);
    let result = engine.evaluate(&compiled, ctx, &arena)
        .map_err(|e| DataflowError::LogicEvaluation(e.to_string()))?;

    // Use the result (DataValue<'_> borrowed from the arena, or call
    // `result.to_owned()` to lift it into an OwnedDataValue).
    let _owned = result.to_owned();
    Ok((200, vec![]))
}
```

If your handler evaluates many expressions against the same context,
build the `DataValue<'_>` once via `to_arena` and reuse it — that skips
the per-eval deep-walk into the arena.

## Async Operations

Custom functions support async/await for I/O operations:

```rust
use reqwest;

pub struct HttpFetchFunction;

#[async_trait]
impl AsyncFunctionHandler for HttpFetchFunction {
    async fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
        _engine: Arc<DatalogicEngine>,
    ) -> Result<(usize, Vec<Change>)> {
        // Extract URL from config
        let input = match config {
            FunctionConfig::Custom { input, .. } => input,
            _ => return Err(DataflowError::Validation("Invalid config".into())),
        };

        let url = input
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| DataflowError::Validation("Missing url".to_string()))?;

        // Make HTTP request
        let response = reqwest::get(url)
            .await
            .map_err(|e| DataflowError::http(0, e.to_string()))?;

        let data_json: Value = response.json()
            .await
            .map_err(|e| DataflowError::http(0, e.to_string()))?;

        // Bridge into OwnedDataValue and write into the context
        let new_value = OwnedDataValue::from(&data_json);
        let old_value = get_nested_value(&message.context, "data.fetched")
            .cloned()
            .unwrap_or(OwnedDataValue::Null);
        set_nested_value(&mut message.context, "data.fetched", new_value.clone());

        let changes = vec![Change {
            path: Arc::from("data.fetched"),
            old_value,
            new_value,
        }];

        Ok((200, changes))
    }
}
```

## Error Handling

Return appropriate errors for different failure modes:

```rust
use dataflow_rs::engine::error::DataflowError;

async fn execute(&self, ...) -> Result<(usize, Vec<Change>)> {
    // Bad/invalid input — non-retryable
    if some_validation_fails {
        return Err(DataflowError::Validation("Invalid input".to_string()));
    }

    // Task-level execution failure — non-retryable
    if some_operation_fails {
        return Err(DataflowError::Task("Operation failed".to_string()));
    }

    // Wrapping a downstream error — retryability inherited from `source`
    if downstream_call_failed {
        return Err(DataflowError::function_execution(
            "HTTP call failed",
            Some(DataflowError::http(503, "Service Unavailable")),
        ));
    }

    // Or return status codes without an error
    // 200 for success
    // 400 for validation failure
    // 500 for processing failure
    Ok((200, vec![]))
}
```

## Complete Example

```rust
use async_trait::async_trait;
use dataflow_rs::engine::{
    AsyncFunctionHandler, FunctionConfig,
    error::{DataflowError, Result},
    message::{Change, Message},
    utils::{get_nested_value, set_nested_value},
};
use datalogic_rs::Engine as DatalogicEngine;
use datavalue::OwnedDataValue;
use serde_json::{json, Value};
use std::sync::Arc;

/// Calculates statistics from numeric array data
pub struct StatisticsFunction;

#[async_trait]
impl AsyncFunctionHandler for StatisticsFunction {
    async fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
        _engine: Arc<DatalogicEngine>,
    ) -> Result<(usize, Vec<Change>)> {
        // Extract config from Custom variant
        let input = match config {
            FunctionConfig::Custom { input, .. } => input,
            _ => return Err(DataflowError::Validation("Invalid config".into())),
        };

        // Get the field to analyze
        let field = input
            .get("field")
            .and_then(Value::as_str)
            .ok_or_else(|| DataflowError::Validation(
                "Missing 'field' in config".to_string()
            ))?;

        // Pull the array out of `data.<field>`
        let path = format!("data.{}", field);
        let array = match get_nested_value(&message.context, &path) {
            Some(OwnedDataValue::Array(items)) => items.clone(),
            _ => return Err(DataflowError::Validation(
                format!("Field '{}' is not an array", field)
            )),
        };

        // Calculate statistics. `OwnedDataValue::as_f64()` mirrors serde_json.
        let numbers: Vec<f64> = array.iter()
            .filter_map(|v| v.as_f64())
            .collect();

        if numbers.is_empty() {
            return Err(DataflowError::Validation(
                "No numeric values found".to_string()
            ));
        }

        let sum: f64 = numbers.iter().sum();
        let count = numbers.len() as f64;
        let mean = sum / count;
        let min = numbers.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = numbers.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        // Build the result and write it back into the context
        let stats = OwnedDataValue::from(&json!({
            "count": count,
            "sum": sum,
            "mean": mean,
            "min": min,
            "max": max
        }));
        let old_value = get_nested_value(&message.context, "data.statistics")
            .cloned()
            .unwrap_or(OwnedDataValue::Null);
        set_nested_value(&mut message.context, "data.statistics", stats.clone());

        let changes = vec![Change {
            path: Arc::from("data.statistics"),
            old_value,
            new_value: stats,
        }];

        Ok((200, changes))
    }
}
```

## Best Practices

1. **Track Changes** - Record all modifications for audit trail
2. **Validate Input** - Check configuration before processing
3. **Handle Errors** - Return appropriate error types
4. **Mutate via `set_nested_value`** - It's the canonical way to write
   into `message.context` (auto-creates intermediates, handles array
   indexing and `#`-prefix escapes).
5. **Document** - Add clear documentation for your function
6. **Test** - Write unit tests for your custom functions
