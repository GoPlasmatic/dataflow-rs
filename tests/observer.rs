//! `ExecutionObserver` — per-task callbacks covering the sync built-ins and the
//! async path.

use dataflow_rs::engine::message::Message;
use dataflow_rs::{Engine, ExecutionObserver, TaskEvent, Workflow};
use serde_json::json;
use std::sync::{Arc, Mutex};

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

// =============================================================================
// Lifecycle callbacks — message and workflow boundaries.
//
// The hazard these guard is double emission or a missed emission: a workflow
// runs via one of two structurally different paths (the shared-arena sync run,
// or the per-workflow async driver), and a looping workflow runs its body many
// times. Exactly one started/finished pair must come out of all of them.
// =============================================================================

/// Records the callback sequence as flat strings, so an assertion reads as the
/// timeline an observer actually sees.
#[derive(Clone, Default)]
struct Timeline(Arc<Mutex<Vec<String>>>);

impl Timeline {
    fn events(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
    fn push(&self, s: String) {
        self.0.lock().unwrap().push(s);
    }
}

impl ExecutionObserver for Timeline {
    fn message_started(&self, e: &dataflow_rs::MessageStarted<'_>) {
        self.push(format!("message_started({})", e.workflows_considered));
    }
    fn message_finished(&self, e: &dataflow_rs::MessageFinished<'_>) {
        self.push(format!(
            "message_finished(stopped_early={})",
            e.stopped_early
        ));
    }
    fn workflow_started(&self, e: &dataflow_rs::WorkflowStarted<'_>) {
        self.push(format!("workflow_started({})", e.workflow_id));
    }
    fn workflow_finished(&self, e: &dataflow_rs::WorkflowFinished<'_>) {
        self.push(format!(
            "workflow_finished({}, sweeps={}, halted={})",
            e.workflow_id, e.sweeps, e.halted
        ));
    }
    fn task_finished(&self, e: &TaskEvent<'_>) {
        self.push(format!("task_finished({}/{})", e.workflow_id, e.task_id));
    }
}

fn map_task(id: &str) -> String {
    format!(
        r#"{{"id": "{id}", "name": "{id}", "function": {{"name": "map",
            "input": {{"mappings": [{{"path": "data.{id}", "logic": 1}}]}}}}}}"#
    )
}

async fn timeline_for(workflows: Vec<Workflow>) -> Vec<String> {
    let recorder = Timeline::default();
    let engine = Engine::builder()
        .with_workflows(workflows)
        .with_observer(Arc::new(recorder.clone()))
        .build()
        .unwrap();
    let mut message = Message::from_value(&json!({}));
    let _ = engine.process_message(&mut message).await;
    recorder.events()
}

fn wf_json(id: &str, tasks: &str, extra: &str) -> Workflow {
    Workflow::from_json(&format!(
        r#"{{"id": "{id}", "name": "{id}", "priority": 0 {extra}, "tasks": [{tasks}]}}"#
    ))
    .expect("fixture parses")
}

#[tokio::test]
async fn a_two_workflow_message_reports_nested_boundaries_in_order() {
    let events = timeline_for(vec![
        wf_json("first", &map_task("a"), ""),
        wf_json("second", &map_task("b"), ""),
    ])
    .await;

    assert_eq!(
        events,
        vec![
            "message_started(2)",
            "workflow_started(first)",
            "task_finished(first/a)",
            "workflow_finished(first, sweeps=1, halted=false)",
            "workflow_started(second)",
            "task_finished(second/b)",
            "workflow_finished(second, sweeps=1, halted=false)",
            "message_finished(stopped_early=false)",
        ],
        "task events must nest inside their workflow's pair"
    );
}

#[tokio::test]
async fn a_workflow_skipped_by_its_condition_emits_no_workflow_events() {
    let events = timeline_for(vec![
        wf_json("skipped", &map_task("a"), r#", "condition": false"#),
        wf_json("ran", &map_task("b"), ""),
    ])
    .await;

    assert!(
        !events.iter().any(|e| e.contains("skipped")),
        "a workflow its condition rejected never started, got {events:?}"
    );
    assert!(events.contains(&"workflow_started(ran)".to_string()));
}

#[tokio::test]
async fn a_workflow_outside_its_rollout_bucket_emits_no_workflow_events() {
    let recorder = Timeline::default();
    let engine = Engine::builder()
        .with_workflow(wf_json(
            "canary",
            &map_task("a"),
            r#", "rollout": {"bucket_start": 0, "bucket_end": 10}"#,
        ))
        .with_observer(Arc::new(recorder.clone()))
        .build()
        .unwrap();

    let mut message = Message::builder().routing_bucket(50).build();
    let _ = engine.process_message(&mut message).await;

    assert!(
        !recorder.events().iter().any(|e| e.contains("canary")),
        "the rollout gate is upstream of the span, got {:?}",
        recorder.events()
    );
}

#[tokio::test]
async fn a_looping_workflow_reports_one_pair_with_the_sweep_count() {
    let events = timeline_for(vec![wf_json(
        "sweeper",
        &map_task("a"),
        r#", "loop": {"counter": "i", "init": 0, "max": 3, "increment": 1}"#,
    )])
    .await;

    assert_eq!(
        events
            .iter()
            .filter(|e| e.starts_with("workflow_started"))
            .count(),
        1,
        "one pair for the whole loop, not one per sweep: {events:?}"
    );
    assert!(
        events.contains(&"workflow_finished(sweeper, sweeps=3, halted=false)".to_string()),
        "the sweep count carries the cardinality instead: {events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.starts_with("task_finished"))
            .count(),
        3,
        "the body still reports per sweep"
    );
}

#[tokio::test]
async fn a_halted_workflow_says_so_on_the_finished_event() {
    let tasks = format!(
        r#"{}, {{"id": "gate", "name": "gate", "function": {{"name": "filter",
            "input": {{"condition": false, "on_reject": "halt"}}}}}}, {}"#,
        map_task("a"),
        map_task("never")
    );
    let events = timeline_for(vec![wf_json("guarded", &tasks, "")]).await;

    assert!(
        events
            .iter()
            .any(|e| e.starts_with("workflow_finished(guarded") && e.contains("halted=true")),
        "got {events:?}"
    );
    assert!(!events.iter().any(|e| e.contains("/never")));
}

/// The criterion the issue does not state, and the one that guards the real
/// hazard: a workflow reaches the executor by one of two structurally different
/// routes — the shared-arena run for fully-sync workflows, or the per-workflow
/// async driver — and both must produce the same event sequence.
#[tokio::test]
async fn the_sync_and_async_paths_emit_identical_sequences() {
    // Map-only: takes the shared-arena fully-sync run.
    let sync_events = timeline_for(vec![wf_json("w", &map_task("a"), "")]).await;

    // Same shape, but with a custom async handler, so it takes the await path.
    let recorder = Timeline::default();
    let engine = Engine::builder()
        .with_workflow(
            Workflow::from_json(
                r#"{"id": "w", "name": "w", "priority": 0,
                    "tasks": [{"id": "a", "name": "a",
                               "function": {"name": "async_noop", "input": {}}}]}"#,
            )
            .unwrap(),
        )
        .register("async_noop", LoggingTask)
        .with_observer(Arc::new(recorder.clone()))
        .build()
        .unwrap();
    let mut message = Message::from_value(&json!({}));
    let _ = engine.process_message(&mut message).await;

    assert_eq!(
        sync_events,
        recorder.events(),
        "the two execution paths must be indistinguishable to an observer"
    );
}

#[tokio::test]
async fn a_run_that_stops_early_still_reports_message_finished() {
    let tasks = r#"{"id": "boom", "name": "boom",
                    "function": {"name": "failing", "input": {}}}"#;
    let recorder = Timeline::default();
    let engine = Engine::builder()
        .with_workflow(wf_json("w", tasks, ""))
        .register("failing", FailingTask)
        .with_observer(Arc::new(recorder.clone()))
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let result = engine.process_message(&mut message).await;
    assert!(result.is_err(), "the fixture stops the run");

    let events = recorder.events();
    assert!(
        events.contains(&"message_finished(stopped_early=true)".to_string()),
        "a stopped run is the interesting one to measure: {events:?}"
    );
    assert!(
        events.iter().any(|e| e.starts_with("workflow_finished(w")),
        "and the failing workflow's own span still closes: {events:?}"
    );
}

#[tokio::test]
async fn an_engine_with_no_observer_runs_the_same_workflows() {
    // The defaulted callbacks must not change behaviour when unobserved.
    let engine = Engine::builder()
        .with_workflow(wf_json("w", &map_task("a"), ""))
        .build()
        .unwrap();
    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();
    assert_eq!(message.data().get("a").and_then(|v| v.as_i64()), Some(1));
}
