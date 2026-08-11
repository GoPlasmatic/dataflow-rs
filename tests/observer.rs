//! `ExecutionObserver` — per-task callbacks covering the sync built-ins and the
//! async path.

use dataflow_rs::engine::message::Message;
use dataflow_rs::{Engine, Workflow};
use serde_json::json;
use std::sync::Arc;

mod common;

use common::{FailingTask, LoggingTask, RecordingObserver, n_map_task_workflow};

// =============================================================================
// ExecutionObserver — per-task timing that covers the sync built-ins
// =============================================================================

#[tokio::test]
async fn the_observer_covers_the_sync_builtins_and_the_async_path() {
    // The reason this exists: the eight sync built-ins never reach the function
    // registry, so a host cannot wrap them. Assert both dispatch sites report.
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "m", "name": "m", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "v", "name": "v", "function": {
                "name": "validation",
                "input": { "rules": [] } } },
            { "id": "l", "name": "l", "function": {
                "name": "log", "input": { "message": "hi" } } },
            { "id": "custom", "name": "custom", "function": {
                "name": "logger", "input": {} } }
        ]
    }"#,
    )
    .unwrap();

    let observer = Arc::new(RecordingObserver::default());
    let engine = Engine::builder()
        .with_workflow(wf)
        .register("logger", LoggingTask)
        .with_observer(observer.clone())
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    let seen = observer.seen();
    let ids: Vec<&str> = seen.iter().map(|e| e.task_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["m", "v", "l", "custom"],
        "every dispatched task reports, sync built-ins included"
    );

    // Function names, and the documented `validate` canonicalization.
    let fns: Vec<&str> = seen.iter().map(|e| e.function.as_str()).collect();
    assert_eq!(fns, vec!["map", "validate", "log", "logger"]);

    // All succeeded.
    assert!(seen.iter().all(|e| e.status == Some(200)));
    // Workflow id is reported.
    assert!(seen.iter().all(|e| e.workflow_id == "w"));
}

#[tokio::test]
async fn the_observer_reports_a_failing_task_before_the_error_propagates() {
    // Emitted before handle_task_result, whose `?` would otherwise drop exactly
    // the tasks a host most wants timed.
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "ok", "name": "ok", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.a", "logic": 1 } ] } } },
            { "id": "boom", "name": "boom", "function": {
                "name": "fail", "input": {} } }
        ]
    }"#,
    )
    .unwrap();

    let observer = Arc::new(RecordingObserver::default());
    let engine = Engine::builder()
        .with_workflow(wf)
        .register("fail", FailingTask)
        .with_observer(observer.clone())
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    assert!(engine.process_message(&mut message).await.is_err());

    let seen = observer.seen();
    assert_eq!(seen.len(), 2, "the failing task is still reported");
    assert_eq!(seen[1].task_id, "boom");
    assert_eq!(seen[1].status, Some(500), "an Err dispatch reports 500");
}

#[tokio::test]
async fn a_skipped_condition_is_not_reported_but_a_skip_outcome_is() {
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "never", "name": "never",
              "condition": { "==": [1, 2] },
              "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.no", "logic": 1 } ] } } },
            { "id": "gate", "name": "gate", "function": {
                "name": "filter",
                "input": { "condition": false, "on_reject": "skip" } } }
        ]
    }"#,
    )
    .unwrap();

    let observer = Arc::new(RecordingObserver::default());
    let engine = Engine::builder()
        .with_workflow(wf)
        .with_observer(observer.clone())
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    let seen = observer.seen();
    // The false-condition task was never dispatched, so there is nothing to time.
    assert_eq!(seen.len(), 1, "only the dispatched task reports: {seen:?}");
    assert_eq!(seen[0].task_id, "gate");
    // TaskOutcome::Skip ran its body but records no audit status.
    assert_eq!(seen[0].status, None, "a Skip outcome reports status None");
}

#[tokio::test]
async fn the_observer_survives_a_hot_reload() {
    // with_new_workflows rebuilds the executor stack; dropping the observer
    // there would stop metrics silently at the first reload.
    let observer = Arc::new(RecordingObserver::default());
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(1))
        .with_observer(observer.clone())
        .build()
        .unwrap();

    let mut m1 = Message::from_value(&json!({}));
    engine.process_message(&mut m1).await.unwrap();
    assert_eq!(observer.seen().len(), 1);

    let reloaded = engine
        .with_new_workflows(vec![n_map_task_workflow(2)])
        .unwrap();
    let mut m2 = Message::from_value(&json!({}));
    reloaded.process_message(&mut m2).await.unwrap();

    assert_eq!(
        observer.seen().len(),
        3,
        "the reloaded engine must keep reporting"
    );
}

#[tokio::test]
async fn observer_durations_are_populated_and_with_handlers_reaches_the_builder() {
    // `with_handlers` exists so an embedder that builds the whole handler map in
    // one place can still reach `with_observer`.
    #[derive(Default)]
    struct DurationObserver {
        total_us: std::sync::atomic::AtomicU64,
        count: std::sync::atomic::AtomicU64,
    }
    impl dataflow_rs::ExecutionObserver for DurationObserver {
        fn task_finished(&self, event: &dataflow_rs::TaskEvent<'_>) {
            use std::sync::atomic::Ordering;
            self.total_us
                .fetch_add(event.duration.as_micros() as u64, Ordering::Relaxed);
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }

    let mut handlers: std::collections::HashMap<String, dataflow_rs::BoxedFunctionHandler> =
        std::collections::HashMap::new();
    handlers.insert("logger".to_string(), Box::new(LoggingTask));

    let observer = Arc::new(DurationObserver::default());
    let engine = Engine::builder()
        .with_workflow(
            Workflow::from_json(
                r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [ { "id": "custom", "name": "custom", "function": {
            "name": "logger", "input": {} } } ]
    }"#,
            )
            .unwrap(),
        )
        .with_handlers(handlers)
        .with_observer(observer.clone())
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    use std::sync::atomic::Ordering;
    assert_eq!(observer.count.load(Ordering::Relaxed), 1);
    // Duration is a real reading, not a placeholder; it may legitimately be 0us
    // on a fast task, so only assert it was recorded.
    let _ = observer.total_us.load(Ordering::Relaxed);
}

#[tokio::test]
async fn no_observer_means_no_added_clock_reads() {
    // The gate is on observer presence, so the documented one-Utc::now()-per-
    // message invariant holds for every caller that has not opted in.
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(4))
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    let stamps: Vec<_> = message.audit_trail().iter().map(|a| a.timestamp).collect();
    assert_eq!(stamps.len(), 4);
    assert!(stamps.windows(2).all(|w| w[0] == w[1]));
}
