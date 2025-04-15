# Tokio Integration Proposal for dataflow-rs

## Overview

This document proposes adding asynchronous programming support to dataflow-rs through Tokio integration. This enhancement will significantly improve scalability, performance, and resource utilization for IO-bound workflows, while maintaining backward compatibility with existing code.

## Motivation

The current implementation of dataflow-rs uses a synchronous, blocking execution model. While thread-safe through the use of `Arc<Mutex<T>>`, this approach has several limitations:

1. **Resource Inefficiency**: Each blocking operation (HTTP requests, file I/O) consumes an OS thread for the duration of the operation
2. **Scalability Ceiling**: The number of concurrent workflows is effectively limited by the number of available threads
3. **Performance Bottlenecks**: IO-bound workflows cannot efficiently overlap operations
4. **Limited Ecosystem Integration**: Many modern Rust libraries offer async-first APIs that cannot be easily utilized

Adding Tokio-based async support would address these limitations while enabling new capabilities.

## Benefits

1. **Improved Scalability**:
   - Handle thousands of concurrent workflows with a small thread pool
   - Process more messages per second on the same hardware

2. **Enhanced IO Performance**:
   - Non-blocking HTTP requests and other IO operations
   - Lower latency for multi-stage IO-bound workflows
   - Improved throughput for high-volume processing

3. **Resource Efficiency**:
   - Lower memory footprint (fewer threads required)
   - Better CPU utilization
   - Reduced context switching overhead

4. **Modern Ecosystem Access**:
   - Integration with async AWS/GCP/Azure SDKs
   - Compatibility with async database drivers
   - Access to the broader Tokio ecosystem

## Architecture

### Core Components

1. **AsyncEngine**:
   - Async-first variant of the current Engine
   - Uses Tokio for task scheduling and execution

2. **AsyncFunctionHandler**:
   - Async trait for function handlers
   - Allows non-blocking execution of tasks

3. **Dual API Support**:
   - Maintain compatibility with existing synchronous code
   - Provide high-performance async paths for new implementations

### Component Relationship Diagram

```
┌───────────────┐      ┌───────────────┐      ┌───────────────┐
│  AsyncEngine  │──────▶ AsyncWorkflow │──────▶  AsyncTask    │
└───────────────┘      └───────────────┘      └───────────────┘
        │                                             │
        │                                             │
        ▼                                             ▼
┌───────────────┐                           ┌───────────────┐
│ Message       │                           │ AsyncFunction │
└───────────────┘                           └───────────────┘
                                                    │
                                                    │
                                                    ▼
                                           ┌───────────────┐
                                           │  Tokio        │
                                           │  Runtime      │
                                           └───────────────┘
```

## Implementation Strategy

We propose a phased implementation approach to minimize disruption while maximizing benefits:

### Phase 1: Core Async Infrastructure

1. **Add Async Trait Definitions**:
   - Define `AsyncFunctionHandler` trait
   - Create async versions of core interfaces

2. **Implement AsyncEngine**:
   - Create async-first engine implementation
   - Implement message processing pipeline with Tokio

3. **Tokio Runtime Management**:
   - Provide configurable runtime creation
   - Support both multi-threaded and current-thread runtimes

### Phase 2: Async HTTP Implementation

1. **Async HTTP Client**:
   - Replace blocking reqwest client with async version
   - Implement timeout and retry with Tokio's utilities

2. **Non-blocking Retries**:
   - Replace `std::thread::sleep` with `tokio::time::sleep`
   - Implement backoff without blocking worker threads

### Phase 3: Advanced Async Capabilities

1. **Parallel Task Execution**:
   - Add dependency tracking between tasks
   - Execute independent tasks concurrently with `join_all`

2. **Stream Processing**:
   - Add streaming capabilities for large datasets
   - Support for `Stream` trait in data sources/sinks

3. **Backpressure Handling**:
   - Implement rate limiting for external resource access
   - Add queue depth monitoring and control

### Phase 4: Ecosystem Integration

1. **Async Database Support**:
   - Add connectors for async database drivers
   - Support for connection pooling

2. **Cloud Service Integration**:
   - Add adapters for major cloud providers' async SDKs
   - Support for async messaging systems

## API Changes

### New Traits

```rust
#[async_trait]
pub trait AsyncFunctionHandler: Send + Sync {
    /// Execute the function asynchronously
    async fn execute(
        &self,
        message: &mut Message,
        input: &Value
    ) -> Result<(usize, Vec<Change>)>;
}
```

### AsyncEngine Implementation

```rust
pub struct AsyncEngine {
    workflows: HashMap<String, Workflow>,
    task_functions: HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>>,
    data_logic: Arc<Mutex<DataLogic>>,
    retry_config: RetryConfig,
}

impl AsyncEngine {
    /// Process a message asynchronously
    pub async fn process_message(&self, message: &mut Message) -> Result<()> {
        // Async implementation
    }
}
```

### Backward Compatibility

To ensure a smooth transition, we'll maintain compatibility with existing code:

1. **Synchronous Wrapper**:
   - `Engine` can use `AsyncEngine` internally with `block_on`
   - Existing function handlers can be wrapped to implement `AsyncFunctionHandler`

2. **Gradual Migration Path**:
   - Existing code continues to work unchanged
   - New code can leverage async APIs for better performance

## Implementation Examples

### Async HTTP Function

```rust
pub struct AsyncHttpFunction {
    client: reqwest::Client,  // Async client
}

impl AsyncHttpFunction {
    pub fn new(timeout_secs: u64) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .expect("Failed to create HTTP client");

        Self { client }
    }
}

#[async_trait]
impl AsyncFunctionHandler for AsyncHttpFunction {
    async fn execute(&self, message: &mut Message, input: &Value) -> Result<(usize, Vec<Change>)> {
        // Extract URL and build request
        let url = input.get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| DataflowError::Validation("URL is required".to_string()))?;

        // Make async request
        let response = self.client.get(url).send().await
            .map_err(|e| {
                if e.is_timeout() {
                    DataflowError::Timeout(format!("HTTP request timed out: {}", e))
                } else {
                    DataflowError::Http {
                        status: e.status().map_or(0, |s| s.as_u16()),
                        message: format!("HTTP request failed: {}", e)
                    }
                }
            })?;

        // Process response
        let status = response.status();
        let status_code = status.as_u16() as usize;
        
        // Async body reading
        let body = response.text().await
            .map_err(|e| DataflowError::Http {
                status: status.as_u16(),
                message: format!("Failed to read response: {}", e)
            })?;
            
        let body_json = serde_json::from_str::<Value>(&body)
            .unwrap_or_else(|_| json!(body));
            
        // Update message with response data
        message.temp_data = json!({
            "status": status_code,
            "body": body_json,
            "success": status.is_success(),
        });
        
        // Return changes
        Ok((
            status_code,
            vec![Change {
                path: "temp_data".to_string(),
                old_value: Value::Null,
                new_value: message.temp_data.clone(),
            }],
        ))
    }
}
```

### Async Message Processing

```rust
impl AsyncEngine {
    pub async fn process_message(&self, message: &mut Message) -> Result<()> {
        debug!("Processing message {} asynchronously", message.id);
        
        // Process each workflow
        for workflow in self.workflows.values() {
            // Check workflow condition
            let condition = workflow.condition.clone().unwrap_or(Value::Bool(true));
            
            match self.eval_condition(&condition, &message.metadata).await {
                Ok(should_process) => {
                    if !should_process {
                        debug!("Workflow {} skipped - condition not met", workflow.id);
                        continue;
                    }
                    
                    info!("Processing workflow {}", workflow.id);
                    
                    // Process workflow tasks, potentially in parallel
                    self.process_workflow_tasks(workflow, message).await?;
                }
                Err(e) => {
                    // Handle error...
                    return Err(e);
                }
            }
        }
        
        Ok(())
    }
    
    async fn process_workflow_tasks(&self, workflow: &Workflow, message: &mut Message) -> Result<()> {
        // Create a future for each task that should execute
        let mut task_futures = Vec::new();
        
        for task in &workflow.tasks {
            let task_condition = task.condition.clone().unwrap_or(Value::Bool(true));
            
            match self.eval_condition(&task_condition, &message.metadata).await {
                Ok(should_execute) => {
                    if !should_execute {
                        debug!("Task {} skipped - condition not met", task.id);
                        continue;
                    }
                    
                    // If we have a handler for this task
                    if let Some(function) = self.task_functions.get(&task.function.name) {
                        // Add this task execution to our futures
                        task_futures.push(self.execute_task_with_retry(
                            &task.id,
                            &workflow.id,
                            message,
                            &task.function.input,
                            &**function,
                        ));
                    } else {
                        // Handle missing function error...
                    }
                }
                Err(e) => {
                    // Handle condition evaluation error...
                    return Err(e);
                }
            }
        }
        
        // Execute tasks (option 1: sequentially)
        for task_future in task_futures {
            task_future.await?;
        }
        
        // Execute tasks (option 2: in parallel)
        // futures::future::try_join_all(task_futures).await?;
        
        Ok(())
    }
}
```

### Async Retry Mechanism

```rust
impl AsyncEngine {
    async fn execute_task_with_retry(
        &self,
        task_id: &str,
        workflow_id: &str,
        message: &mut Message,
        input_json: &Value,
        function: &dyn AsyncFunctionHandler,
    ) -> Result<()> {
        info!("Executing task {} in workflow {}", task_id, workflow_id);
        
        let mut last_error = None;
        let mut retry_count = 0;
        
        // Try executing the task up to max_retries + 1 times (initial attempt + retries)
        while retry_count <= self.retry_config.max_retries {
            match function.execute(message, input_json).await {
                Ok((status_code, changes)) => {
                    // Success! Record audit trail and return
                    message.audit_trail.push(AuditTrail {
                        workflow_id: workflow_id.to_string(),
                        task_id: task_id.to_string(),
                        timestamp: Utc::now().to_rfc3339(),
                        changes,
                        status_code,
                    });
                    
                    info!("Task {} completed with status {}", task_id, status_code);
                    
                    // Add progress metadata
                    let mut progress = Map::new();
                    progress.insert("task_id".to_string(), Value::String(task_id.to_string()));
                    progress.insert("workflow_id".to_string(), Value::String(workflow_id.to_string()));
                    progress.insert("status_code".to_string(), Value::Number(Number::from(status_code)));
                    progress.insert("timestamp".to_string(), Value::String(Utc::now().to_rfc3339()));
                    
                    if retry_count > 0 {
                        progress.insert("retries".to_string(), Value::Number(Number::from(retry_count)));
                    }
                    
                    message.metadata["progress"] = json!(progress);
                    
                    return Ok(());
                }
                Err(e) => {
                    last_error = Some(e.clone());
                    
                    if retry_count < self.retry_config.max_retries {
                        warn!("Task {} execution failed, retry {}/{}: {:?}", 
                              task_id, retry_count + 1, self.retry_config.max_retries, e);
                        
                        // Calculate delay with optional exponential backoff
                        let delay = if self.retry_config.use_backoff {
                            self.retry_config.retry_delay_ms * (2_u64.pow(retry_count))
                        } else {
                            self.retry_config.retry_delay_ms
                        };
                        
                        // Use tokio's non-blocking sleep
                        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                        
                        retry_count += 1;
                    } else {
                        break;
                    }
                }
            }
        }
        
        // If we're here, all retries failed
        let error = last_error.unwrap_or_else(|| 
            DataflowError::Unknown("Unknown error during task execution".to_string())
        );
        
        // Handle and return error...
        Err(error)
    }
}
```

## Performance Expectations

Based on similar systems, we anticipate the following performance improvements:

1. **Throughput**: 5-10x higher message processing rate for IO-bound workflows
2. **Latency**: 30-50% reduction in average processing time
3. **Resource Usage**: 70-80% reduction in thread count for the same workload
4. **Scalability**: Linear scaling to thousands of concurrent messages

## Compatibility Considerations

1. **Existing Code**: All existing code will continue to work without changes
2. **Function Handlers**: Existing handlers can be used with a simple adapter
3. **Migration Path**: Clear path for gradually adopting async where beneficial

## Dependencies

New dependencies required:

1. **tokio**: Async runtime and utilities
   ```toml
   tokio = { version = "1.28", features = ["full"] }
   ```

2. **async-trait**: For async trait support
   ```toml
   async-trait = "0.1.68"
   ```

3. **futures**: For combinators and utilities
   ```toml
   futures = "0.3.28"
   ```

## Timeline

Proposed implementation timeline:

1. **Phase 1**: Core infrastructure (2-3 weeks)
2. **Phase 2**: HTTP implementation (1-2 weeks)
3. **Phase 3**: Advanced capabilities (2-3 weeks)
4. **Phase 4**: Ecosystem integration (ongoing)

## Conclusion

Integrating Tokio into dataflow-rs will significantly enhance its capabilities, performance, and resource efficiency. The phased approach ensures minimal disruption while steadily improving the library. This enhancement will position dataflow-rs as a modern, high-performance workflow engine capable of handling demanding workloads efficiently.
