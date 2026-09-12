//! The `tensor` operator family, once the cargo feature forwards it.
//!
//! Deliberately thin: the 20 operators are datalogic's and are covered by its
//! own conformance battery. What is *this* crate's to prove is that enabling
//! the feature actually reaches the engine, that a `Tensor` survives the
//! `OwnedDataValue` boundary a `Message` is built on, and that the family's
//! collision with ordinary JSON keys behaves the documented way.
#![cfg(feature = "tensor")]

use dataflow_rs::Engine;
use dataflow_rs::engine::message::Message;
use serde_json::json;

mod common;

use common::workflow;

/// Run a single `map` task writing `logic` to `data.out`, and return the
/// message so a caller can read both `data` and `errors()`.
async fn run(logic: serde_json::Value) -> Message {
    let engine = Engine::builder()
        .with_workflow(workflow(json!({
            "id": "w", "name": "w", "priority": 0,
            "tasks": [
                { "id": "t", "name": "t", "function": {
                    "name": "map",
                    "input": { "mappings": [ { "path": "data.out", "logic": logic } ] } } }
            ]
        })))
        .build()
        .unwrap();
    let mut m = Message::from_value(&json!({}));
    engine.process_message(&mut m).await.unwrap();
    m
}

/// `data.out`, projected to JSON.
fn out(m: &Message) -> serde_json::Value {
    serde_json::Value::from(m.data().get("out").unwrap())
}

#[tokio::test]
async fn a_tensor_operator_evaluates_inside_a_workflow() {
    // `to_list` marshals back to ordinary JSON, which is the round trip a
    // host actually performs: JSON in, tensor ops, JSON out.
    let m = run(json!({
        "to_list": [ { "cast": [ { "tensor": [[1.7, 300, -5], "f64"] }, "u8" ] } ]
    }))
    .await;

    assert!(m.errors().is_empty(), "{:?}", m.errors());
    assert_eq!(
        out(&m),
        json!([1, 255, 0]),
        "narrowing should saturate, not wrap"
    );
}

#[tokio::test]
async fn a_tensor_survives_the_owned_value_boundary() {
    // A bare tensor, not marshalled back by `to_list`: it is written into the
    // message context as `OwnedDataValue::Tensor` and only becomes JSON when
    // the context is projected. If the `datavalue/tensor` forward were
    // missing, this is what would fail to compile or would flatten to null.
    let m = run(json!({ "tensor": [[1, 2, 3], "u8"] })).await;

    assert!(m.errors().is_empty(), "{:?}", m.errors());
    let value = out(&m);
    assert!(
        !value.is_null(),
        "a tensor must not flatten to null crossing OwnedDataValue: {value}"
    );
}

#[tokio::test]
async fn shape_is_a_live_operator_once_the_feature_is_on() {
    // The reason `tensor` is not in `all-operators`. Under templating mode a
    // single-key object whose key is a live operator *evaluates* — so with the
    // feature on, `{"shape": ...}` is the tensor `shape` operator and no
    // longer passes through as literal data.
    //
    // This is the pinned trigger for that trade, not an accident: if the
    // family is ever folded into `all-operators`, this test is the reminder of
    // what that costs, and `{"$shape": ...}` is the documented escape.
    let m = run(json!({ "shape": [ { "tensor": [[1, 2, 3], "u8"] } ] })).await;
    assert_eq!(
        out(&m),
        json!([3]),
        "`shape` should have evaluated, not echoed back as data"
    );

    let escaped = run(json!({ "$shape": [1, 2, 3] })).await;
    assert_eq!(
        out(&escaped),
        json!({ "shape": [1, 2, 3] }),
        "the `$` escape must still pin the literal reading"
    );
}
