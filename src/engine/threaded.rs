use crate::engine::{
    Engine, FunctionHandler, RetryConfig,
    compiler::LogicCompiler,
    error::{DataflowError, Result},
    message::Message,
    workflow::Workflow,
};
use crossbeam::channel::{Receiver, Sender, bounded};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Thread-safe wrapper around Engine that enables vertical scaling through thread pools
pub struct ThreadedEngine {
    // Per-worker channels for direct work assignment
    worker_senders: Vec<Sender<WorkItem>>,

    // Round-robin counter for work distribution
    next_worker: AtomicUsize,

    // Worker thread handles
    worker_handles: Vec<JoinHandle<()>>,

    // Shared configuration
    workflows: Arc<HashMap<String, Workflow>>,
    task_functions: Arc<HashMap<String, Box<dyn FunctionHandler + Send + Sync>>>,
    retry_config: RetryConfig,

    // Graceful shutdown
    shutdown: Arc<AtomicBool>,
}

struct WorkItem {
    message: Message,
    result_sender: tokio::sync::oneshot::Sender<Result<Message>>,
}

impl ThreadedEngine {
    /// Creates a new ThreadedEngine with configurable thread pool size
    ///
    /// # Arguments
    /// * `workflows` - The workflows to use for processing messages
    /// * `custom_functions` - Optional custom function handlers
    /// * `retry_config` - Optional retry configuration
    /// * `thread_count` - Number of worker threads to spawn
    pub fn new(
        workflows: Vec<Workflow>,
        custom_functions: Option<HashMap<String, Box<dyn FunctionHandler + Send + Sync>>>,
        retry_config: Option<RetryConfig>,
        thread_count: usize,
    ) -> Self {
        // Compile workflows once and share across all workers
        let mut compiler = LogicCompiler::new();
        let workflow_map = compiler.compile_workflows(workflows);
        let workflows = Arc::new(workflow_map);

        // Build function registry
        let mut task_functions = custom_functions.unwrap_or_default();
        for (name, handler) in crate::engine::functions::builtins::get_all_functions() {
            task_functions.insert(name, handler);
        }
        let task_functions = Arc::new(task_functions);

        let retry_config = retry_config.unwrap_or_default();
        let shutdown = Arc::new(AtomicBool::new(false));

        // Create per-worker bounded channels
        let mut worker_senders = Vec::with_capacity(thread_count);
        let mut worker_handles = Vec::with_capacity(thread_count);

        for id in 0..thread_count {
            // Use bounded channel for backpressure
            let (tx, rx) = bounded::<WorkItem>(256);
            worker_senders.push(tx);

            // Spawn worker thread
            let workflows = Arc::clone(&workflows);
            let task_functions = Arc::clone(&task_functions);
            let retry_config = retry_config.clone();
            let shutdown = Arc::clone(&shutdown);

            let handle = thread::spawn(move || {
                worker_loop(id, rx, workflows, task_functions, retry_config, shutdown);
            });

            worker_handles.push(handle);
        }

        ThreadedEngine {
            worker_senders,
            next_worker: AtomicUsize::new(0),
            worker_handles,
            workflows,
            task_functions,
            retry_config,
            shutdown,
        }
    }

    /// Process a message asynchronously using the thread pool
    pub async fn process_message(&self, message: Message) -> Result<Message> {
        if self.shutdown.load(Ordering::Relaxed) {
            return Err(DataflowError::Workflow(
                "Engine is shutting down".to_string(),
            ));
        }

        // Direct oneshot channel - no bridging needed
        let (tx, rx) = tokio::sync::oneshot::channel();

        // Round-robin work distribution
        let worker_count = self.worker_senders.len();
        let worker_idx = self.next_worker.fetch_add(1, Ordering::Relaxed) % worker_count;

        // Send to worker using bounded channel
        self.worker_senders[worker_idx]
            .send(WorkItem {
                message,
                result_sender: tx,
            })
            .map_err(|_| DataflowError::Workflow("Failed to send to worker".to_string()))?;

        // Await result directly
        rx.await
            .map_err(|_| DataflowError::Workflow("Worker failed to respond".to_string()))?
    }

    /// Process a message synchronously (blocks until complete)
    pub fn process_message_sync(&self, message: Message) -> Result<Message> {
        if self.shutdown.load(Ordering::Relaxed) {
            return Err(DataflowError::Workflow(
                "Engine is shutting down".to_string(),
            ));
        }

        // Use std::sync channel for sync operation
        let (tx, rx) = std::sync::mpsc::channel();

        // Round-robin work distribution
        let worker_count = self.worker_senders.len();
        let worker_idx = self.next_worker.fetch_add(1, Ordering::Relaxed) % worker_count;

        // Create adapter for sync channel
        let (oneshot_tx, oneshot_rx) = tokio::sync::oneshot::channel();

        // Spawn a thread to bridge the result
        std::thread::spawn(move || {
            if let Ok(result) = oneshot_rx.blocking_recv() {
                let _ = tx.send(result);
            }
        });

        // Send work item
        self.worker_senders[worker_idx]
            .send(WorkItem {
                message,
                result_sender: oneshot_tx,
            })
            .map_err(|_| DataflowError::Workflow("Failed to send to worker".to_string()))?;

        // Wait for result
        rx.recv()
            .map_err(|_| DataflowError::Workflow("Worker failed to respond".to_string()))?
    }

    /// Initiates graceful shutdown of the engine
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // Dropping senders will cause workers to exit
    }

    /// Waits for all workers to finish current work and terminate
    pub fn wait_for_shutdown(mut self) {
        for handle in self.worker_handles.drain(..) {
            let _ = handle.join();
        }
    }

    /// Shutdown with timeout
    pub fn shutdown_with_timeout(self, timeout: Duration) {
        self.shutdown();
        thread::sleep(timeout);
        self.wait_for_shutdown();
    }

    /// Health check for worker threads
    pub fn is_healthy(&self) -> bool {
        !self.shutdown.load(Ordering::Relaxed)
    }

    /// Get the number of worker threads
    pub fn thread_count(&self) -> usize {
        self.worker_senders.len()
    }
}

fn worker_loop(
    _id: usize,
    rx: Receiver<WorkItem>,
    workflows: Arc<HashMap<String, Workflow>>,
    task_functions: Arc<HashMap<String, Box<dyn FunctionHandler + Send + Sync>>>,
    retry_config: RetryConfig,
    shutdown: Arc<AtomicBool>,
) {
    // Create this worker's Engine instance
    let workflows_vec: Vec<Workflow> = workflows.values().cloned().collect();
    let mut engine = Engine::new_with_shared_functions(
        workflows_vec,
        Arc::clone(&task_functions),
        Some(retry_config),
    );

    // Process messages from channel
    while !shutdown.load(Ordering::Relaxed) {
        match rx.recv() {
            Ok(work_item) => {
                let mut message = work_item.message;
                let result = engine.process_message(&mut message).map(|_| message);

                // Send result back
                let _ = work_item.result_sender.send(result);
            }
            Err(_) => {
                // Channel closed, shutdown
                break;
            }
        }
    }
}

impl Drop for ThreadedEngine {
    fn drop(&mut self) {
        if !self.shutdown.load(Ordering::Relaxed) {
            self.shutdown();
        }
    }
}

impl Clone for ThreadedEngine {
    fn clone(&self) -> Self {
        ThreadedEngine {
            worker_senders: self.worker_senders.clone(),
            next_worker: AtomicUsize::new(0),
            worker_handles: Vec::new(),
            workflows: Arc::clone(&self.workflows),
            task_functions: Arc::clone(&self.task_functions),
            retry_config: self.retry_config.clone(),
            shutdown: Arc::clone(&self.shutdown),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::functions::FunctionHandler;
    use crate::engine::message::Change;
    use crate::engine::{FunctionConfig, Workflow};
    use datalogic_rs::datalogic::DataLogic;
    use serde_json::{Value, json};

    // Custom test function handler
    struct TestFunction {
        name: String,
    }

    impl FunctionHandler for TestFunction {
        fn execute(
            &self,
            message: &mut Message,
            _config: &FunctionConfig,
            _datalogic: &DataLogic,
        ) -> Result<(usize, Vec<Change>)> {
            // Add a field to indicate the custom function was called
            let old_value = message
                .data
                .get("custom_function_called")
                .unwrap_or(&Value::Null)
                .clone();
            message.data["custom_function_called"] = json!(self.name);
            Ok((
                200,
                vec![Change {
                    path: "data.custom_function_called".to_string(),
                    old_value,
                    new_value: json!(self.name),
                }],
            ))
        }
    }

    #[test]
    fn test_threaded_engine_creation() {
        let workflows = vec![
            Workflow::from_json(r#"{"id": "test", "name": "Test", "priority": 0, "tasks": []}"#)
                .unwrap(),
        ];

        let engine = ThreadedEngine::new(workflows, None, None, 2);
        assert_eq!(engine.thread_count(), 2);
    }

    #[test]
    fn test_process_message_sync() {
        let workflows = vec![
            Workflow::from_json(r#"{"id": "test", "name": "Test", "priority": 0, "tasks": []}"#)
                .unwrap(),
        ];

        let engine = ThreadedEngine::new(workflows, None, None, 2);
        let message = Message::new(&json!({"test": "data"}));

        let result = engine.process_message_sync(message);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_process_message_async() {
        let workflows = vec![
            Workflow::from_json(r#"{"id": "test", "name": "Test", "priority": 0, "tasks": []}"#)
                .unwrap(),
        ];

        let engine = ThreadedEngine::new(workflows, None, None, 2);
        let message = Message::new(&json!({"test": "data"}));

        let result = engine.process_message(message).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_custom_functions() {
        // Create a workflow that uses a custom function
        let workflows = vec![
            Workflow::from_json(
                r#"{
                "id": "test", 
                "name": "Test",
                "priority": 0,
                "tasks": [{
                    "id": "custom_task",
                    "name": "Custom Task",
                    "function": {
                        "name": "test_function",
                        "input": {}
                    }
                }]
            }"#,
            )
            .unwrap(),
        ];

        // Create custom functions map
        let mut custom_functions = HashMap::new();
        custom_functions.insert(
            "test_function".to_string(),
            Box::new(TestFunction {
                name: "test_function".to_string(),
            }) as Box<dyn FunctionHandler + Send + Sync>,
        );

        let engine = ThreadedEngine::new(workflows, Some(custom_functions), None, 2);
        let message = Message::new(&json!({"test": "data"}));

        let result = engine.process_message_sync(message);
        assert!(result.is_ok());

        let processed_message = result.unwrap();
        assert_eq!(
            processed_message.data["custom_function_called"],
            json!("test_function")
        );
    }

    #[test]
    fn test_graceful_shutdown() {
        let workflows = vec![
            Workflow::from_json(r#"{"id": "test", "name": "Test", "priority": 0, "tasks": []}"#)
                .unwrap(),
        ];

        let engine = ThreadedEngine::new(workflows, None, None, 2);

        // Test that message processing works before shutdown
        let message = Message::new(&json!({"test": "data"}));
        let result = engine.process_message_sync(message);
        assert!(result.is_ok());

        engine.shutdown();

        // After shutdown, process_message should fail
        let message = Message::new(&json!({"test": "data"}));
        let result = engine.process_message_sync(message);
        assert!(result.is_err());
    }

    #[test]
    fn test_health_check() {
        let workflows = vec![
            Workflow::from_json(r#"{"id": "test", "name": "Test", "priority": 0, "tasks": []}"#)
                .unwrap(),
        ];

        let engine = ThreadedEngine::new(workflows, None, None, 2);

        // Initially should be healthy
        assert!(engine.is_healthy());

        // Check thread count
        assert_eq!(engine.thread_count(), 2);
    }
}
