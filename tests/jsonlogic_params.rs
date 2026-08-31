//! Every built-in parameter is JSONLogic — the static spelling unchanged, the
//! computed spelling newly possible.
//!
//! The static spelling is the load-bearing half: a JSON literal *is* JSONLogic
//! for itself, folds to a constant at compile time, and keeps the precomputed
//! write path the hot loop has always used. So a pre-3.9 workflow behaves
//! identically, and only a parameter that actually reads the message pays
//! anything.

mod common;

use common::workflow;
use dataflow_rs::engine::message::Message;
use dataflow_rs::{Engine, Workflow};
use serde_json::{Value, json};

fn wf(tasks: Value) -> Workflow {
    workflow(json!({"id": "w", "name": "w", "priority": 0, "tasks": tasks}))
}

/// Build, run one message seeded with `initial` in `data`, and hand back the
/// message. `data`, not the payload: `payload` is a separate field on `Message`
/// and is not part of the JSONLogic evaluation context.
async fn run(tasks: Value, initial: Value) -> Message {
    let engine = Engine::builder()
        .with_workflow(wf(tasks))
        .build()
        .expect("engine should build");
    let mut message = Message::builder().data_json(&initial).build();
    engine
        .process_message(&mut message)
        .await
        .expect("processing should succeed");
    message
}

fn data(message: &Message) -> Value {
    Value::from(message.data())
}

#[tokio::test]
async fn a_map_destination_can_be_computed_from_the_message() {
    // The shape that motivated this: writing into a slot named by the data.
    let m = run(
        json!([{"id": "t", "name": "t", "function": {"name": "map", "input": {"mappings": [
            {"path": {"cat": ["data.accounts.", {"var": "data.id"}, ".balance"]},
             "logic": 42}
        ]}}}]),
        json!({"id": "ACC7"}),
    )
    .await;

    assert_eq!(data(&m)["accounts"]["ACC7"]["balance"], json!(42));
}

#[tokio::test]
async fn a_computed_destination_is_recorded_on_the_audit_trail() {
    // `Change.path` carries where the value actually went, not the expression.
    // A trail that named the expression would be useless for diffing.
    let engine = Engine::builder()
        .with_workflow(wf(
            json!([{"id": "t", "name": "t", "function": {"name": "map", "input": {"mappings": [
                {"path": {"cat": ["data.slot_", {"var": "data.n"}]}, "logic": true}
            ]}}}]),
        ))
        .build()
        .unwrap();

    let mut message = Message::builder()
        .data_json(&json!({"n": 3}))
        .capture_changes(true)
        .build();
    engine.process_message(&mut message).await.unwrap();

    let paths: Vec<String> = message
        .audit_trail()
        .iter()
        .flat_map(|a| a.changes.iter().map(|c| c.path.to_string()))
        .collect();
    assert!(
        paths.contains(&"data.slot_3".to_string()),
        "audit trail must name the resolved destination, got {paths:?}"
    );
}

#[tokio::test]
async fn a_static_destination_still_writes_exactly_where_it_did() {
    // The no-regression half. A literal path folds to a constant and takes the
    // precomputed route; the observable result must be unchanged.
    let m = run(
        json!([{"id": "t", "name": "t", "function": {"name": "map", "input": {"mappings": [
            {"path": "data.user.name", "logic": "ada"}
        ]}}}]),
        json!({}),
    )
    .await;
    assert_eq!(data(&m)["user"]["name"], json!("ada"));
}

#[tokio::test]
async fn parse_and_publish_targets_can_be_computed() {
    let engine = Engine::builder()
        .with_workflow(wf(json!([
            {"id": "p", "name": "p", "function": {"name": "parse_json", "input": {
                "source": "payload",
                "target": {"cat": ["in_", {"var": "data.slot"}]}
            }}},
            {"id": "q", "name": "q", "function": {"name": "publish_json", "input": {
                "source": {"cat": ["in_", {"var": "data.slot"}]},
                "target": {"cat": ["out_", {"var": "data.slot"}]}
            }}}
        ])))
        .build()
        .expect("engine should build");

    let mut message = Message::builder()
        .payload_json(&json!({"v": 1}))
        .data_json(&json!({"slot": "a"}))
        .build();
    engine.process_message(&mut message).await.unwrap();

    // The payload landed at the computed `data.in_a`, and the serialized form
    // at the computed `data.out_a`.
    assert_eq!(data(&message)["in_a"], json!({"v": 1}));
    assert_eq!(data(&message)["out_a"], json!("{\"v\":1}"));
}

#[tokio::test]
async fn a_validation_message_can_name_the_value_that_failed() {
    // Previously a static string, so an error could say "age must be positive"
    // but never which age.
    let engine = Engine::builder()
        .with_workflow(wf(
            json!([{"id": "t", "name": "t", "continue_on_error": true,
                    "function": {"name": "validation", "input": {"rules": [
                {"logic": {">": [{"var": "data.age"}, 0]},
                 "message": {"cat": ["age must be positive, got ", {"var": "data.age"}]}}
            ]}}}]),
        ))
        .build()
        .unwrap();

    let mut message = Message::builder().data_json(&json!({"age": -3})).build();
    let _ = engine.process_message(&mut message).await;

    let texts: Vec<String> = message.errors().iter().map(|e| e.message.clone()).collect();
    assert!(
        texts.iter().any(|t| t == "age must be positive, got -3"),
        "the message should interpolate the value, got {texts:?}"
    );
}

#[tokio::test]
async fn a_passing_rule_never_renders_its_message() {
    // The message is resolved only on failure, so a computed one costs nothing
    // on the common path. A message that would *error* proves it never ran.
    let engine = Engine::builder()
        .with_workflow(wf(
            json!([{"id": "t", "name": "t", "function": {"name": "validation", "input": {"rules": [
                {"logic": true, "message": {"+": ["not", "a number"]}}
            ]}}}]),
        ))
        .build()
        .unwrap();

    let mut message = Message::builder().data_json(&json!({})).build();
    engine.process_message(&mut message).await.unwrap();
    assert!(
        !message.has_errors(),
        "a passing rule must not evaluate its message: {:?}",
        message.errors()
    );
}

#[test]
fn a_publish_root_element_can_follow_the_message() {
    // Deserializes and builds; the XML writer reads it per message.
    Engine::builder()
        .with_workflow(wf(
            json!([{"id": "t", "name": "t", "function": {"name": "publish_xml", "input": {
                "source": "doc", "target": "xml",
                "root_element": {"var": "data.doc_type"}
            }}}]),
        ))
        .build()
        .expect("a computed root_element must build");
}
