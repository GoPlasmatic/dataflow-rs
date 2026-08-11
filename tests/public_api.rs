//! The surface an external crate can reach: built-in classification, typed
//! integration configs, crate-root re-exports, connector introspection, and the
//! `TaskContext` eval methods.

use async_trait::async_trait;
use dataflow_rs::engine::functions::AsyncFunctionHandler;
use dataflow_rs::engine::message::Message;
use dataflow_rs::engine::utils::set_nested_value;
use dataflow_rs::{
    BUILTIN_FUNCTION_NAMES, BuiltinKind, Engine, Result, TaskContext, TaskOutcome, Workflow,
    builtin_function_kind,
};
use serde_json::{Value, json};

mod common;

use common::{LoggingTask, dv};

// =============================================================================
// Built-in function classification — regression coverage for the
// has_function / config-only-integration conflation
// =============================================================================

#[tokio::test]
async fn handler_less_enrich_builds_but_is_detectable_before_processing() {
    // `enrich` ships as a config schema with no implementation. It deserializes
    // to `FunctionConfig::Enrich` rather than `Custom`, so `precompile_custom_inputs`
    // never visits it and `Engine::new` accepts the workflow — then every message
    // fails with FunctionNotFound.
    //
    // `Engine::new` staying permissive is deliberate (a host screening stored
    // definitions one row at a time must not be stopped from booting by one
    // unusable row). What was missing was any way to *detect* the gap first.
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [{ "id": "t", "name": "t",
                    "function": { "name": "enrich",
                                  "input": { "connector": "c",
                                             "merge_path": "data.out" } } }]
    }"#,
    )
    .expect("a handler-less enrich task still parses");

    // Unchanged: the engine builds.
    let engine = Engine::new(vec![wf], std::collections::HashMap::new())
        .expect("Engine::new stays permissive for config-only integrations");

    // The gap is now detectable without processing a message.
    assert_eq!(
        builtin_function_kind("enrich"),
        Some(BuiltinKind::RequiresHandler),
        "a caller can screen for this before accepting the workflow"
    );

    // And the underlying behaviour it predicts is real.
    let mut message = Message::from_value(&json!({}));
    let result = engine.process_message(&mut message).await;
    assert!(
        result.is_err(),
        "a handler-less enrich still fails on the first message"
    );
    let codes: Vec<&str> = message.errors().iter().map(|e| e.code.as_str()).collect();
    assert!(
        !codes.is_empty(),
        "the failure is also recorded on the message, got {codes:?}"
    );
}

#[tokio::test]
async fn a_registered_enrich_handler_closes_the_gap() {
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [{ "id": "t", "name": "t",
                    "function": { "name": "enrich",
                                  "input": { "connector": "c",
                                             "merge_path": "data.out" } } }]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflow(wf)
        .register("enrich", MockEnrich)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine
        .process_message(&mut message)
        .await
        .expect("with a handler registered the same workflow runs");
}

// A handler for a config-only integration must declare the *typed* input its
// `FunctionConfig` variant carries — an `enrich` task deserializes to
// `FunctionConfig::Enrich { input: EnrichConfig }`, so a handler declaring
// `type Input = Value` fails the downcast at dispatch with "Handler input type
// mismatch" rather than running.
struct MockEnrich;

#[async_trait]
impl AsyncFunctionHandler for MockEnrich {
    type Input = dataflow_rs::EnrichConfig;

    async fn execute(
        &self,
        _ctx: &mut TaskContext<'_>,
        _input: &Self::Input,
    ) -> Result<TaskOutcome> {
        Ok(TaskOutcome::Success)
    }
}

#[test]
fn builtin_classification_is_reachable_from_the_crate_root() {
    // Pins the re-export path a consumer actually uses.
    assert!(BUILTIN_FUNCTION_NAMES.contains(&"enrich"));
    assert!(dataflow_rs::is_builtin_function("map"));
    assert_eq!(
        builtin_function_kind("map"),
        Some(BuiltinKind::SelfContained)
    );
    assert_eq!(builtin_function_kind("my_custom_handler"), None);
}

// =============================================================================
// http_call destination field — regression coverage for the silent discard
// =============================================================================

// Records what the engine actually handed the handler, by writing the observed
// `response_path` into the message context. `type Input` is pinned to
// `HttpCallConfig` because `http_call` deserializes to a typed built-in variant.
struct SpyHttpCall;

#[async_trait]
impl AsyncFunctionHandler for SpyHttpCall {
    type Input = dataflow_rs::HttpCallConfig;

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &Self::Input) -> Result<TaskOutcome> {
        let observed = input.response_path.clone().unwrap_or_default();
        ctx.set("data.observed_response_path", dv(json!(observed)));
        ctx.set("data.observed_method", dv(json!(input.method.as_str())));
        Ok(TaskOutcome::Success)
    }
}

#[tokio::test]
async fn http_call_output_alias_survives_the_full_engine_path() {
    // Proves the alias holds through Workflow::from_json -> LogicCompiler ->
    // Engine::builder().build() -> dispatch, not just a bare from_value.
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [{ "id": "call", "name": "call",
                    "function": { "name": "http_call",
                                  "input": { "connector": "user_service",
                                             "method": "POST",
                                             "output": "data.user_profile" } } }]
    }"#,
    )
    .expect("a task spelling the field `output` should parse");

    let engine = Engine::builder()
        .with_workflow(wf)
        .register("http_call", SpyHttpCall)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(
        message.context["data"]["observed_response_path"],
        dv(json!("data.user_profile")),
        "the handler must see the aliased destination, not an empty default"
    );
    assert_eq!(
        message.context["data"]["observed_method"],
        dv(json!("POST")),
        "as_str() must give the canonical token the config was written with"
    );
}

#[test]
fn http_call_misspelled_destination_is_rejected_at_workflow_parse_time() {
    // The defect: this previously parsed cleanly, and the task would make its
    // request and throw the response away with no error anywhere.
    let err = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [{ "id": "call", "name": "call",
                    "function": { "name": "http_call",
                                  "input": { "connector": "c",
                                             "outputs": "data.user_profile" } } }]
    }"#,
    )
    .expect_err("a misspelled destination field must fail the workflow parse");

    let msg = err.to_string();
    assert!(
        msg.contains("unknown field") && msg.contains("outputs"),
        "the error should name the offending field, got: {msg}"
    );
}

#[test]
fn http_method_is_reachable_from_the_crate_root() {
    use dataflow_rs::HttpMethod;

    assert_eq!(HttpMethod::default(), HttpMethod::Get);
    assert_eq!(HttpMethod::Delete.as_str(), "DELETE");
    assert!(HttpMethod::Put.is_idempotent());
    assert!(!HttpMethod::Post.is_idempotent());
    assert_eq!(HttpMethod::ALL.len(), 5);
}

// =============================================================================
// Crate-root re-exports of datalogic_rs / datavalue
// =============================================================================
//
// Compile-only. Underscore-prefixed so `-D warnings` does not flag them as dead
// code; they exist to fail the build if a path stops resolving or the two
// reachable spellings of the value type ever become different types.

/// Both paths to the value type are one type — catches an accidental
/// double-link of incompatible `datavalue` majors.
fn _datavalue_reexport_paths_agree(
    v: dataflow_rs::datavalue::OwnedDataValue,
) -> dataflow_rs::datalogic_rs::datavalue::OwnedDataValue {
    v
}

/// The re-exported engine type is the one the accessor returns.
fn _datalogic_engine_type_is_reachable(
    e: &dataflow_rs::Engine,
) -> &std::sync::Arc<dataflow_rs::datalogic_rs::Engine> {
    e.datalogic()
}

/// `Logic` is nameable through the re-export — the type handler authors need
/// for `HttpCallConfig::compiled_path_logic` and friends.
fn _datalogic_logic_is_nameable(
    l: &Option<std::sync::Arc<dataflow_rs::datalogic_rs::Logic>>,
) -> bool {
    l.is_some()
}

#[test]
fn reexports_do_not_shadow_the_crate_root_names() {
    // `datalogic_rs` / `datavalue` must not collide with `engine` / `prelude`
    // or the Rule/Action/RulesEngine aliases.
    let _: dataflow_rs::Rule = Workflow::new();
    let _engine_mod_still_reachable: fn(&str) -> Option<&dataflow_rs::datavalue::OwnedDataValue> =
        |_| None;
    assert!(dataflow_rs::is_builtin_function("map"));
}

// =============================================================================
// Connector introspection across a built engine
// =============================================================================

#[tokio::test]
async fn connector_refs_across_a_built_engine() {
    // The rename-guard shape: "is any workflow still pointing at this
    // connector?" — answered without an engine-level wrapper, via
    // `workflows()` + flat_map.
    let wf_a = Workflow::from_json(
        r#"{
        "id": "wf_a", "name": "wf_a", "priority": 0, "condition": true,
        "tasks": [
            { "id": "m", "name": "m", "function": {
                "name": "map", "input": { "mappings": [] } } },
            { "id": "call", "name": "call", "function": {
                "name": "http_call", "input": { "connector": "user_service" } } }
        ]
    }"#,
    )
    .unwrap();
    let wf_b = Workflow::from_json(
        r#"{
        "id": "wf_b", "name": "wf_b", "priority": 1, "condition": true,
        "tasks": [
            { "id": "db", "name": "db", "function": {
                "name": "pg_query",
                "input": { "connector": "pg_main", "database": "orders" } } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflows(vec![wf_a, wf_b])
        .register("pg_query", LoggingTask)
        .build()
        .unwrap();

    let refs: Vec<_> = engine
        .workflows()
        .iter()
        .flat_map(Workflow::connector_refs)
        .collect();

    assert_eq!(refs.len(), 2, "one per connector-bearing task");

    // Workflow provenance survives, so a diagnostic can name where a reference
    // lives — and the Custom convention is picked up alongside the typed field.
    let located: Vec<(&str, &str, &str)> = refs
        .iter()
        .map(|r| (r.workflow_id, r.task_id, r.connector))
        .collect();
    assert_eq!(
        located,
        vec![("wf_a", "call", "user_service"), ("wf_b", "db", "pg_main"),]
    );

    // The rename guard itself.
    assert!(refs.iter().any(|r| r.connector == "pg_main"));
    assert!(!refs.iter().any(|r| r.connector == "retired_connector"));
}

#[test]
fn remove_nested_value_is_public_from_the_utils_module() {
    // Reachable from an external crate with no other change.
    use dataflow_rs::engine::utils::remove_nested_value;

    let mut ctx = dv(json!({"data": {"keep": 1, "scratch": {"x": 2}}}));

    assert_eq!(
        remove_nested_value(&mut ctx, "data.scratch"),
        Some(dv(json!({"x": 2})))
    );
    assert_eq!(
        serde_json::Value::from(&ctx),
        json!({"data": {"keep": 1}}),
        "unlike set_nested_value(path, Null), the key is gone rather than nulled"
    );
}

// =============================================================================
// TaskContext eval surface and integration-config resolve_*
// =============================================================================

/// Reads its config through the sanctioned `resolve_*` methods rather than
/// touching the `compiled_*` slots, and records what it saw.
struct ResolvingHttpCall;

#[async_trait]
impl AsyncFunctionHandler for ResolvingHttpCall {
    type Input = dataflow_rs::HttpCallConfig;

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &Self::Input) -> Result<TaskOutcome> {
        let path = input.resolve_path(ctx)?.unwrap_or_default();
        let body = input.resolve_body(ctx)?.unwrap_or(json!(null));
        let method = input.method.as_str().to_string();

        ctx.set("data.seen_path", dv(json!(path)));
        ctx.set("data.seen_body", dv(body));
        ctx.set("data.seen_method", dv(json!(method)));
        Ok(TaskOutcome::Success)
    }
}

#[tokio::test]
async fn resolve_methods_work_through_the_full_engine_path() {
    // Proves the compiled slots are populated by LogicCompiler and read through
    // resolve_*, end to end: from_json -> compile -> build -> dispatch.
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "seed", "name": "seed", "function": {
                "name": "map",
                "input": { "mappings": [
                    { "path": "data.user_id", "logic": "u-42" },
                    { "path": "data.amount", "logic": 100 } ] } } },
            { "id": "call", "name": "call", "function": {
                "name": "http_call",
                "input": {
                    "connector": "user_service",
                    "method": "POST",
                    "path_logic": { "cat": ["/users/", { "var": "data.user_id" }] },
                    "body_logic": { "var": "data.amount" }
                } } }
        ]
    }"#,
    )
    .expect("workflow with path_logic/body_logic should parse");

    let engine = Engine::builder()
        .with_workflow(wf)
        .register("http_call", ResolvingHttpCall)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(
        message.context["data"]["seen_path"],
        dv(json!("/users/u-42")),
        "path_logic must be compiled by the engine and resolved to a plain string"
    );
    assert_eq!(message.context["data"]["seen_body"], dv(json!(100)));
    assert_eq!(message.context["data"]["seen_method"], dv(json!("POST")));
}

#[tokio::test]
async fn task_context_eval_surface_is_reachable_from_outside_the_crate() {
    // Built the way test_async_task_execution does, so this proves the methods
    // need no imports beyond datalogic_rs::Logic — reached here through the
    // crate-root re-export added in #26.
    use dataflow_rs::datalogic_rs::Logic;

    struct EvalProbe;

    #[async_trait]
    impl AsyncFunctionHandler for EvalProbe {
        type Input = Value;
        async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
            // The whole-context accessor lines up with the three slots.
            assert_eq!(&ctx.context()["data"], ctx.data());
            assert_eq!(&ctx.context()["metadata"], ctx.metadata());
            assert_eq!(&ctx.context()["temp_data"], ctx.temp_data());

            let logic: std::sync::Arc<Logic> = ctx
                .datalogic()
                .compile_arc(&json!({"var": "data.x"}))
                .unwrap();

            assert_eq!(ctx.eval_json(&logic)?, json!("dx"));
            assert_eq!(ctx.eval_to_plain_string(&logic)?, "dx");
            assert_eq!(ctx.eval(&logic)?, dv(json!("dx")));
            Ok(TaskOutcome::Success)
        }
    }

    let mut message = Message::from_value(&json!({}));
    set_nested_value(&mut message.context, "data.x", dv(json!("dx")));
    let datalogic = std::sync::Arc::new(
        dataflow_rs::datalogic_rs::Engine::builder()
            .with_templating(true)
            .build(),
    );

    let mut ctx = TaskContext::new(&mut message, &datalogic);
    let outcome = EvalProbe
        .execute(&mut ctx, &json!({}))
        .await
        .expect("eval surface should be reachable and correct");
    assert_eq!(outcome, TaskOutcome::Success);
}
