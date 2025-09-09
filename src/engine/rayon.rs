use crate::engine::{
    Engine, FunctionHandler, RetryConfig,
    error::{DataflowError, Result},
    message::Message,
    workflow::Workflow,
};
use rayon::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

thread_local! {
    static LOCAL_ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
}

/// High-performance CPU-optimized workflow engine using Rayon for maximum CPU utilization.
///
/// RayonEngine is designed for CPU-bound workloads with minimal I/O operations. It leverages
/// Rayon's work-stealing thread pool to automatically balance work across all available CPU cores.
///
/// ## Architecture
///
/// - **Work-Stealing Pool**: Rayon automatically distributes work across threads
/// - **Thread-Local Engines**: Each worker thread maintains its own Engine instance with compiled logic
/// - **Zero Contention**: No shared mutable state between threads
/// - **Automatic Scaling**: Utilizes all available CPU cores by default
///
/// ## Performance Characteristics
///
/// - Near-linear scaling with CPU cores for CPU-bound workloads
/// - Minimal overhead from work distribution
/// - Efficient cache utilization through thread locality
/// - Automatic load balancing via work-stealing
pub struct RayonEngine {
    // Rayon thread pool for CPU-bound processing
    rayon_pool: Arc<rayon::ThreadPool>,

    // Shared immutable configuration for engine initialization
    workflows: Arc<Vec<Workflow>>,
    task_functions: Arc<HashMap<String, Box<dyn FunctionHandler + Send + Sync>>>,
    retry_config: Arc<RetryConfig>,
}

impl RayonEngine {
    /// Creates a new RayonEngine with default thread pool (uses all CPU cores)
    ///
    /// # Arguments
    /// * `workflows` - The workflows to use for processing messages
    /// * `custom_functions` - Optional custom function handlers
    /// * `retry_config` - Optional retry configuration
    pub fn new(
        workflows: Vec<Workflow>,
        custom_functions: Option<HashMap<String, Box<dyn FunctionHandler + Send + Sync>>>,
        retry_config: Option<RetryConfig>,
    ) -> Self {
        let cpu_cores = num_cpus::get();
        Self::with_thread_count(workflows, custom_functions, retry_config, cpu_cores)
    }

    /// Creates a new RayonEngine with specified thread count
    ///
    /// # Arguments
    /// * `workflows` - The workflows to use for processing messages
    /// * `custom_functions` - Optional custom function handlers
    /// * `retry_config` - Optional retry configuration
    /// * `thread_count` - Number of worker threads in the Rayon pool
    pub fn with_thread_count(
        workflows: Vec<Workflow>,
        custom_functions: Option<HashMap<String, Box<dyn FunctionHandler + Send + Sync>>>,
        retry_config: Option<RetryConfig>,
        thread_count: usize,
    ) -> Self {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(thread_count)
            .thread_name(|i| format!("rayon-engine-{}", i))
            .build()
            .expect("Failed to create Rayon thread pool");

        Self::with_pool(workflows, custom_functions, retry_config, pool)
    }

    /// Creates a new RayonEngine with a custom Rayon thread pool
    ///
    /// # Arguments
    /// * `workflows` - The workflows to use for processing messages
    /// * `custom_functions` - Optional custom function handlers
    /// * `retry_config` - Optional retry configuration
    /// * `pool` - Custom Rayon thread pool
    pub fn with_pool(
        workflows: Vec<Workflow>,
        custom_functions: Option<HashMap<String, Box<dyn FunctionHandler + Send + Sync>>>,
        retry_config: Option<RetryConfig>,
        pool: rayon::ThreadPool,
    ) -> Self {
        let task_functions = custom_functions.unwrap_or_default();
        let retry_config = retry_config.unwrap_or_default();

        RayonEngine {
            rayon_pool: Arc::new(pool),
            workflows: Arc::new(workflows),
            task_functions: Arc::new(task_functions),
            retry_config: Arc::new(retry_config),
        }
    }

    /// Process a message synchronously (blocks until complete)
    ///
    /// This method executes directly in the Rayon thread pool context,
    /// providing the most efficient execution path for CPU-bound workloads.
    pub fn process_message_sync(&self, mut message: Message) -> Result<Message> {
        let workflows = Arc::clone(&self.workflows);
        let task_functions = Arc::clone(&self.task_functions);
        let retry_config = Arc::clone(&self.retry_config);

        self.rayon_pool.install(|| {
            // Initialize thread-local engine if needed
            LOCAL_ENGINE.with(|engine_cell| {
                let mut engine_ref = engine_cell.borrow_mut();
                if engine_ref.is_none() {
                    let engine = Engine::new_with_shared_functions(
                        workflows.as_ref().clone(),
                        Arc::clone(&task_functions),
                        Some(retry_config.as_ref().clone()),
                    );
                    *engine_ref = Some(engine);
                }

                // Process the message using the thread-local engine
                engine_ref.as_mut().unwrap().process_message(&mut message)
            })?;

            Ok(message)
        })
    }

    /// Process a message asynchronously
    ///
    /// This method bridges async Tokio with sync Rayon using a oneshot channel,
    /// allowing integration with async code while maintaining CPU efficiency.
    pub async fn process_message(&self, message: Message) -> Result<Message> {
        let workflows = Arc::clone(&self.workflows);
        let task_functions = Arc::clone(&self.task_functions);
        let retry_config = Arc::clone(&self.retry_config);
        let pool = Arc::clone(&self.rayon_pool);

        // Use oneshot channel for async/sync bridge
        let (tx, rx) = tokio::sync::oneshot::channel();

        pool.spawn(move || {
            let result = LOCAL_ENGINE.with(|engine_cell| {
                let mut engine_ref = engine_cell.borrow_mut();
                if engine_ref.is_none() {
                    let engine = Engine::new_with_shared_functions(
                        workflows.as_ref().clone(),
                        Arc::clone(&task_functions),
                        Some(retry_config.as_ref().clone()),
                    );
                    *engine_ref = Some(engine);
                }

                let mut msg = message;
                engine_ref.as_mut().unwrap().process_message(&mut msg)?;
                Ok::<Message, DataflowError>(msg)
            });

            let _ = tx.send(result);
        });

        rx.await
            .map_err(|_| DataflowError::Workflow("Worker failed to respond".to_string()))?
    }

    /// Process multiple messages in parallel
    ///
    /// This method leverages Rayon's parallel iterator for efficient batch processing,
    /// automatically distributing messages across all available threads.
    pub fn process_batch(&self, messages: Vec<Message>) -> Vec<Result<Message>> {
        let workflows = Arc::clone(&self.workflows);
        let task_functions = Arc::clone(&self.task_functions);
        let retry_config = Arc::clone(&self.retry_config);

        self.rayon_pool.install(|| {
            messages
                .into_par_iter()
                .map(|message| {
                    LOCAL_ENGINE.with(|engine_cell| {
                        let mut engine_ref = engine_cell.borrow_mut();
                        if engine_ref.is_none() {
                            let engine = Engine::new_with_shared_functions(
                                workflows.as_ref().clone(),
                                Arc::clone(&task_functions),
                                Some(retry_config.as_ref().clone()),
                            );
                            *engine_ref = Some(engine);
                        }

                        let mut msg = message;
                        engine_ref.as_mut().unwrap().process_message(&mut msg)?;
                        Ok(msg)
                    })
                })
                .collect()
        })
    }

    /// Process a stream of messages in parallel with controlled concurrency
    ///
    /// This method processes messages as they arrive, maintaining a pipeline
    /// of concurrent processing for optimal throughput.
    pub fn process_stream<I>(&self, messages: I) -> impl ParallelIterator<Item = Result<Message>>
    where
        I: IntoParallelIterator<Item = Message>,
    {
        let workflows = Arc::clone(&self.workflows);
        let task_functions = Arc::clone(&self.task_functions);
        let retry_config = Arc::clone(&self.retry_config);

        messages.into_par_iter().map(move |message| {
            LOCAL_ENGINE.with(|engine_cell| {
                let mut engine_ref = engine_cell.borrow_mut();
                if engine_ref.is_none() {
                    let engine = Engine::new_with_shared_functions(
                        workflows.as_ref().clone(),
                        Arc::clone(&task_functions),
                        Some(retry_config.as_ref().clone()),
                    );
                    *engine_ref = Some(engine);
                }

                let mut msg = message;
                engine_ref.as_mut().unwrap().process_message(&mut msg)?;
                Ok(msg)
            })
        })
    }

    /// Get the number of threads in the Rayon pool
    pub fn thread_count(&self) -> usize {
        self.rayon_pool.current_num_threads()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_rayon_engine_creation() {
        let workflows = vec![];
        let engine = RayonEngine::new(workflows, None, None);
        assert!(engine.thread_count() > 0);
    }

    #[test]
    fn test_sync_processing() {
        let workflow_json = r#"{
            "id": "test-workflow",
            "name": "Test Workflow",
            "tasks": [
                {
                    "id": "task1",
                    "name": "Test Task",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "data.result",
                                    "logic": "test"
                                }
                            ]
                        }
                    }
                }
            ]
        }"#;

        let workflow = Workflow::from_json(workflow_json).unwrap();
        let engine = RayonEngine::new(vec![workflow], None, None);

        let message = Message::new(&json!({}));
        let result = engine.process_message_sync(message).unwrap();

        assert_eq!(result.data.get("result"), Some(&json!("test")));
    }

    #[tokio::test]
    async fn test_async_processing() {
        let workflow_json = r#"{
            "id": "test-workflow",
            "name": "Test Workflow",
            "tasks": [
                {
                    "id": "task1",
                    "name": "Test Task",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "data.result",
                                    "logic": "async_test"
                                }
                            ]
                        }
                    }
                }
            ]
        }"#;

        let workflow = Workflow::from_json(workflow_json).unwrap();
        let engine = RayonEngine::new(vec![workflow], None, None);

        let message = Message::new(&json!({}));
        let result = engine.process_message(message).await.unwrap();

        assert_eq!(result.data.get("result"), Some(&json!("async_test")));
    }

    #[test]
    fn test_batch_processing() {
        let workflow_json = r#"{
            "id": "test-workflow",
            "name": "Test Workflow",
            "tasks": [
                {
                    "id": "task1",
                    "name": "Test Task",
                    "function": {
                        "name": "map",
                        "input": {
                            "mappings": [
                                {
                                    "path": "data.processed",
                                    "logic": true
                                }
                            ]
                        }
                    }
                }
            ]
        }"#;

        let workflow = Workflow::from_json(workflow_json).unwrap();
        let engine = RayonEngine::new(vec![workflow], None, None);

        let messages: Vec<Message> = (0..10).map(|i| Message::new(&json!({"id": i}))).collect();

        let results = engine.process_batch(messages);

        assert_eq!(results.len(), 10);
        for result in results {
            let msg = result.unwrap();
            assert_eq!(msg.data.get("processed"), Some(&json!(true)));
        }
    }

    #[test]
    fn test_custom_thread_count() {
        let workflows = vec![];
        let engine = RayonEngine::with_thread_count(workflows, None, None, 4);
        assert_eq!(engine.thread_count(), 4);
    }
}
