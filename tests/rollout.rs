//! Workflow rollout — traffic splits gated on `Message::routing_bucket`, plus the
//! message-builder seeding they are written against.

use dataflow_rs::engine::message::Message;
use dataflow_rs::{Engine, ExecutionStep, Workflow};
use serde_json::json;

mod common;

use common::{LoggingTask, dv, n_map_task_workflow};

// =============================================================================
// Workflow rollout — traffic splits gated on Message::routing_bucket
// =============================================================================

/// Two map-only workflows splitting the traffic. Fully sync, so these route
/// through `execute_sync_workflow_run`.
fn split_pair_sync() -> Vec<Workflow> {
    vec![
        Workflow::from_json(
            r#"{ "id": "lower", "name": "lower", "priority": 0, "condition": true,
                 "rollout": { "bucket_start": 0, "bucket_end": 50 },
                 "tasks": [ { "id": "lo", "name": "lo", "function": {
                     "name": "map",
                     "input": { "mappings": [ { "path": "data.side", "logic": "lower" } ] } } } ] }"#,
        )
        .unwrap(),
        Workflow::from_json(
            r#"{ "id": "upper", "name": "upper", "priority": 1, "condition": true,
                 "rollout": { "bucket_start": 50, "bucket_end": 100 },
                 "tasks": [ { "id": "hi", "name": "hi", "function": {
                     "name": "map",
                     "input": { "mappings": [ { "path": "data.side", "logic": "upper" } ] } } } ] }"#,
        )
        .unwrap(),
    ]
}

/// The same split, but each workflow carries a custom-handler task so the
/// workflows are *not* fully sync and route through `execute_inner`.
fn split_pair_async() -> Vec<Workflow> {
    vec![
        Workflow::from_json(
            r#"{ "id": "lower", "name": "lower", "priority": 0, "condition": true,
                 "rollout": { "bucket_start": 0, "bucket_end": 50 },
                 "tasks": [
                     { "id": "lo_map", "name": "lo_map", "function": {
                         "name": "map",
                         "input": { "mappings": [ { "path": "data.side", "logic": "lower" } ] } } },
                     { "id": "lo_async", "name": "lo_async", "function": {
                         "name": "logger", "input": {} } } ] }"#,
        )
        .unwrap(),
        Workflow::from_json(
            r#"{ "id": "upper", "name": "upper", "priority": 1, "condition": true,
                 "rollout": { "bucket_start": 50, "bucket_end": 100 },
                 "tasks": [
                     { "id": "hi_map", "name": "hi_map", "function": {
                         "name": "map",
                         "input": { "mappings": [ { "path": "data.side", "logic": "upper" } ] } } },
                     { "id": "hi_async", "name": "hi_async", "function": {
                         "name": "logger", "input": {} } } ] }"#,
        )
        .unwrap(),
    ]
}

fn task_ids(message: &Message) -> Vec<&str> {
    message
        .audit_trail()
        .iter()
        .map(|a| a.task_id.as_ref())
        .collect()
}

#[tokio::test]
async fn rollout_splits_traffic_on_the_fully_sync_path() {
    // Fully-sync workflows route through `execute_sync_workflow_run`.
    let engine = Engine::builder()
        .with_workflows(split_pair_sync())
        .build()
        .unwrap();

    let mut low = Message::builder().routing_bucket(7).build();
    engine.process_message(&mut low).await.unwrap();
    assert_eq!(low.context["data"]["side"], dv(json!("lower")));
    assert_eq!(task_ids(&low), vec!["lo"]);

    let mut high = Message::builder().routing_bucket(77).build();
    engine.process_message(&mut high).await.unwrap();
    assert_eq!(high.context["data"]["side"], dv(json!("upper")));
    assert_eq!(task_ids(&high), vec!["hi"]);
}

#[tokio::test]
async fn rollout_splits_traffic_on_the_async_path() {
    // This is the case that catches a gate installed in only one of the two
    // admission sites: these workflows are not fully sync, so they route through
    // `execute_inner` instead.
    let engine = Engine::builder()
        .with_workflows(split_pair_async())
        .register("logger", LoggingTask)
        .build()
        .unwrap();

    let mut low = Message::builder().routing_bucket(7).build();
    engine.process_message(&mut low).await.unwrap();
    assert_eq!(low.context["data"]["side"], dv(json!("lower")));
    assert_eq!(task_ids(&low), vec!["lo_map", "lo_async"]);

    let mut high = Message::builder().routing_bucket(77).build();
    engine.process_message(&mut high).await.unwrap();
    assert_eq!(high.context["data"]["side"], dv(json!("upper")));
    assert_eq!(task_ids(&high), vec!["hi_map", "hi_async"]);
}

#[tokio::test]
async fn a_workflow_without_a_rollout_runs_for_every_bucket() {
    let mut workflows = split_pair_sync();
    workflows.push(
        Workflow::from_json(
            r#"{ "id": "always", "name": "always", "priority": 2, "condition": true,
                 "tasks": [ { "id": "always_task", "name": "always_task", "function": {
                     "name": "map",
                     "input": { "mappings": [ { "path": "data.always", "logic": true } ] } } } ] }"#,
        )
        .unwrap(),
    );
    let engine = Engine::builder().with_workflows(workflows).build().unwrap();

    for bucket in [0u8, 7, 49, 50, 77, 99] {
        let mut m = Message::builder().routing_bucket(bucket).build();
        engine.process_message(&mut m).await.unwrap();
        assert_eq!(
            m.context["data"]["always"],
            dv(json!(true)),
            "the un-split workflow must run for bucket {bucket}"
        );
    }
}

#[tokio::test]
async fn a_message_with_no_bucket_is_admitted_by_every_split() {
    // The recorded decision: admit. Byte-identical behaviour for every existing
    // caller, and the wasm entry points have no way to set a bucket.
    let engine = Engine::builder()
        .with_workflows(split_pair_sync())
        .build()
        .unwrap();

    let mut m = Message::from_value(&json!({}));
    assert_eq!(m.routing_bucket(), None);
    engine.process_message(&mut m).await.unwrap();

    // Both halves ran; the later one wins the shared key.
    assert_eq!(task_ids(&m), vec!["lo", "hi"]);
    assert_eq!(m.context["data"]["side"], dv(json!("upper")));
}

#[tokio::test]
async fn rollout_is_honoured_on_the_channel_entry_point_too() {
    // The channel path builds a separate Vec<&Workflow>, so cover it explicitly.
    let workflows = vec![
        Workflow::from_json(
            r#"{ "id": "lower", "name": "lower", "priority": 0, "channel": "orders",
                 "condition": true,
                 "rollout": { "bucket_start": 0, "bucket_end": 50 },
                 "tasks": [ { "id": "lo", "name": "lo", "function": {
                     "name": "map",
                     "input": { "mappings": [ { "path": "data.side", "logic": "lower" } ] } } } ] }"#,
        )
        .unwrap(),
        Workflow::from_json(
            r#"{ "id": "upper", "name": "upper", "priority": 1, "channel": "orders",
                 "condition": true,
                 "rollout": { "bucket_start": 50, "bucket_end": 100 },
                 "tasks": [ { "id": "hi", "name": "hi", "function": {
                     "name": "map",
                     "input": { "mappings": [ { "path": "data.side", "logic": "upper" } ] } } } ] }"#,
        )
        .unwrap(),
    ];
    let engine = Engine::builder().with_workflows(workflows).build().unwrap();

    let mut m = Message::builder().routing_bucket(10).build();
    engine
        .process_message_for_channel("orders", &mut m)
        .await
        .unwrap();
    assert_eq!(m.context["data"]["side"], dv(json!("lower")));
    assert_eq!(task_ids(&m), vec!["lo"]);
}

#[tokio::test]
async fn an_excluded_workflow_emits_one_skipped_step_and_no_side_effects() {
    let engine = Engine::builder()
        .with_workflows(split_pair_sync())
        .build()
        .unwrap();

    let mut m = Message::builder().routing_bucket(7).build();
    let trace = engine.process_message_with_trace(&mut m).await.unwrap();

    // The excluded workflow yields exactly one workflow-level Skipped step —
    // identical to a false condition, since no new step reason was added.
    let skipped: Vec<&ExecutionStep> = trace
        .steps
        .iter()
        .filter(|s| s.result == dataflow_rs::StepResult::Skipped)
        .collect();
    assert_eq!(trace.skipped_count(), 1);
    assert_eq!(skipped[0].workflow_id, "upper");
    assert_eq!(
        skipped[0].task_id, None,
        "workflow-level skip carries no task id"
    );

    // No side effects: one audit entry (the admitted workflow's), and
    // metadata.progress names only the admitted task.
    assert_eq!(task_ids(&m), vec!["lo"]);
    assert_eq!(
        m.context["metadata"]["progress"]["workflow_id"],
        dv(json!("lower"))
    );
    assert_eq!(
        m.context["metadata"]["progress"]["task_id"],
        dv(json!("lo"))
    );
}

#[tokio::test]
async fn a_builder_seeded_message_fires_a_data_condition_with_no_parse_task() {
    // #30's repro, inverted: the workflow condition reads `data.*` and the
    // message was seeded through the builder, with no parse_json in the pipeline.
    let wf = Workflow::from_json(
        r#"{ "id": "premium", "name": "premium", "priority": 0,
             "condition": { ">=": [ { "var": "data.order.total" }, 1000 ] },
             "tasks": [ { "id": "discount", "name": "discount", "function": {
                 "name": "map",
                 "input": { "mappings": [
                     { "path": "data.order.discount",
                       "logic": { "*": [ { "var": "data.order.total" }, 0.1 ] } } ] } } } ] }"#,
    )
    .unwrap();

    let engine = Engine::builder().with_workflow(wf).build().unwrap();
    let mut m = Message::builder()
        .data_json(&json!({"order": {"total": 1500}}))
        .build();

    engine.process_message(&mut m).await.unwrap();
    assert_eq!(
        m.context["data"]["order"]["discount"],
        dv(json!(150.0)),
        "a builder-seeded data field must satisfy a data.* condition directly"
    );
}

#[tokio::test]
async fn a_seeded_metadata_survives_processing_and_gains_the_engine_stamps() {
    let engine = Engine::builder()
        .with_workflow(n_map_task_workflow(1))
        .build()
        .unwrap();

    let mut m = Message::builder()
        .metadata_json(&json!({"source": "api", "channel": "seeded"}))
        .build();
    engine.process_message(&mut m).await.unwrap();

    assert_eq!(m.context["metadata"]["source"], dv(json!("api")));
    assert!(m.context["metadata"].get("processed_at").is_some());
    assert!(m.context["metadata"].get("engine_version").is_some());

    // A seeded `channel` key is overwritten by the channel entry point.
    let mut m2 = Message::builder()
        .metadata_json(&json!({"channel": "seeded"}))
        .build();
    engine
        .process_message_for_channel("default", &mut m2)
        .await
        .unwrap();
    assert_eq!(m2.context["metadata"]["channel"], dv(json!("default")));
}
