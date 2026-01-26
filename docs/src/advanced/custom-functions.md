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
    error::Result,
    message::{Change, Message}
};
use datalogic_rs::DataLogic;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct MyCustomFunction;

#[async_trait]
impl AsyncFunctionHandler for MyCustomFunction {
    async fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
        datalogic: Arc<DataLogic>,
    ) -> Result<(usize, Vec<Change>)> {
        // Your custom logic here

        // Access input configuration
        let input = &config.input;

        // Modify message data
        let old_value = message.context["data"]["processed"].clone();
        message.context["data"]["processed"] = json!(true);

        // Track changes for audit trail
        let changes = vec![Change {
            path: Arc::from("data.processed"),
            old_value: Arc::new(old_value),
            new_value: Arc::new(json!(true)),
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
let engine = Engine::new(workflows, Some(custom_functions));
```

## Using Custom Functions in Workflows

```json
{
    "id": "custom_workflow",
    "tasks": [
        {
            "id": "custom_task",
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
    datalogic: Arc<DataLogic>,
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

## Using DataLogic

Evaluate JSONLogic expressions using the provided `datalogic` instance:

```rust
async fn execute(
    &self,
    message: &mut Message,
    config: &FunctionConfig,
    datalogic: Arc<DataLogic>,
) -> Result<(usize, Vec<Change>)> {
    // Get the context for evaluation
    let context_arc = message.get_context_arc();

    // Compile and evaluate a JSONLogic expression
    let logic = json!({"var": "data.input"});
    let compiled = datalogic.compile(&logic)?;
    let result = datalogic.evaluate(&compiled, context_arc)?;

    // Use the result...
    Ok((200, vec![]))
}
```

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
        _datalogic: Arc<DataLogic>,
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
            .map_err(|e| DataflowError::Processing(e.to_string()))?;

        let data: Value = response.json()
            .await
            .map_err(|e| DataflowError::Processing(e.to_string()))?;

        // Store result
        let old_value = message.data().get("fetched").cloned().unwrap_or(json!(null));
        if let Some(data_obj) = message.data_mut().as_object_mut() {
            data_obj.insert("fetched".to_string(), data.clone());
        }
        message.invalidate_context_cache();

        let changes = vec![Change {
            path: Arc::from("data.fetched"),
            old_value: Arc::new(old_value),
            new_value: Arc::new(data),
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
    // Validation error
    if some_validation_fails {
        return Err(DataflowError::Validation("Invalid input".to_string()));
    }

    // Processing error
    if some_operation_fails {
        return Err(DataflowError::Processing("Operation failed".to_string()));
    }

    // Configuration error
    if config_is_invalid {
        return Err(DataflowError::Configuration("Invalid config".to_string()));
    }

    // Or return status codes without error
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
    message::{Change, Message}
};
use datalogic_rs::DataLogic;
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
        _datalogic: Arc<DataLogic>,
    ) -> Result<(usize, Vec<Change>)> {
        // Extract config from Custom variant
        let input = match config {
            FunctionConfig::Custom { input, .. } => input,
            _ => return Err(DataflowError::Configuration("Invalid config".into())),
        };

        // Get the field to analyze
        let field = input
            .get("field")
            .and_then(Value::as_str)
            .ok_or_else(|| DataflowError::Validation(
                "Missing 'field' in config".to_string()
            ))?;

        // Get the array from message
        let data = message.data()
            .get(field)
            .and_then(Value::as_array)
            .ok_or_else(|| DataflowError::Validation(
                format!("Field '{}' is not an array", field)
            ))?
            .clone();

        // Calculate statistics
        let numbers: Vec<f64> = data.iter()
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

        // Build result
        let stats = json!({
            "count": count,
            "sum": sum,
            "mean": mean,
            "min": min,
            "max": max
        });

        // Store in message
        let old_value = message.data().get("statistics").cloned().unwrap_or(json!(null));
        if let Some(data_obj) = message.data_mut().as_object_mut() {
            data_obj.insert("statistics".to_string(), stats.clone());
        }
        message.invalidate_context_cache();

        let changes = vec![Change {
            path: Arc::from("data.statistics"),
            old_value: Arc::new(old_value),
            new_value: Arc::new(stats),
        }];

        Ok((200, changes))
    }
}
```

## Best Practices

1. **Track Changes** - Record all modifications for audit trail
2. **Validate Input** - Check configuration before processing
3. **Handle Errors** - Return appropriate error types
4. **Invalidate Cache** - Call `message.invalidate_context_cache()` after modifications
5. **Document** - Add clear documentation for your function
6. **Test** - Write unit tests for your custom functions
