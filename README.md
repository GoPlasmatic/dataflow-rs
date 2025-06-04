# Dataflow-rs

[![Release Crates](https://github.com/codetiger/dataflow-rs/actions/workflows/crate-publish.yml/badge.svg)](https://github.com/codetiger/dataflow-rs/actions/workflows/crate-publish.yml)
[![Crates.io Version](https://img.shields.io/crates/v/dataflow-rs)](https://crates.io/crates/dataflow-rs)
[![License](https://img.shields.io/crates/l/dataflow-rs)](LICENSE)

Dataflow-rs is a lightweight, rule-driven **async workflow engine** designed for building powerful data processing pipelines and nanoservices in Rust. Extend it with your custom async tasks to create robust, maintainable services with proper concurrency and performance.

## ✨ Features

- **🚀 Async-First Design:** Built from the ground up with Tokio for high-performance async processing
- **📋 Rule-Based Workflow Selection:** Dynamically select workflows using JSONLogic expressions
- **⚙️ Task Orchestration:** Compose sequences of async tasks for complex data processing
- **🔄 Message Transformation:** Seamlessly modify message data via specialized async tasks
- **❌ Comprehensive Error Handling:** Detailed error types and recovery mechanisms
- **🔁 Retry Capabilities:** Configurable retry policies for transient failures
- **📝 Audit Trails:** Automatically record changes for debugging and monitoring
- **🔌 Pluggable Architecture:** Easily extend the framework by registering custom async tasks
- **🧵 Thread-Safety:** Properly handles concurrent execution with thread-safe patterns
- **🎯 Custom Functions:** Implement domain-specific async functions with full engine integration

## 📚 Table of Contents

- [Overview](#overview)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [Async Architecture](#async-architecture)
- [Built-in Functions](#built-in-functions)
- [Custom Functions](#custom-functions)
- [Advanced Examples](#advanced-examples)
- [Error Handling](#error-handling)
- [Performance & Benchmarking](#performance--benchmarking)
- [Contributing](#contributing)
- [License](#license)

## 🔍 Overview

Dataflow-rs empowers developers to build scalable async nanoservices and data pipelines with ease. Its core design focuses on **asynchronous processing**, flexibility, extensibility, and resilience, allowing you to integrate your custom business logic into robust workflows with proper error handling and performance optimization.

### Key Components

- **🚀 Engine**: The central async component that processes messages through workflows
- **📋 Workflow**: A collection of tasks with conditions that determine when they should be applied *(Note: workflow conditions can only access metadata fields)*
- **⚙️ Task**: An individual async processing unit that performs a specific function on a message  
- **🔧 AsyncFunctionHandler**: A trait implemented by task handlers to define custom async processing logic
- **📨 Message**: The data structure that flows through the engine, containing payload, metadata, and processing results

## 📦 Installation

To incorporate Dataflow-rs into your project, add the following to your `Cargo.toml`:

```toml
[dependencies]
dataflow-rs = "0.1.6"
tokio = { version = "1.0", features = ["full"] }
serde_json = "1.0"
```

## 🚀 Quick Start

Below is a simple example demonstrating how to set up an async workflow that processes data:

```rust
use dataflow_rs::{Engine, Workflow};
use dataflow_rs::engine::message::Message;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create the async workflow engine (built-in functions are auto-registered)
    let mut engine = Engine::new();

    // Define a workflow in JSON
    let workflow_json = r#"
    {
        "id": "data_processor",
        "name": "Data Processor",
        "tasks": [
            {
                "id": "fetch_data",
                "name": "Fetch Data",
                "function": {
                    "name": "http",
                    "input": { "url": "https://jsonplaceholder.typicode.com/users/1" }
                }
            },
            {
                "id": "transform_data",
                "name": "Transform Data",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "data.user_name",
                                "logic": { "var": "temp_data.body.name" }
                            },
                            {
                                "path": "data.user_email", 
                                "logic": { "var": "temp_data.body.email" }
                            }
                        ]
                    }
                }
            }
        ]
    }
    "#;

    // Parse and add the workflow to the engine
    let workflow = Workflow::from_json(workflow_json)?;
    engine.add_workflow(&workflow);

    // Create a message to process
    let mut message = Message::new(&json!({}));

    // Process the message asynchronously through the workflow
    engine.process_message(&mut message).await?;

    println!("✅ Processed result: {}", serde_json::to_string_pretty(&message.data)?);

    Ok(())
}
```

## 🏗️ Async Architecture

Dataflow-rs is built with async-first principles using Tokio:

### Sequential Workflow Processing

Workflows are processed **sequentially** to ensure that later workflows can depend on the results of earlier workflows:

```rust
// The engine processes messages asynchronously
engine.process_message(&mut message).await?;

// Each workflow's condition is evaluated just before execution
// using the current message state, allowing workflows to depend
// on results from previous workflows

// Multiple messages can still be processed concurrently
let futures: Vec<_> = messages.into_iter()
    .map(|mut msg| engine.process_message(&mut msg))
    .collect();

let results = futures::future::join_all(futures).await;
```

### Workflow Dependencies

Since workflows are executed sequentially and conditions are evaluated just before execution, you can create workflows that depend on each other. However, workflow conditions can **only access metadata fields**, not data fields:

```json
{
  "workflows": [
    {
      "id": "fetch_user_data",
      "condition": true,
      "tasks": [
        {
          "id": "fetch_data",
          "function": {
            "name": "http",
            "input": { "url": "https://api.example.com/users/1" }
          }
        },
        {
          "id": "set_metadata",
          "function": {
            "name": "map",
            "input": {
              "mappings": [
                {
                  "path": "metadata.user_fetched",
                  "logic": true
                },
                {
                  "path": "metadata.user_id", 
                  "logic": { "var": "temp_data.body.id" }
                }
              ]
            }
          }
        }
      ]
    },
    {
      "id": "process_user_data", 
      "condition": { "!!": { "var": "user_fetched" } },
      "tasks": [...]
    }
  ]
}
```

In this example, the first workflow sets metadata flags that the second workflow's condition can evaluate.

### Async Task Execution

Within each workflow, tasks are executed sequentially but asynchronously with proper error handling and audit trails maintained throughout the async execution chain.

## 🛠️ Built-in Functions

The engine comes with several pre-registered async functions:

### 📡 HTTP Function
Fetches data from external HTTP APIs asynchronously:

```json
{
    "function": {
        "name": "http",
        "input": {
            "url": "https://api.example.com/data",
            "method": "GET",
            "headers": {
                "Authorization": "Bearer token"
            }
        }
    }
}
```

### 🗂️ Map Function
Maps and transforms data between different parts of a message using JSONLogic with support for both object and array notation:

```json
{
    "function": {
        "name": "map", 
        "input": {
            "mappings": [
                {
                    "path": "data.result",
                    "logic": { "var": "temp_data.body.value" }
                },
                {
                    "path": "data.processed_at",
                    "logic": { "cat": ["Processed at ", { "var": "metadata.timestamp" }] }
                },
                {
                    "path": "data.transactions.0.id", 
                    "logic": "TXN123"
                }
            ]
        }
    }
}
```

The Map function supports **array notation** in paths - when numeric indices like `0`, `1`, `2` are encountered, arrays are automatically created.

### ✅ Validate Function
Validates message data against rules using JSONLogic expressions. Unlike workflow conditions, validation rules can access all message fields (`data`, `metadata`, `temp_data`):

```json
{
    "function": {
        "name": "validate",
        "input": {
            "rules": [
                {
                    "logic": { "!!": { "var": "data.user.email" } },
                    "path": "data",
                    "message": "Email is required"
                }
            ]
        }
    }
}
```

## 🔧 Custom Functions

One of the most powerful features of dataflow-rs is the ability to implement custom async functions that integrate seamlessly with the workflow engine.

### 📋 Basic Structure

To create a custom async function, implement the `AsyncFunctionHandler` trait:

```rust
use dataflow_rs::engine::{AsyncFunctionHandler, error::Result, message::{Change, Message}};
use async_trait::async_trait;
use serde_json::Value;

pub struct MyCustomFunction;

#[async_trait]
impl AsyncFunctionHandler for MyCustomFunction {
    async fn execute(&self, message: &mut Message, input: &Value) -> Result<(usize, Vec<Change>)> {
        // Your custom async logic here
        
        // Simulate async operation
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        
        // Return status code and list of changes made
        Ok((200, vec![]))
    }
}
```

### 🔑 Key Components

#### Input Parameters
- `message`: Mutable reference to the message being processed
- `input`: JSON configuration from the workflow definition

#### Return Value
- `Result<(usize, Vec<Change>)>`: Status code and list of changes made to the message

#### Message Structure
```rust
pub struct Message {
    pub id: String,
    pub data: Value,           // Main data payload
    pub payload: Value,        // Original input payload
    pub metadata: Value,       // Processing metadata
    pub temp_data: Value,      // Temporary processing data
    pub audit_trail: Vec<AuditTrail>,
    pub errors: Vec<ErrorInfo>,
}
```

### 📊 Example: Statistics Function

Here's a comprehensive example of a custom function that calculates statistical measures:

```rust
use dataflow_rs::{
    engine::{
        error::{DataflowError, Result},
        message::{Change, Message},
        AsyncFunctionHandler,
    },
    Engine, Workflow,
};
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct StatisticsFunction;

#[async_trait]
impl AsyncFunctionHandler for StatisticsFunction {
    async fn execute(&self, message: &mut Message, input: &Value) -> Result<(usize, Vec<Change>)> {
        let data_path = input.get("data_path").and_then(Value::as_str).unwrap_or("data.numbers");
        let output_path = input.get("output_path").and_then(Value::as_str).unwrap_or("data.statistics");
        
        // Extract numbers from the specified path
        let numbers = self.extract_numbers_from_path(message, data_path)?;
        
        if numbers.is_empty() {
            return Err(DataflowError::Validation("No numeric data found".to_string()));
        }
        
        // Calculate statistics asynchronously
        let stats = self.calculate_statistics(&numbers).await;
        
        // Store results
        self.set_value_at_path(message, output_path, stats.clone())?;
        
        Ok((200, vec![Change {
            path: output_path.to_string(),
            old_value: Value::Null,
            new_value: stats,
        }]))
    }
}

impl StatisticsFunction {
    pub fn new() -> Self {
        Self
    }
    
    async fn calculate_statistics(&self, numbers: &[f64]) -> Value {
        // Simulate async processing
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        
        let count = numbers.len();
        let sum: f64 = numbers.iter().sum();
        let mean = sum / count as f64;
        
        let mut sorted = numbers.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        
        let median = if count % 2 == 0 {
            (sorted[count / 2 - 1] + sorted[count / 2]) / 2.0
        } else {
            sorted[count / 2]
        };
        
        json!({
            "count": count,
            "sum": sum,
            "mean": mean,
            "median": median,
            "min": sorted[0],
            "max": sorted[count - 1]
        })
    }
    
    // Helper methods for path navigation...
}
```

### 🏢 Example: Data Enrichment Function

Here's an example that demonstrates async external data lookup:

```rust
pub struct DataEnrichmentFunction {
    enrichment_data: HashMap<String, Value>,
}

#[async_trait]
impl AsyncFunctionHandler for DataEnrichmentFunction {
    async fn execute(&self, message: &mut Message, input: &Value) -> Result<(usize, Vec<Change>)> {
        let lookup_value = input.get("lookup_value").and_then(Value::as_str)
            .ok_or_else(|| DataflowError::Validation("Missing lookup_value".to_string()))?;
        
        // Simulate async operation (database lookup, API call, etc.)
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        
        // Look up enrichment data
        let enrichment = self.enrichment_data.get(lookup_value)
            .cloned()
            .unwrap_or_else(|| json!({"status": "not_found"}));
        
        // Store enrichment data
        let output_path = input.get("output_path").and_then(Value::as_str).unwrap_or("data.enrichment");
        self.set_value_at_path(message, output_path, enrichment.clone())?;
        
        Ok((200, vec![Change {
            path: output_path.to_string(),
            old_value: Value::Null,
            new_value: enrichment,
        }]))
    }
}
```

### 📋 Registering Custom Functions

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create engine (empty or with built-ins)
    let mut engine = Engine::new_empty();
    
    // Register your custom async functions
    engine.register_task_function(
        "statistics".to_string(),
        Box::new(StatisticsFunction::new()),
    );
    
    engine.register_task_function(
        "enrich_data".to_string(),
        Box::new(DataEnrichmentFunction::new()),
    );
    
    // Use in workflows
    let workflow_json = r#"
    {
        "id": "custom_workflow",
        "name": "Custom Function Demo",
        "tasks": [
            {
                "id": "calculate_stats",
                "name": "Calculate Statistics",
                "function": {
                    "name": "statistics",
                    "input": {
                        "data_path": "data.numbers",
                        "output_path": "data.stats"
                    }
                }
            }
        ]
    }
    "#;
    
    let workflow = Workflow::from_json(workflow_json)?;
    engine.add_workflow(&workflow);
    
    Ok(())
}
```

### ✅ Best Practices for Custom Functions

#### 1. **Async Operations**
```rust
// ✅ Good: Non-blocking async operation
let response = reqwest::get(url).await?;

// ✅ Good: Simulated async delay
tokio::time::sleep(Duration::from_millis(100)).await;
```

#### 2. **Error Handling**
```rust
let required_field = input.get("required_field")
    .ok_or_else(|| DataflowError::Validation("Missing required_field parameter".to_string()))?;
```

#### 3. **Change Tracking**
```rust
let changes = vec![Change {
    path: "data.result".to_string(),
    old_value: old_value,
    new_value: new_value,
}];
Ok((200, changes))
```

## 📈 Advanced Examples

### 🔄 Concurrent Message Processing

While workflows within a single message are processed sequentially, you can still process multiple **messages** concurrently:

```rust
use futures::stream::{FuturesUnordered, StreamExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::new();
    let mut messages = vec![/* your messages */];
    
    // Process multiple messages concurrently
    // Each message's workflows execute sequentially within that message
    let mut futures = FuturesUnordered::new();
    
    for message in &mut messages {
        futures.push(engine.process_message(message));
    }
    
    // Collect results as they complete
    while let Some(result) = futures.next().await {
        match result {
            Ok(_) => println!("Message processed successfully"),
            Err(e) => println!("Error processing message: {:?}", e),
        }
    }
    
    Ok(())
}
```

### 🔧 Custom Function Handler with State

```rust
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct StatefulFunction {
    cache: Arc<Mutex<HashMap<String, Value>>>,
}

#[async_trait]
impl AsyncFunctionHandler for StatefulFunction {
    async fn execute(&self, message: &mut Message, input: &Value) -> Result<(usize, Vec<Change>)> {
        let mut cache = self.cache.lock().await;
        
        // Use shared state
        let key = "cache_key";
        if let Some(cached_value) = cache.get(key) {
            // Use cached value
        } else {
            // Compute and cache new value
            let new_value = json!("computed_value");
            cache.insert(key.to_string(), new_value);
        }
        
        Ok((200, vec![]))
    }
}
```

## ❌ Error Handling

Dataflow-rs provides comprehensive async error handling with dedicated error types:

```rust
use dataflow_rs::{Engine, DataflowError};
use dataflow_rs::engine::message::Message;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = Engine::new();
    let mut message = Message::new(&json!({}));
    
    // Configure message to continue processing despite errors
    message.set_continue_on_error(true);
    
    // Process the message, errors will be collected but not halt execution
    match engine.process_message(&mut message).await {
        Ok(_) => {
            // Check if there were any errors during processing
            if message.has_errors() {
                for error in &message.errors {
                    println!("⚠️ Error in workflow: {:?}, task: {:?}: {:?}", 
                             error.workflow_id, error.task_id, error.error);
                }
            }
        }
        Err(e) => {
            println!("❌ Fatal error: {:?}", e);
        }
    }
    
    Ok(())
}
```

### 🔁 Retry Configuration

Configure retry behavior for transient failures:

```rust
use dataflow_rs::{Engine, RetryConfig};

#[tokio::main] 
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create an engine with custom retry configuration
    let engine = Engine::new()
        .with_retry_config(RetryConfig {
            max_retries: 5,
            retry_delay_ms: 500,
            use_backoff: true,
        });
    
    // Now any transient failures will be retried according to this configuration
    Ok(())
}
```

## 📊 Performance & Benchmarking

### Running Benchmarks

To test the async performance of the workflow engine:

```bash
cargo run --example benchmark
```

This benchmark demonstrates:
- ✅ Async vs sync performance comparison
- ✅ Proper async function execution timing
- ✅ Realistic workflow processing scenarios
- ✅ Statistical analysis of processing times

### Running Custom Function Examples

To see custom async functions in action:

```bash
cargo run --example custom_function
```

Example output:
```json
{
  "numbers": [10.5, 15.2, 8.7, 22.1, 18.9, 12.3, 25.6, 14.8, 19.4, 16.7],
  "stats": {
    "count": 10,
    "max": 25.6,
    "mean": 16.42,
    "median": 15.95,
    "min": 8.7,
    "std_dev": 4.97,
    "sum": 164.2,
    "variance": 24.74
  },
  "user_info": {
    "department": "Engineering",
    "location": "San Francisco",
    "manager": "Alice Johnson",
    "security_clearance": "Level 2",
    "start_date": "2022-01-15"
  }
}
```

## 🚀 Advanced Features

### 🏗️ Engine Variants

```rust
// Full engine with all built-in functions
let engine = Engine::new();

// Empty engine for custom functions only
let engine = Engine::new_empty();

// Engine with specific functions
let mut engine = Engine::new_empty();
engine.register_task_function("custom".to_string(), Box::new(CustomFunction));
```

### 🔧 Workflow Conditions

Use JSONLogic for dynamic workflow selection. **Important**: Workflow conditions can only access `metadata` fields:

```json
{
    "id": "conditional_workflow",
    "condition": {
        "and": [
            { "==": [{ "var": "message_type" }, "user"] },
            { ">": [{ "var": "priority" }, 5] }
        ]
    },
    "tasks": [...]
}
```

To make data available for workflow conditions, set metadata fields in earlier workflows:

```json
{
    "id": "data_preparation",
    "tasks": [
        {
            "id": "set_metadata",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {
                            "path": "metadata.message_type",
                            "logic": { "var": "data.type" }
                        },
                        {
                            "path": "metadata.priority",
                            "logic": { "var": "data.priority" }
                        }
                    ]
                }
            }
        }
    ]
}
```

### 🏢 Extending the Framework

Dataflow-rs is highly extensible for building nanoservices:

- ✅ Implement custom async tasks by creating structs that implement `AsyncFunctionHandler`
- ✅ Create your own error types by extending from the base `DataflowError`
- ✅ Build nanoservices by integrating multiple async workflows
- ✅ Leverage the built-in HTTP, validation, and mapping functions
- ✅ Integrate with external databases, APIs, and services asynchronously

## 📚 Documentation

For detailed API documentation and additional examples:

- [API Documentation](https://docs.rs/dataflow-rs)
- [GitHub Discussions](https://github.com/codetiger/dataflow-rs/discussions)
- [Examples Directory](examples/)

## 🤝 Contributing

We welcome contributions! Check out our [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines on how to help improve Dataflow-rs.

## 📄 License

This project is licensed under the Apache License 2.0. See the [LICENSE](LICENSE) file for details.

---

**Built with ❤️ for the Rust async ecosystem**
