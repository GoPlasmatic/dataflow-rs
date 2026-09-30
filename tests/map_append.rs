//! Appending to an array from `map` (#69): `"mode": "append"` and `"extend"`.
//!
//! The only spelling before was the `merge` idiom, which rebuilds the whole
//! array per append and made a log accumulated over a loop quadratic. These
//! pin that the in-place modes produce what the idiom did, and record per
//! element rather than per array.

use dataflow_rs::engine::message::Message;
use dataflow_rs::{Engine, IssueCode, Workflow};
use serde_json::{Value, json};

fn looped(mapping: Value) -> Engine {
    let workflow = json!({"id": "w", "name": "w", "loop": {"counter": "i", "max": 5}, "tasks": [
        {"id": "log", "name": "log", "function": {"name": "map", "input": {"mappings": [mapping]}}}
    ]});
    Engine::builder()
        .with_workflow(Workflow::from_json(&workflow.to_string()).unwrap())
        .build()
        .unwrap()
}

#[tokio::test]
async fn an_append_in_a_loop_matches_the_merge_idiom() {
    let entry = json!({"turn": {"var": "temp_data.i"}});
    let append = looped(json!({"path": "data.log", "logic": entry, "mode": "append"}));
    let merge = looped(json!({"path": "data.log",
        "logic": {"merge": [{"var": "data.log"}, [entry]]}}));

    let mut a = Message::builder().build();
    append.process_message(&mut a).await.unwrap();
    let mut b = Message::builder().build();
    merge.process_message(&mut b).await.unwrap();

    let expected: Vec<Value> = (0..5).map(|i| json!({"turn": i})).collect();
    assert_eq!(Value::from(&a.context["data"]["log"]), json!(expected));
    assert_eq!(a.context["data"], b.context["data"]);
}

#[tokio::test]
async fn each_append_records_only_its_element() {
    let engine = looped(json!({"path": "data.log",
        "logic": {"var": "temp_data.i"}, "mode": "append"}));
    let mut m = Message::builder().build();
    engine.process_message(&mut m).await.unwrap();

    for (sweep, entry) in m.audit_trail().iter().enumerate() {
        let [change] = entry.changes.as_slice() else {
            panic!("one change per append, got {:?}", entry.changes);
        };
        assert_eq!(change.path.as_ref(), format!("data.log.{sweep}"));
        assert_eq!(Value::from(&change.new_value), json!(sweep));
        assert_eq!(Value::from(&change.old_value), Value::Null);
    }
}

#[tokio::test]
async fn extend_pushes_each_element_and_a_null_result_follows_on_null() {
    let workflow = json!({"id": "w", "name": "w", "tasks": [
        {"id": "t", "name": "t", "function": {"name": "map", "input": {"mappings": [
            {"path": "data.xs", "logic": {"var": "data.more"}, "mode": "extend"},
            {"path": "data.xs", "logic": {"var": "data.missing"}, "mode": "extend"},
            {"path": "data.ys", "logic": {"var": "data.missing"}, "mode": "append",
             "on_null": "unset"}
        ]}}}
    ]});
    let engine = Engine::builder()
        .with_workflow(Workflow::from_json(&workflow.to_string()).unwrap())
        .build()
        .unwrap();
    let mut m = Message::builder()
        .data_json(&json!({"xs": [1], "more": [2, 3], "ys": [9]}))
        .build();
    engine.process_message(&mut m).await.unwrap();

    assert_eq!(Value::from(&m.context["data"]["xs"]), json!([1, 2, 3]));
    assert!(
        m.context["data"].get("ys").is_none(),
        "on_null: unset removes"
    );
    assert!(m.errors().is_empty());
}

#[tokio::test]
async fn an_append_to_a_non_array_fails_the_task_and_leaves_the_value() {
    let engine = looped(json!({"path": "data.n", "logic": 1, "mode": "append"}));
    let mut m = Message::builder().data_json(&json!({"n": 7})).build();
    let result = engine.process_message(&mut m).await;

    assert!(result.is_err(), "a 500 without continue_on_error stops");
    assert_eq!(Value::from(&m.context["data"]["n"]), json!(7));
    assert_eq!(m.audit_trail()[0].status, 500);
}

#[test]
fn validate_authored_reports_the_mode_rules_at_their_keys() {
    let workflow = |mapping: Value| {
        json!({"id": "w", "name": "w", "tasks": [
            {"id": "t", "name": "t", "function": {"name": "map", "input": {"mappings": [mapping]}}}
        ]})
    };
    for (mapping, at) in [
        (
            json!({"path": "data.x", "unset": true, "mode": "append"}),
            "mode",
        ),
        (
            json!({"path": "data", "logic": 1, "mode": "append"}),
            "path",
        ),
        (
            json!({"path": "metadata", "logic": [1], "mode": "extend"}),
            "path",
        ),
    ] {
        let issues = Workflow::validate_authored(&workflow(mapping.clone()));
        let issue = issues
            .iter()
            .find(|i| i.code == IssueCode::InvalidMapping)
            .unwrap_or_else(|| panic!("{mapping}: {issues:?}"));
        assert_eq!(
            issue.path.as_deref(),
            Some(format!("tasks[0].function.input.mappings[0].{at}").as_str()),
            "{mapping}"
        );
    }
    assert!(
        Workflow::validate_authored(&workflow(
            json!({"path": "data.log", "logic": 1, "mode": "append"})
        ))
        .is_empty()
    );
}
