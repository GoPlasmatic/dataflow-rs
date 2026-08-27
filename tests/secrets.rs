//! `{"secret": "name"}` — the reserved operator that reads an engine-scoped
//! secret store, so a workflow can use a value the engine never records.

use dataflow_rs::datalogic_rs::bumpalo::Bump;
use dataflow_rs::engine::message::Message;
use dataflow_rs::{Engine, Workflow};
use serde_json::{Value, json};

mod common;

use common::dv;

const SECRET: &str = "s3cr3t-XYZ";

fn secrets() -> Value {
    json!({
        "partner_key": SECRET,
        "partner": { "hmac": "nested-hmac-value" }
    })
}

/// Evaluate `expr` against `data` on the engine's own datalogic instance —
/// the same path a handler calling `ctx.datalogic()` takes.
fn eval(engine: &Engine, expr: Value, data: Value) -> Result<Value, String> {
    let logic = engine
        .datalogic()
        .compile_arc(&expr)
        .map_err(|e| e.to_string())?;
    let arena = Bump::new();
    engine
        .datalogic()
        .evaluate(&logic, &data, &arena)
        .map(|v| serde_json::to_value(v).unwrap())
        .map_err(|e| e.to_string())
}

fn gated_workflow(condition: Value) -> Workflow {
    Workflow::from_json(
        &json!({
            "id": "w", "name": "w", "priority": 0,
            "condition": condition,
            "tasks": [
                { "id": "mark", "name": "mark", "function": {
                    "name": "map",
                    "input": { "mappings": [ { "path": "data.ran", "logic": true } ] } } }
            ]
        })
        .to_string(),
    )
    .unwrap()
}

// =============================================================================
// Resolution
// =============================================================================

#[tokio::test]
async fn a_secret_resolves_in_a_workflow_condition() {
    let engine = Engine::builder()
        .with_secrets_json(&secrets())
        .with_workflow(gated_workflow(
            json!({ "==": [ { "secret": "partner_key" }, SECRET ] }),
        ))
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(message.context["data"]["ran"], dv(json!(true)));
}

#[test]
fn a_secret_resolves_through_the_engines_datalogic_directly() {
    let engine = Engine::builder()
        .with_secrets_json(&secrets())
        .build()
        .unwrap();

    assert_eq!(
        eval(&engine, json!({ "secret": "partner_key" }), json!({})),
        Ok(json!(SECRET))
    );
    // Dotted paths walk into a nested store, so hosts can namespace.
    assert_eq!(
        eval(&engine, json!({ "secret": "partner.hmac" }), json!({})),
        Ok(json!("nested-hmac-value"))
    );
    // The single-element array form JSONLogic allows for every operator.
    assert_eq!(
        eval(&engine, json!({ "secret": ["partner_key"] }), json!({})),
        Ok(json!(SECRET))
    );
}

#[test]
fn a_dynamic_key_resolves_at_runtime() {
    let engine = Engine::builder()
        .with_secrets_json(&secrets())
        .build()
        .unwrap();

    assert_eq!(
        eval(
            &engine,
            json!({ "secret": { "var": "data.key_name" } }),
            json!({ "data": { "key_name": "partner_key" } })
        ),
        Ok(json!(SECRET))
    );
}

#[test]
fn an_unknown_key_is_an_error_that_names_the_key_and_no_value() {
    let engine = Engine::builder()
        .with_secrets_json(&secrets())
        .build()
        .unwrap();

    let err = eval(&engine, json!({ "secret": "nope" }), json!({})).unwrap_err();
    assert!(err.contains("nope"), "error should name the key: {err}");
    assert!(!err.contains(SECRET), "error must not leak a value: {err}");

    // Not null-for-missing: a missing key must never silently sign with nothing.
    let err = eval(
        &engine,
        json!({ "secret": { "var": "data.key_name" } }),
        json!({ "data": { "key_name": "nope" } }),
    )
    .unwrap_err();
    assert!(err.contains("nope"), "{err}");
}

#[test]
fn malformed_arguments_are_errors_and_never_the_whole_store() {
    let engine = Engine::builder()
        .with_secrets_json(&secrets())
        .build()
        .unwrap();

    for expr in [
        json!({ "secret": "" }),
        json!({ "secret": [] }),
        json!({ "secret": ["partner_key", "partner.hmac"] }),
        json!({ "secret": 42 }),
        json!({ "secret": null }),
    ] {
        let err = eval(&engine, expr.clone(), json!({})).unwrap_err();
        assert!(
            !err.contains(SECRET) && !err.contains("nested-hmac-value"),
            "{expr} leaked a value: {err}"
        );
    }
}

#[tokio::test]
async fn an_unresolvable_secret_makes_a_condition_false_not_a_crash() {
    let engine = Engine::builder()
        .with_secrets_json(&secrets())
        .with_workflow(gated_workflow(
            json!({ "==": [ { "secret": { "var": "data.key_name" } }, SECRET ] }),
        ))
        .build()
        .unwrap();

    let mut message = Message::builder()
        .data_json(&json!({ "key_name": "nope" }))
        .build();
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(message.context["data"]["ran"], dv(json!(null)));
}

#[test]
fn a_secret_keeps_its_json_type_and_its_bytes() {
    let engine = Engine::builder()
        .with_secrets_json(&json!({
            "n": 4242,
            "f": 1.5,
            "b": true,
            "list": [1, "two"],
            "obj": { "a": { "b": "deep" } },
            "unicode": "clé-秘密-🔑",
            "escaped": { "20": "twenty", "#": "hash" }
        }))
        .build()
        .unwrap();

    for (name, expected) in [
        ("n", json!(4242)),
        ("f", json!(1.5)),
        ("b", json!(true)),
        ("list", json!([1, "two"])),
        ("list.1", json!("two")),
        ("obj", json!({ "a": { "b": "deep" } })),
        ("obj.a.b", json!("deep")),
        ("unicode", json!("clé-秘密-🔑")),
        // Same `#` escape as every other dotted path in the crate.
        ("escaped.#20", json!("twenty")),
        ("escaped.##", json!("hash")),
    ] {
        assert_eq!(
            eval(&engine, json!({ "secret": name }), json!({})),
            Ok(expected),
            "{name}"
        );
    }
}

#[test]
fn the_last_with_secrets_call_wins() {
    let engine = Engine::builder()
        .with_secrets_json(&json!({ "old": "o" }))
        .with_secrets_json(&json!({ "new": "n" }))
        .build()
        .unwrap();

    assert_eq!(engine.declared_secrets().collect::<Vec<_>>(), vec!["new"]);
    assert_eq!(
        eval(&engine, json!({ "secret": "new" }), json!({})),
        Ok(json!("n"))
    );
    assert!(eval(&engine, json!({ "secret": "old" }), json!({})).is_err());
}

// =============================================================================
// Engine surface
// =============================================================================

#[test]
fn declared_secrets_lists_names_never_values() {
    let engine = Engine::builder()
        .with_secrets_json(&secrets())
        .build()
        .unwrap();

    let mut names: Vec<&str> = engine.declared_secrets().collect();
    names.sort_unstable();
    assert_eq!(names, vec!["partner", "partner_key"]);

    let bare = Engine::builder().build().unwrap();
    assert_eq!(bare.declared_secrets().count(), 0);
}

#[test]
fn a_non_object_store_fails_build() {
    for value in [json!("nope"), json!(["a"]), json!(42), json!(null)] {
        let err = Engine::builder()
            .with_secrets_json(&value)
            .build()
            .err()
            .unwrap_or_else(|| panic!("{value} must not build"));
        assert!(err.to_string().contains("object"), "{err}");
    }
}

#[test]
fn secret_is_part_of_the_operator_vocabulary_on_every_engine() {
    // Registered even with no store, so `{"secret": "k"}` is never inert data.
    let bare = Engine::builder().build().unwrap();
    assert!(bare.operator_names().any(|n| n == "secret"));
    assert_eq!(bare.operator_names().filter(|n| *n == "secret").count(), 1);

    let err = eval(&bare, json!({ "secret": "partner_key" }), json!({})).unwrap_err();
    assert!(err.contains("partner_key"), "{err}");
}

#[test]
fn registering_an_operator_named_secret_fails_build() {
    let err = Engine::builder()
        .with_datalogic_operator("secret", common_ops::Shout)
        .build()
        .err()
        .expect("`secret` is reserved");
    assert!(err.to_string().contains("reserved"), "{err}");
}

#[tokio::test]
async fn the_store_survives_a_hot_reload() {
    let engine = Engine::builder()
        .with_secrets_json(&secrets())
        .build()
        .unwrap();

    let reloaded = engine
        .with_new_workflows(vec![gated_workflow(
            json!({ "==": [ { "secret": "partner_key" }, SECRET ] }),
        )])
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    reloaded.process_message(&mut message).await.unwrap();
    assert_eq!(message.context["data"]["ran"], dv(json!(true)));
    assert_eq!(reloaded.declared_secrets().count(), 2);
}

// =============================================================================
// Handler surface
// =============================================================================

#[derive(serde::Deserialize)]
struct KeyedInput {
    key_name: String,
}

/// Reads a secret by *name* from config and writes a derived, non-secret
/// value — the shape of a handler that takes a key reference rather than a
/// `Template`.
struct KeyLengthHandler;

#[async_trait::async_trait]
impl dataflow_rs::engine::functions::AsyncFunctionHandler for KeyLengthHandler {
    type Input = KeyedInput;

    async fn execute(
        &self,
        ctx: &mut dataflow_rs::TaskContext<'_>,
        input: &Self::Input,
    ) -> dataflow_rs::Result<dataflow_rs::TaskOutcome> {
        let len = ctx
            .secret(&input.key_name)
            .and_then(|v| v.as_str())
            .map(str::len)
            .ok_or_else(|| dataflow_rs::DataflowError::Task("no such secret".into()))?;
        ctx.set("data.key_len", dv(json!(len)));
        Ok(dataflow_rs::TaskOutcome::Success)
    }
}

#[tokio::test]
async fn a_handler_reads_a_secret_by_name_through_task_context() {
    let wf = Workflow::from_json(
        &json!({
            "id": "w", "name": "w", "priority": 0,
            "tasks": [ { "id": "t", "name": "t", "function": {
                "name": "key_len", "input": { "key_name": "partner_key" } } } ]
        })
        .to_string(),
    )
    .unwrap();
    let engine = Engine::builder()
        .with_secrets_json(&secrets())
        .with_workflow(wf)
        .register("key_len", KeyLengthHandler)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();
    assert_eq!(message.context["data"]["key_len"], dv(json!(SECRET.len())));
}

#[test]
fn a_task_context_built_outside_the_engine_has_no_secrets() {
    let dl = std::sync::Arc::new(dataflow_rs::datalogic_rs::Engine::new());
    let mut message = Message::from_value(&json!({}));
    let ctx = dataflow_rs::TaskContext::new(&mut message, &dl);
    assert!(ctx.secret("partner_key").is_none());
}

mod common_ops {
    pub struct Shout;

    impl dataflow_rs::datalogic_rs::CustomOperator for Shout {
        fn evaluate<'a>(
            &self,
            args: &[&'a dataflow_rs::datalogic_rs::DataValue<'a>],
            _ctx: &mut dataflow_rs::datalogic_rs::operator::EvalContext<'_, 'a>,
            arena: &'a dataflow_rs::datalogic_rs::bumpalo::Bump,
        ) -> dataflow_rs::datalogic_rs::Result<&'a dataflow_rs::datalogic_rs::DataValue<'a>>
        {
            use dataflow_rs::datalogic_rs::ArenaExt;
            let s = args.first().and_then(|v| v.as_str()).unwrap_or_default();
            Ok(arena.string(&s.to_uppercase()))
        }
    }
}

// =============================================================================
// Static checks — `check_workflow` and `build()`
// =============================================================================

use dataflow_rs::IssueCode;

fn wf(tasks: Value) -> Workflow {
    Workflow::from_json(
        &json!({ "id": "w", "name": "w", "priority": 0, "tasks": tasks }).to_string(),
    )
    .unwrap()
}

fn map_task(id: &str, logic: Value) -> Value {
    json!({ "id": id, "name": id, "function": {
        "name": "map",
        "input": { "mappings": [
            { "path": "data.a", "logic": 1 },
            { "path": "data.b", "logic": logic } ] } } })
}

/// `(code, path, task_id)` triples, sorted, for order-independent comparison.
fn triples(issues: &[dataflow_rs::WorkflowIssue]) -> Vec<(IssueCode, String, Option<String>)> {
    let mut out: Vec<_> = issues
        .iter()
        .map(|i| {
            (
                i.code,
                i.path.clone().unwrap_or_default(),
                i.task_id.clone(),
            )
        })
        .collect();
    out.sort_by(|a, b| (a.1.as_str(), &a.2).cmp(&(b.1.as_str(), &b.2)));
    out
}

#[test]
fn an_undeclared_literal_key_is_reported_wherever_an_expression_lives() {
    let workflow = Workflow::from_json(
        &json!({
            "id": "w", "name": "w", "priority": 0,
            "condition": { "==": [ { "secret": "nope_wf" }, 1 ] },
            "tasks": [
                { "id": "gated", "name": "gated",
                  "condition": { "secret": "nope_task" },
                  "function": { "name": "validation", "input": { "rules": [
                      { "logic": { "==": [ { "secret": "nope_rule" }, 1 ] }, "message": "m" } ] } } },
                { "id": "gate", "name": "gate", "function": {
                    "name": "filter", "input": { "condition": { "secret": ["nope_filter"] } } } },
                { "id": "call", "name": "call", "function": {
                    "name": "http_call", "input": {
                        "connector": "c", "method": "GET",
                        "path_logic": { "cat": ["/v1/", { "secret": "nope_http" }] } } } },
                { "id": "custom", "name": "custom", "function": {
                    "name": "logger", "input": {
                        "headers": { "Authorization": { "secret": "nope_custom" } },
                        "declared": { "secret": "partner_key" } } } }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let builder = Engine::builder()
        .with_secrets_json(&secrets())
        .register("logger", common::LoggingTask)
        .register("http_call", common::LoggingTask);
    let issues = builder.check_workflow(&workflow);

    let unknown: Vec<_> = triples(&issues)
        .into_iter()
        .filter(|(code, _, _)| *code == IssueCode::UnknownSecret)
        .map(|(_, path, task)| (path, task))
        .collect();
    assert_eq!(
        unknown,
        vec![
            ("condition".to_string(), None),
            ("condition".to_string(), Some("gated".to_string())),
            (
                "function.input.condition".to_string(),
                Some("gate".to_string())
            ),
            (
                "function.input.headers.Authorization".to_string(),
                Some("custom".to_string())
            ),
            (
                "function.input.path_logic".to_string(),
                Some("call".to_string())
            ),
            (
                "function.input.rules[0].logic".to_string(),
                Some("gated".to_string())
            ),
        ]
    );
    // Every message names the key it could not find.
    for issue in issues.iter().filter(|i| i.code == IssueCode::UnknownSecret) {
        assert!(issue.message.contains("nope_"), "{issue}");
    }

    // `build()` refuses the same workflow, naming a key.
    let err = builder.with_workflow(workflow).build().err().unwrap();
    assert!(err.to_string().contains("nope_"), "{err}");
}

#[test]
fn a_bare_engine_reports_every_literal_key_as_undeclared() {
    let workflow = wf(json!([ { "id": "t", "name": "t",
        "condition": { "secret": "partner_key" },
        "function": { "name": "map", "input": { "mappings": [] } } } ]));

    let issues = Engine::builder().check_workflow(&workflow);
    assert_eq!(
        triples(&issues),
        vec![(
            IssueCode::UnknownSecret,
            "condition".into(),
            Some("t".into())
        )]
    );
    assert!(Engine::builder().with_workflow(workflow).build().is_err());
}

#[test]
fn a_secret_in_a_map_mapping_is_refused_even_when_declared_or_dynamic() {
    let workflow = wf(json!([
        map_task("literal", json!({ "secret": "partner_key" })),
        map_task("dynamic", json!({ "secret": { "var": "data.k" } })),
        map_task(
            "wrapped",
            json!({ "cat": ["sig=", { "hmac": [ { "secret": "partner_key" }, "x" ] }] })
        )
    ]));

    let builder = Engine::builder().with_secrets_json(&secrets());
    let issues = builder.check_workflow(&workflow);
    assert_eq!(
        triples(&issues),
        vec![
            (
                IssueCode::SecretInMessageWrite,
                "function.input.mappings[1].logic".into(),
                Some("dynamic".into())
            ),
            (
                IssueCode::SecretInMessageWrite,
                "function.input.mappings[1].logic".into(),
                Some("literal".into())
            ),
            (
                IssueCode::SecretInMessageWrite,
                "function.input.mappings[1].logic".into(),
                Some("wrapped".into())
            ),
        ]
    );

    let err = builder.with_workflow(workflow).build().err().unwrap();
    assert!(err.to_string().contains("mappings[1]"), "{err}");
}

#[test]
fn a_secret_in_a_log_expression_is_refused() {
    let workflow = wf(json!([ { "id": "l", "name": "l", "function": {
        "name": "log", "input": {
            "level": "info",
            "message": { "cat": ["key=", { "secret": "partner_key" }] },
            "fields": { "safe": { "var": "data.x" }, "leak": { "secret": "partner.hmac" } } } } } ]));

    let issues = Engine::builder()
        .with_secrets_json(&secrets())
        .check_workflow(&workflow);
    assert_eq!(
        triples(&issues),
        vec![
            (
                IssueCode::SecretInMessageWrite,
                "function.input.fields.leak".into(),
                Some("l".into())
            ),
            (
                IssueCode::SecretInMessageWrite,
                "function.input.message".into(),
                Some("l".into())
            ),
        ]
    );
}

#[test]
fn a_built_engine_checks_workflows_against_its_own_store() {
    let engine = Engine::builder()
        .with_secrets_json(&secrets())
        .build()
        .unwrap();
    let ok = wf(json!([ { "id": "t", "name": "t",
        "condition": { "secret": "partner.hmac" },
        "function": { "name": "map", "input": { "mappings": [] } } } ]));
    assert!(engine.check_workflow(&ok).is_empty());

    let bad = wf(json!([ { "id": "t", "name": "t",
        "condition": { "secret": "partner.nope" },
        "function": { "name": "map", "input": { "mappings": [] } } } ]));
    assert_eq!(
        engine.check_workflow(&bad)[0].code,
        IssueCode::UnknownSecret
    );
}

#[test]
fn issue_codes_have_stable_string_forms() {
    assert_eq!(IssueCode::UnknownSecret.as_str(), "UNKNOWN_SECRET");
    assert_eq!(
        IssueCode::SecretInMessageWrite.as_str(),
        "SECRET_IN_MESSAGE_WRITE"
    );
}

// =============================================================================
// The guarantee
// =============================================================================

#[derive(serde::Deserialize)]
struct SignInput {
    key: dataflow_rs::Template,
}

/// The shape the store exists for: a handler reads the key through a
/// `Template` and writes only a derived, non-secret value.
struct SignHandler;

#[async_trait::async_trait]
impl dataflow_rs::engine::functions::AsyncFunctionHandler for SignHandler {
    type Input = SignInput;

    fn compile_input(
        input: &mut Self::Input,
        c: &dataflow_rs::TemplateCompiler,
    ) -> dataflow_rs::Result<()> {
        input.key.compile(c, "key")
    }

    async fn execute(
        &self,
        ctx: &mut dataflow_rs::TaskContext<'_>,
        input: &Self::Input,
    ) -> dataflow_rs::Result<dataflow_rs::TaskOutcome> {
        let key: String = input.key.eval_into(ctx)?;
        assert_eq!(key, SECRET);
        ctx.set("data.sig_len", dv(json!(key.len())));
        Ok(dataflow_rs::TaskOutcome::Success)
    }
}

#[tokio::test]
async fn a_secret_read_everywhere_it_may_be_read_appears_in_nothing_the_engine_records() {
    let wf = Workflow::from_json(
        &json!({
            "id": "w", "name": "w", "priority": 0,
            "condition": { "==": [ { "secret": "partner_key" }, SECRET ] },
            "tasks": [
                { "id": "seed", "name": "seed", "function": { "name": "map", "input": {
                    "mappings": [ { "path": "data.body", "logic": "payload" } ] } } },
                { "id": "check", "name": "check",
                  "condition": { "!!": { "secret": "partner.hmac" } },
                  "function": { "name": "validation", "input": { "rules": [
                      { "logic": { "==": [ { "secret": "partner_key" }, SECRET ] },
                        "message": "key mismatch" } ] } } },
                { "id": "gate", "name": "gate", "function": { "name": "filter", "input": {
                    "condition": { "!=": [ { "secret": "partner_key" }, "" ] } } } },
                { "id": "sign", "name": "sign", "function": { "name": "sign", "input": {
                    "key": { "secret": "partner_key" } } } },
                { "id": "after", "name": "after", "function": { "name": "map", "input": {
                    "mappings": [ { "path": "data.done", "logic": { "var": "data.sig_len" } } ] } } }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let engine = Engine::builder()
        .with_secrets_json(&secrets())
        .with_workflow(wf)
        .register("sign", SignHandler)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let trace = engine
        .process_message_with_trace_options(&mut message, dataflow_rs::TraceOptions::default())
        .await
        .unwrap();

    // Every task ran and the derived value landed.
    assert!(message.errors().is_empty(), "{:?}", message.errors());
    assert_eq!(trace.executed_count(), 5);
    assert_eq!(message.context["data"]["done"], dv(json!(SECRET.len())));

    // Full snapshots, mapping contexts and the audit trail are all on; none of
    // it can contain the value, because none of it ever held it.
    let recorded = [
        serde_json::to_string(&trace).unwrap(),
        serde_json::to_string(&message).unwrap(),
        format!("{message:?}"),
        format!("{trace:?}"),
    ];
    for text in &recorded {
        assert!(!text.contains(SECRET), "secret value leaked: {text}");
        assert!(
            !text.contains("nested-hmac-value"),
            "secret value leaked: {text}"
        );
    }
    assert!(
        recorded[0].contains("mapping_contexts"),
        "the test must be exercising the mapping-context clones"
    );
}
