//! Removing a path from `map` (#59): `unset: true`, and `on_null: "unset"`.
//!
//! A null result is skipped, which is what lets `{"if": [cond, value, null]}`
//! mean "set or keep" — so `null` cannot also mean "clear". These pin the two
//! explicit spellings that do clear, and that the skip default is untouched.

use dataflow_rs::engine::message::Message;
use dataflow_rs::{Engine, Workflow};
use serde_json::{Value, json};

mod common;

use common::dv;

fn map_task(id: &str, mappings: Value) -> Value {
    json!({"id": id, "name": id, "function": {"name": "map", "input": {"mappings": mappings}}})
}

fn engine(workflows: Vec<Value>) -> Engine {
    Engine::builder()
        .with_workflows(
            workflows
                .into_iter()
                .map(|w| Workflow::from_json(&w.to_string()).unwrap())
                .collect::<Vec<_>>(),
        )
        .build()
        .unwrap()
}

fn one_task(mappings: Value) -> Engine {
    engine(vec![
        json!({"id": "w", "name": "w", "tasks": [map_task("t", mappings)]}),
    ])
}

fn message(data: Value, temp_data: Value) -> Message {
    Message::builder()
        .data(dv(data))
        .temp_data(dv(temp_data))
        .build()
}

#[tokio::test]
async fn unset_removes_the_key_and_records_a_removal() {
    let engine = one_task(json!([{"path": "temp_data.x", "unset": true}]));
    let mut m = message(json!({}), json!({"x": "stale", "y": 1}));
    engine.process_message(&mut m).await.unwrap();

    assert_eq!(Value::from(m.temp_data()), json!({"y": 1}));

    let changes = &m.audit_trail()[0].changes;
    assert_eq!(changes.len(), 1);
    assert!(changes[0].removed);
    assert_eq!(changes[0].path.as_ref(), "temp_data.x");
    assert_eq!(Value::from(&changes[0].old_value), json!("stale"));

    // On the wire a removal is `removed: true` beside a null `new_value`, so a
    // reader that predates the flag still loads it.
    let wire = serde_json::to_value(&changes[0]).unwrap();
    assert_eq!(
        wire,
        json!({"path": "temp_data.x", "old_value": "stale", "new_value": null, "removed": true})
    );
}

#[tokio::test]
async fn unset_of_an_absent_key_is_a_no_op_that_records_nothing() {
    let engine = one_task(json!([{"path": "temp_data.never.there", "unset": true}]));
    let mut m = message(json!({}), json!({"x": 1}));
    engine.process_message(&mut m).await.unwrap();

    assert_eq!(Value::from(m.temp_data()), json!({"x": 1}));
    let entry = &m.audit_trail()[0];
    assert_eq!(entry.status, 200);
    assert!(entry.changes.is_empty());
    assert!(m.errors().is_empty());
}

#[tokio::test]
async fn a_removal_is_seen_by_the_next_mapping_and_the_next_workflow() {
    // Both reads run against the arena cache, not the owned context, so this
    // is what proves the cache followed the removal.
    let engine = engine(vec![
        json!({"id": "w1", "name": "w1", "priority": 0, "tasks": [map_task("t", json!([
            {"path": "temp_data.x", "unset": true},
            {"path": "data.missing_after", "logic": {"missing": ["temp_data.x"]}}
        ]))]}),
        json!({"id": "w2", "name": "w2", "priority": 1,
               "condition": {"==": [{"var": ["temp_data.x", "gone"]}, "gone"]},
               "tasks": [map_task("t2", json!([{"path": "data.w2_ran", "logic": true}]))]}),
    ]);
    let mut m = message(json!({}), json!({"x": "stale"}));
    engine.process_message(&mut m).await.unwrap();

    assert_eq!(
        Value::from(m.data()),
        json!({"missing_after": ["temp_data.x"], "w2_ran": true})
    );
}

#[tokio::test]
async fn deep_and_array_removals() {
    let engine = one_task(json!([
        {"path": "data.a.b.c", "unset": true},
        {"path": "data.items.0", "unset": true},
        {"path": "data.read_back", "logic": {"cat": [{"var": ["data.a.b.c", "-"]}, {"var": "data.items.0"}]}}
    ]));
    let mut m = message(
        json!({"a": {"b": {"c": 1, "d": 2}}, "items": ["first", "second"]}),
        json!({}),
    );
    engine.process_message(&mut m).await.unwrap();

    // Array removal shifts later elements down, as `remove_nested_value` does.
    assert_eq!(
        Value::from(m.data()),
        json!({"a": {"b": {"d": 2}}, "items": ["second"], "read_back": "-second"})
    );
}

#[tokio::test]
async fn on_null_unset_clears_on_null_and_writes_otherwise() {
    let engine = one_task(json!([{
        "path": "temp_data.reason",
        "logic": {"if": [{"var": "data.ok"}, null, "FAILED"]},
        "on_null": "unset"
    }]));

    let mut ok = message(json!({"ok": true}), json!({"reason": "old"}));
    engine.process_message(&mut ok).await.unwrap();
    assert_eq!(Value::from(ok.temp_data()), json!({}));
    assert!(ok.audit_trail()[0].changes[0].removed);

    let mut failed = message(json!({"ok": false}), json!({"reason": "old"}));
    engine.process_message(&mut failed).await.unwrap();
    assert_eq!(Value::from(failed.temp_data()), json!({"reason": "FAILED"}));
    assert!(!failed.audit_trail()[0].changes[0].removed);
}

#[tokio::test]
async fn a_null_result_is_still_skipped_by_default() {
    // "Set or keep" is load-bearing; neither new key may change it.
    let engine = one_task(json!([
        {"path": "temp_data.a", "logic": {"if": [{"var": "data.go"}, "new", null]}},
        {"path": "temp_data.b", "logic": {"var": "data.go_missing"}, "on_null": "skip"},
        {"path": "temp_data.c", "logic": null}
    ]));
    let mut m = message(json!({"go": false}), json!({"a": 1, "b": 2, "c": 3}));
    engine.process_message(&mut m).await.unwrap();

    assert_eq!(Value::from(m.temp_data()), json!({"a": 1, "b": 2, "c": 3}));
    assert!(m.audit_trail()[0].changes.is_empty());
}

/// The report in #59: a loop's per-item slot set with "set or keep" leaks the
/// previous item's value into every later sweep that does not set it.
fn per_item_loop(slot_mapping: Value, clear: Option<Value>) -> Engine {
    let mut body = vec![
        map_task("pick", json!([slot_mapping])),
        map_task(
            "register",
            json!([{"path": "data.registered", "logic":
                {"if": [{"var": "temp_data.slot"},
                        {"merge": [{"var": "data.registered"}, [{"var": "temp_data.slot"}]]},
                        {"var": "data.registered"}]}}]),
        ),
    ];
    if let Some(clear) = clear {
        body.push(map_task("clear", json!([clear])));
    }
    engine(vec![
        json!({"id": "setup", "name": "setup", "priority": 0, "tasks": [map_task("init", json!([
            {"path": "temp_data.n", "logic":
                {"reduce": [{"var": "data.items"}, {"+": [{"var": "accumulator"}, 1]}, 0]}},
            {"path": "data.registered", "logic": []}
        ]))]}),
        json!({"id": "per_item", "name": "per_item", "priority": 1,
               "condition": {"<": [{"var": "temp_data.i"}, {"var": "temp_data.n"}]},
               "loop": {"counter": "i", "max": 100},
               "tasks": body}),
    ])
}

fn items() -> Message {
    message(
        json!({"items": [
            {"id": "a", "ok": true}, {"id": "b", "ok": false}, {"id": "c", "ok": false}
        ]}),
        json!({}),
    )
}

fn slot_if_ok(on_null: Option<&str>) -> Value {
    let mut mapping = json!({"path": "temp_data.slot", "logic":
        {"if": [{"val": [["data", "items", {"var": "temp_data.i"}, "ok"]]},
                {"val": [["data", "items", {"var": "temp_data.i"}, "id"]]},
                null]}});
    if let Some(on_null) = on_null {
        mapping["on_null"] = json!(on_null);
    }
    mapping
}

#[tokio::test]
async fn the_loop_slot_from_the_report_leaks_with_logic_null_and_clears_with_unset() {
    // As reported: `"logic": null` looks like a clear and does nothing.
    let leaky = per_item_loop(
        slot_if_ok(None),
        Some(json!({"path": "temp_data.slot", "logic": null})),
    );
    let mut m = items();
    leaky.process_message(&mut m).await.unwrap();
    assert_eq!(
        Value::from(&m.data()["registered"]),
        json!(["a", "a", "a"]),
        "the unchanged behaviour the issue describes"
    );

    // Fix one: clear the slot at the end of every sweep.
    let cleared = per_item_loop(
        slot_if_ok(None),
        Some(json!({"path": "temp_data.slot", "unset": true})),
    );
    let mut m = items();
    cleared.process_message(&mut m).await.unwrap();
    assert_eq!(Value::from(&m.data()["registered"]), json!(["a"]));

    // Fix two: make the slot's own null mean "clear".
    let set_or_clear = per_item_loop(slot_if_ok(Some("unset")), None);
    let mut m = items();
    set_or_clear.process_message(&mut m).await.unwrap();
    assert_eq!(Value::from(&m.data()["registered"]), json!(["a"]));
}

#[tokio::test]
async fn a_computed_path_resolving_to_a_root_fails_the_mapping_and_leaves_the_root() {
    // A literal root is refused at parse time; a computed one can only be
    // caught when it resolves.
    let engine = engine(vec![json!({"id": "w", "name": "w", "tasks": [{
        "id": "t", "name": "t", "continue_on_error": true,
        "function": {"name": "map", "input": {"mappings": [
            {"path": {"cat": ["temp", "_data"]}, "unset": true},
            {"path": "data.after", "logic": "ran"}
        ]}}
    }]})]);
    let mut m = message(json!({}), json!({"x": 1}));
    engine.process_message(&mut m).await.unwrap();

    assert_eq!(Value::from(m.temp_data()), json!({"x": 1}));
    assert_eq!(m.audit_trail()[0].status, 500);
    assert_eq!(
        Value::from(m.data()),
        json!({"after": "ran"}),
        "one failed mapping does not stop the rest, as with a failed write"
    );
}

#[tokio::test]
async fn removal_still_happens_with_audit_capture_off() {
    let engine = one_task(json!([{"path": "temp_data.x", "unset": true}]));
    let mut m = Message::builder()
        .temp_data(dv(json!({"x": 1})))
        .capture_changes(false)
        .build();
    engine.process_message(&mut m).await.unwrap();

    assert_eq!(Value::from(m.temp_data()), json!({}));
    assert!(m.audit_trail()[0].changes.is_empty());
}
