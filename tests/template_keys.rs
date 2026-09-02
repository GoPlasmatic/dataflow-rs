//! The template-key escape (`$`) and the authoring checks that came with it.
//!
//! Templating mode makes every single-key object an operator invocation, so
//! before 3.9 a literal object whose key collided with an operator name was
//! **inexpressible**. That one fact forced the `path`/`path_logic` field pairs,
//! kept `Template` opt-in per field, and made enabling an operator family a
//! breaking change for existing data. The escape is what removes it.
//!
//! These tests pin the escape end-to-end through a real engine — not against a
//! bare `datalogic_rs::Engine` — so they fail if the engine this crate builds
//! ever stops configuring it.

mod common;

use common::workflow;
use dataflow_rs::engine::message::Message;
use dataflow_rs::{Engine, IssueCode, Severity, Workflow};
use serde_json::{Value, json};

/// Run one `map` mapping and return what landed at `data.out`.
async fn mapped(logic: Value) -> Value {
    let engine = Engine::builder()
        .with_workflow(workflow(json!({
            "id": "w", "name": "w", "priority": 0,
            "tasks": [{"id": "t", "name": "t", "function": {"name": "map", "input": {
                "mappings": [{"path": "data.out", "logic": logic}]
            }}}]
        })))
        .build()
        .expect("engine should build");

    let mut message = Message::from_value(&json!({}));
    engine
        .process_message(&mut message)
        .await
        .expect("processing should succeed");
    Value::from(message.data()).get("out").cloned().unwrap()
}

#[tokio::test]
async fn an_escaped_key_emits_the_literal_object() {
    // The whole feature in one assertion: `cat` is a core operator, so
    // unescaped it concatenates. Escaped, it is a field name.
    assert_eq!(mapped(json!({"cat": ["a", "b"]})).await, json!("ab"));
    assert_eq!(
        mapped(json!({"$cat": ["a", "b"]})).await,
        json!({"cat": ["a", "b"]})
    );
}

#[tokio::test]
async fn exactly_one_prefix_is_stripped() {
    // So a key that genuinely starts with `$` — MongoDB's `$oid`, JSON
    // Schema's `$ref` — is written by doubling it.
    assert_eq!(
        mapped(json!({"$$oid": "abc"})).await,
        json!({"$oid": "abc"})
    );
    assert_eq!(
        mapped(json!({"$$$oid": "abc"})).await,
        json!({"$$oid": "abc"})
    );
}

#[tokio::test]
async fn the_escape_applies_at_every_depth_and_inside_operators() {
    // Not just at the root of an expression: nested in a template, inside an
    // `if` branch, and inside a `map` body.
    assert_eq!(
        mapped(json!({"outer": {"$cat": ["a", "b"]}})).await,
        json!({"outer": {"cat": ["a", "b"]}})
    );
    assert_eq!(
        mapped(json!({"if": [true, {"$cat": ["y"]}, 0]})).await,
        json!({"cat": ["y"]})
    );
    assert_eq!(
        mapped(json!({"map": [[1, 2], {"$v": {"var": ""}}]})).await,
        json!([{"v": 1}, {"v": 2}])
    );
}

#[tokio::test]
async fn escaping_leaves_non_colliding_keys_alone_but_still_strips_them() {
    // The cost of a uniform rule: stripping is not conditional on the key
    // colliding with an operator. `$total` is not an operator name and is still
    // stripped. This is what a pre-3.9 workflow emitting `$`-keys runs into.
    assert_eq!(mapped(json!({"$total": 1})).await, json!({"total": 1}));
    assert_eq!(mapped(json!({"total": 1})).await, json!({"total": 1}));
}

/// A workflow whose single map mapping produces `logic`.
fn wf_with(logic: Value) -> Workflow {
    workflow(json!({
        "id": "w", "name": "w", "priority": 0,
        "tasks": [{"id": "t", "name": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.out", "logic": logic}]
        }}}]
    }))
}

#[test]
fn keys_that_collide_after_stripping_are_refused_at_build() {
    // `{"$a": 1, "a": 2}` emits `a` twice. The context is a Vec of pairs, so
    // both survive: a later read sees only the first while serialization emits
    // both. There is no reading of that which is intended.
    let w = wf_with(json!({"$a": 1, "a": 2}));

    let issues = Engine::builder().check_workflow(&w);
    let collision = issues
        .iter()
        .find(|i| i.code == IssueCode::DuplicateTemplateKey)
        .unwrap_or_else(|| panic!("check_workflow must report the collision: {issues:?}"));
    assert_eq!(collision.severity(), Severity::Rejected);

    let err = match Engine::builder().with_workflow(w).build() {
        Err(e) => e,
        Ok(_) => panic!("build must refuse a duplicate template key"),
    };
    assert!(err.to_string().contains("emit the key 'a'"), "{err}");
}

#[test]
fn escaped_keys_are_reported_for_audit_but_never_refused() {
    // The migration surface. A host upgrading to 3.9 lists every place the
    // escape changed what a template emits — but after migration an escaped key
    // is exactly what the author meant, so `build()` must not refuse it.
    let w = wf_with(json!({"$oid": {"var": "data.id"}, "kind": "ref"}));

    let issues = Engine::builder().check_workflow(&w);
    let escaped: Vec<_> = issues
        .iter()
        .filter(|i| i.code == IssueCode::EscapedTemplateKey)
        .collect();
    assert_eq!(escaped.len(), 1, "one escaped key, got {issues:?}");
    assert_eq!(
        escaped[0].path.as_deref(),
        Some("function.input.mappings[0].logic.$oid")
    );
    assert_eq!(escaped[0].task_id.as_deref(), Some("t"));
    assert!(
        escaped[0].severity() == Severity::Advisory,
        "the escape is the sanctioned spelling, not a defect"
    );
    assert!(
        escaped[0].message.contains("emitted as 'oid'"),
        "the message must say what it becomes: {}",
        escaped[0].message
    );

    Engine::builder()
        .with_workflow(w)
        .build()
        .expect("an escaped key is legal — it is the sanctioned spelling");
}

#[test]
fn an_ordinary_single_key_output_template_is_not_reported() {
    // Guards against a lint that was designed and dropped. In templating mode
    // an unrecognised single key is *not* inert — it evaluates its argument and
    // emits a structured object — so `{"result": {"var": …}}` is the ordinary
    // single-key output template, indistinguishable from a typo. Reporting it
    // would fire on almost every correct workflow.
    let issues = Engine::builder().check_workflow(&wf_with(json!({"result": {"var": "data.x"}})));
    assert!(
        issues.is_empty(),
        "no issue for a normal template: {issues:?}"
    );
}

#[test]
fn a_custom_tasks_input_is_config_not_a_template() {
    // A custom task's `input` is handed to its handler untyped; which of its
    // fields are JSONLogic is the handler's business. Treating the whole
    // document as a template would refuse valid configs, since
    // `DuplicateTemplateKey` is fatal.
    let w = workflow(json!({
        "id": "w", "name": "w", "priority": 0,
        "tasks": [{"id": "t", "name": "t", "function": {"name": "logger", "input": {
            "$a": 1, "a": 2
        }}}]
    }));
    Engine::builder()
        .register("logger", common::LoggingTask)
        .with_workflow(w)
        .build()
        .expect("a custom input is config, not a template");
}
