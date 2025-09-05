use crate::engine::{
    Engine, FunctionHandler, RetryConfig,
    compiler::LogicCompiler,
    error::{DataflowError, Result},
    message::Message,
    workflow::Workflow,
};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Thread-safe wrapper around Engine that enables vertical scaling through thread pools
pub struct ThreadedEngine {
    // Thread pool management
    thread_count: usize,
    worker_threads: Vec<WorkerThread>,

    // Work distribution
    work_queue: Arc<Mutex<VecDeque<WorkItem>>>,
    available_workers: Arc<Mutex<VecDeque<usize>>>,
    work_notifier: Arc<Condvar>,

    // Shared immutable configuration
    workflows: Arc<HashMap<String, Workflow>>,
    task_functions: Arc<HashMap<String, Box<dyn FunctionHandler + Send + Sync>>>,
    retry_config: RetryConfig,

    // Graceful shutdown control
    shutdown: Arc<AtomicBool>,
}

struct WorkerThread {
    id: usize,
    handle: Option<JoinHandle<()>>,
}

struct WorkItem {
    message: Message,
    result_sender: std::sync::mpsc::Sender<Result<Message>>,
}

/// Configuration for worker threads
struct WorkerConfig {
    id: usize,
    workflows: Arc<HashMap<String, Workflow>>,
    task_functions: Arc<HashMap<String, Box<dyn FunctionHandler + Send + Sync>>>,
    retry_config: RetryConfig,
    work_queue: Arc<Mutex<VecDeque<WorkItem>>>,
    available_workers: Arc<Mutex<VecDeque<usize>>>,
    work_notifier: Arc<Condvar>,
    shutdown: Arc<AtomicBool>,
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

        // Create work distribution infrastructure
        let work_queue = Arc::new(Mutex::new(VecDeque::new()));
        let available_workers = Arc::new(Mutex::new(VecDeque::new()));
        let work_notifier = Arc::new(Condvar::new());
        let shutdown = Arc::new(AtomicBool::new(false));

        // Spawn worker threads
        let mut worker_threads = Vec::with_capacity(thread_count);
        for id in 0..thread_count {
            let config = WorkerConfig {
                id,
                workflows: Arc::clone(&workflows),
                task_functions: Arc::clone(&task_functions),
                retry_config: retry_config.clone(),
                work_queue: Arc::clone(&work_queue),
                available_workers: Arc::clone(&available_workers),
                work_notifier: Arc::clone(&work_notifier),
                shutdown: Arc::clone(&shutdown),
            };
            let worker = Self::spawn_worker(config);
            worker_threads.push(worker);
        }

        ThreadedEngine {
            thread_count,
            worker_threads,
            work_queue,
            available_workers,
            work_notifier,
            workflows,
            task_functions,
            retry_config,
            shutdown,
        }
    }

    /// Process a message asynchronously using the thread pool
    pub async fn process_message(&self, message: Message) -> Result<Message> {
        // Check if shutting down
        if self.shutdown.load(Ordering::Relaxed) {
            return Err(DataflowError::Workflow(
                "Engine is shutting down".to_string(),
            ));
        }

        // Use tokio oneshot channel for async communication
        let (tx, rx) = tokio::sync::oneshot::channel();

        let (std_tx, std_rx) = std::sync::mpsc::channel();

        tokio::spawn(async move {
            if let Ok(result) = std_rx.recv() {
                let _ = tx.send(result);
            }
        });

        // Queue work item
        {
            let mut queue = self.work_queue.lock().unwrap();
            queue.push_back(WorkItem {
                message,
                result_sender: std_tx,
            });
        }

        // Notify an available worker
        self.work_notifier.notify_one();

        // Await result
        rx.await
            .map_err(|_| DataflowError::Workflow("Worker failed to respond".to_string()))?
    }

    /// Process a message synchronously (blocks until complete)
    pub fn process_message_sync(&self, message: Message) -> Result<Message> {
        // Check if shutting down
        if self.shutdown.load(Ordering::Relaxed) {
            return Err(DataflowError::Workflow(
                "Engine is shutting down".to_string(),
            ));
        }

        // Use standard mpsc channel for synchronous operation
        let (tx, rx) = std::sync::mpsc::channel();

        // Queue work item
        {
            let mut queue = self.work_queue.lock().unwrap();
            queue.push_back(WorkItem {
                message,
                result_sender: tx,
            });
        }

        // Notify an available worker
        self.work_notifier.notify_one();

        // Block waiting for result
        rx.recv()
            .map_err(|_| DataflowError::Workflow("Worker failed to respond".to_string()))?
    }

    /// Initiates graceful shutdown of the engine
    pub fn shutdown(&self) {
        // Set shutdown flag
        self.shutdown.store(true, Ordering::Relaxed);

        // Wake all waiting workers
        self.work_notifier.notify_all();
    }

    /// Waits for all workers to finish current work and terminate
    pub fn wait_for_shutdown(&mut self) {
        for worker in &mut self.worker_threads {
            if let Some(handle) = worker.handle.take() {
                // Join worker thread, ignoring any panic
                let _ = handle.join();
            }
        }
    }

    /// Shutdown with timeout - returns unprocessed items
    pub fn shutdown_with_timeout(&mut self, timeout: Duration) -> Vec<Message> {
        // Signal shutdown
        self.shutdown();

        // Wait for timeout
        thread::sleep(timeout);

        // Drain remaining work items
        let mut unprocessed = Vec::new();
        if let Ok(mut queue) = self.work_queue.lock() {
            while let Some(item) = queue.pop_front() {
                unprocessed.push(item.message);
                // Notify sender that processing was cancelled
                let _ = item.result_sender.send(Err(DataflowError::Workflow(
                    "Engine shutdown before processing".to_string(),
                )));
            }
        }

        // Force join remaining threads
        self.wait_for_shutdown();

        unprocessed
    }

    /// Health check for worker threads
    pub fn is_healthy(&self) -> bool {
        let available = self.available_workers.lock().unwrap();
        let queue = self.work_queue.lock().unwrap();

        // Healthy if we have workers and queue isn't growing unbounded
        !available.is_empty() || queue.len() < self.thread_count * 10
    }

    /// Restart failed workers if needed
    pub fn restart_failed_workers(&mut self) {
        for worker in &mut self.worker_threads {
            if worker.handle.is_none() {
                // Handle is None, need to restart this worker
                let config = WorkerConfig {
                    id: worker.id,
                    workflows: Arc::clone(&self.workflows),
                    task_functions: Arc::clone(&self.task_functions),
                    retry_config: self.retry_config.clone(),
                    work_queue: Arc::clone(&self.work_queue),
                    available_workers: Arc::clone(&self.available_workers),
                    work_notifier: Arc::clone(&self.work_notifier),
                    shutdown: Arc::clone(&self.shutdown),
                };
                let new_worker = Self::spawn_worker(config);
                *worker = new_worker;
            }
        }
    }

    /// Get the number of worker threads
    pub fn thread_count(&self) -> usize {
        self.thread_count
    }

    /// Get the current queue depth
    pub fn queue_depth(&self) -> usize {
        self.work_queue.lock().unwrap().len()
    }

    /// Get the number of available (idle) workers
    pub fn available_workers_count(&self) -> usize {
        self.available_workers.lock().unwrap().len()
    }

    // Private helper to spawn a worker thread
    fn spawn_worker(config: WorkerConfig) -> WorkerThread {
        let id = config.id;
        let handle = thread::spawn(move || {
            Self::worker_loop(config);
        });

        WorkerThread {
            id,
            handle: Some(handle),
        }
    }

    // Worker thread main loop
    fn worker_loop(config: WorkerConfig) {
        let WorkerConfig {
            id,
            workflows,
            task_functions,
            retry_config,
            work_queue,
            available_workers,
            work_notifier: notifier,
            shutdown,
        } = config;
        // Create this worker's Engine instance
        // We need to convert Arc<HashMap> to Vec<Workflow> for Engine::new_with_shared_functions
        let workflows_vec: Vec<Workflow> = workflows.values().cloned().collect();

        // Use the new_with_shared_functions method to share the function registry
        let mut engine = Engine::new_with_shared_functions(
            workflows_vec,
            Arc::clone(&task_functions),
            Some(retry_config),
        );

        loop {
            // Mark as available before waiting for work
            {
                let mut available = available_workers.lock().unwrap();
                available.push_back(id);
            }

            // Wait for work or shutdown signal
            let work_item = {
                let mut queue = work_queue.lock().unwrap();
                loop {
                    // Check shutdown flag
                    if shutdown.load(Ordering::Relaxed) {
                        // Remove self from available workers before exiting
                        let mut available = available_workers.lock().unwrap();
                        available.retain(|&x| x != id);
                        return;
                    }

                    // Try to get work
                    if let Some(item) = queue.pop_front() {
                        // Remove self from available workers
                        let mut available = available_workers.lock().unwrap();
                        available.retain(|&x| x != id);
                        break Some(item);
                    }

                    // Wait for notification
                    queue = notifier.wait(queue).unwrap();
                }
            };

            if let Some(item) = work_item {
                // Process message using this worker's Engine instance
                let mut message = item.message;
                let result = engine.process_message(&mut message).map(|_| message);

                // Send result back through channel
                // Ignore send errors (receiver may have timed out)
                let _ = item.result_sender.send(result);
            }
        }
    }
}

impl Drop for ThreadedEngine {
    fn drop(&mut self) {
        // Only shutdown if we have worker threads (not a clone)
        if !self.worker_threads.is_empty() {
            // Ensure clean shutdown
            if !self.shutdown.load(Ordering::Relaxed) {
                self.shutdown();
            }
            self.wait_for_shutdown();
        }
    }
}

// Make ThreadedEngine cloneable for sharing across async tasks
impl Clone for ThreadedEngine {
    fn clone(&self) -> Self {
        // Note: This creates a shallow clone sharing the same worker pool
        // This is intentional - we want multiple handles to the same engine
        ThreadedEngine {
            thread_count: self.thread_count,
            worker_threads: Vec::new(), // Don't clone thread handles
            work_queue: Arc::clone(&self.work_queue),
            available_workers: Arc::clone(&self.available_workers),
            work_notifier: Arc::clone(&self.work_notifier),
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
    use crate::engine::{Workflow, FunctionConfig};
    use crate::engine::message::Change;
    use crate::engine::functions::FunctionHandler;
    use datalogic_rs::datalogic::DataLogic;
    use serde_json::{json, Value};

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
            let old_value = message.data.get("custom_function_called").unwrap_or(&Value::Null).clone();
            message.data["custom_function_called"] = json!(self.name);
            Ok((200, vec![Change {
                path: "data.custom_function_called".to_string(),
                old_value,
                new_value: json!(self.name),
            }]))
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
            Workflow::from_json(r#"{
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
            }"#)
            .unwrap(),
        ];

        // Create custom functions map
        let mut custom_functions = HashMap::new();
        custom_functions.insert(
            "test_function".to_string(),
            Box::new(TestFunction { name: "test_function".to_string() }) as Box<dyn FunctionHandler + Send + Sync>
        );

        let engine = ThreadedEngine::new(workflows, Some(custom_functions), None, 2);
        let message = Message::new(&json!({"test": "data"}));

        let result = engine.process_message_sync(message);
        assert!(result.is_ok());
        
        let processed_message = result.unwrap();
        assert_eq!(processed_message.data["custom_function_called"], json!("test_function"));
    }

    #[test]
    fn test_graceful_shutdown() {
        let workflows = vec![
            Workflow::from_json(r#"{"id": "test", "name": "Test", "priority": 0, "tasks": []}"#)
                .unwrap(),
        ];

        let mut engine = ThreadedEngine::new(workflows, None, None, 2);

        engine.shutdown();
        engine.wait_for_shutdown();

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

        // Check queue depth
        assert_eq!(engine.queue_depth(), 0);
    }
}
