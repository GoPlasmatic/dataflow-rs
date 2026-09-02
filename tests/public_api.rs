//! The surface an external crate can reach: built-in classification, typed
//! integration configs, crate-root re-exports, connector introspection, and the
//! `TaskContext` eval methods.

use async_trait::async_trait;
use dataflow_rs::engine::functions::AsyncFunctionHandler;
use dataflow_rs::engine::message::Message;
use dataflow_rs::engine::utils::set_nested_value;
use dataflow_rs::{
    BUILTIN_FUNCTION_NAMES, BuiltinKind, Engine, FunctionConfig, Result, Task, TaskContext,
    TaskOutcome, Workflow, builtin_function_kind,
};
use serde_json::{Value, json};

mod common;

use common::{LoggingTask, Shout, dv};

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
// Dispatch vocabulary — `dispatchable_functions` / `can_dispatch`. The half of
// the enrich trap `builtin_function_kind` could not answer: not "does this name
// need a handler" but "and is one registered?".
// =============================================================================

fn sorted_names(engine: &Engine) -> Vec<String> {
    let mut names: Vec<String> = engine
        .dispatchable_functions()
        .map(|f| f.name.to_string())
        .collect();
    names.sort();
    names
}

fn enrich_workflow() -> Workflow {
    Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [{ "id": "t", "name": "t",
                    "function": { "name": "enrich",
                                  "input": { "connector": "c",
                                             "merge_path": "data.out" } } }]
    }"#,
    )
    .unwrap()
}

#[tokio::test]
async fn a_handler_less_enrich_is_not_dispatchable_and_really_does_fail() {
    // The load-bearing test: it ties the API's central claim — "a name
    // can_dispatch rejects fails with FunctionNotFound on the first message" —
    // to observed behaviour, so the claim is checked rather than merely written
    // in a docstring.
    let builder = Engine::builder().with_workflow(enrich_workflow());

    assert!(
        !builder.can_dispatch("enrich"),
        "no handler registered, so the builder predicts failure"
    );

    let engine = builder.build().expect("build stays permissive on purpose");

    assert!(
        !engine.can_dispatch("enrich"),
        "and the built engine agrees"
    );
    assert!(
        !sorted_names(&engine).contains(&"enrich".to_string()),
        "an unrunnable name is absent from the vocabulary"
    );

    // Now show the prediction was right.
    let mut message = Message::from_value(&json!({}));
    let result = engine.process_message(&mut message).await;
    assert!(
        result.is_err(),
        "can_dispatch == false predicted exactly this"
    );
}

#[tokio::test]
async fn registering_the_handler_flips_both_answers() {
    let engine = Engine::builder()
        .with_workflow(enrich_workflow())
        .register("enrich", MockEnrich)
        .build()
        .unwrap();

    assert!(engine.can_dispatch("enrich"));

    let entry = engine
        .dispatchable_functions()
        .find(|f| f.name == "enrich")
        .expect("a backed integration is enumerated");
    assert_eq!(
        entry.kind,
        Some(BuiltinKind::RequiresHandler),
        "it stays a built-in — registration backs it, it does not reclassify it"
    );

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();
}

#[test]
fn self_contained_builtins_dispatch_with_nothing_registered() {
    let builder = Engine::builder();
    for name in ["map", "validate", "parse_json", "filter", "log"] {
        assert!(
            builder.can_dispatch(name),
            "'{name}' is executed by the crate itself"
        );
    }
}

#[test]
fn aliases_dispatch_but_are_reported_under_their_canonical_name() {
    let engine = Engine::builder().build().unwrap();

    assert!(
        engine.can_dispatch("validation"),
        "a task named `validation` really does execute"
    );
    assert!(
        !sorted_names(&engine).contains(&"validation".to_string()),
        "but it is not a separate entry"
    );

    let validate = engine
        .dispatchable_functions()
        .find(|f| f.name == "validate")
        .expect("the canonical spelling is the entry");
    assert_eq!(validate.aliases, &["validation"]);
}

#[test]
fn registering_over_a_self_contained_name_is_inert() {
    // `map` deserializes to FunctionConfig::Map, which the crate executes
    // without consulting the registry — so this registration never runs, and
    // the vocabulary is unchanged.
    let bare = Engine::builder().build().unwrap();
    let shadowed = Engine::builder()
        .register("map", LoggingTask)
        .build()
        .unwrap();

    assert_eq!(sorted_names(&bare), sorted_names(&shadowed));
    assert_eq!(
        shadowed
            .dispatchable_functions()
            .filter(|f| f.name == "map")
            .count(),
        1,
        "`map` is reported once, not once per source"
    );
}

#[test]
fn the_builder_and_the_engine_it_builds_agree() {
    let builder = Engine::builder()
        .register("enrich", MockEnrich)
        .register("shout", LoggingTask);

    let mut before: Vec<String> = builder
        .dispatchable_functions()
        .map(|f| f.name.to_string())
        .collect();
    before.sort();

    let engine = builder.build().unwrap();
    assert_eq!(before, sorted_names(&engine));
}

#[tokio::test]
async fn the_vocabulary_survives_a_hot_reload() {
    let engine = Engine::builder()
        .register("enrich", MockEnrich)
        .register("shout", LoggingTask)
        .build()
        .unwrap();

    let before = sorted_names(&engine);
    let reloaded = engine.with_new_workflows(vec![enrich_workflow()]).unwrap();

    assert_eq!(
        before,
        sorted_names(&reloaded),
        "with_new_workflows reuses the handler registry, so the vocabulary is stable"
    );
    assert!(reloaded.can_dispatch("shout"));
}

#[test]
fn every_builtin_name_is_accounted_for_exactly_once() {
    // Acceptance criterion: the enumeration covers BUILTIN_FUNCTION_NAMES
    // exactly once, aliases grouped, RequiresHandler names present iff backed.
    let engine = Engine::builder()
        .register("http_call", SpyHttpCall)
        .build()
        .unwrap();

    let entries: Vec<_> = engine.dispatchable_functions().collect();

    for name in BUILTIN_FUNCTION_NAMES {
        let as_canonical = entries.iter().filter(|f| f.name == *name).count();
        let as_alias = entries.iter().filter(|f| f.aliases.contains(name)).count();

        let backed = matches!(
            builtin_function_kind(name),
            Some(BuiltinKind::SelfContained)
        ) || engine.can_dispatch(name);

        if backed {
            assert_eq!(
                as_canonical + as_alias,
                1,
                "'{name}' must appear exactly once, as an entry or an alias \
                 (entry={as_canonical}, alias={as_alias})"
            );
        } else {
            assert_eq!(
                as_canonical + as_alias,
                0,
                "'{name}' has no handler, so it must not appear at all"
            );
        }
    }

    // The two config-only integrations left unregistered are absent; the
    // registered one is present.
    let names = sorted_names(&engine);
    assert!(names.contains(&"http_call".to_string()));
    assert!(!names.contains(&"enrich".to_string()));
    assert!(!names.contains(&"publish_kafka".to_string()));
}

#[test]
fn an_unregistered_custom_name_is_absent_from_the_vocabulary() {
    let engine = Engine::builder().build().unwrap();
    assert!(!engine.can_dispatch("shout"));
    assert!(!sorted_names(&engine).contains(&"shout".to_string()));

    let engine = Engine::builder()
        .register("shout", LoggingTask)
        .build()
        .unwrap();
    assert!(engine.can_dispatch("shout"));
    let entry = engine
        .dispatchable_functions()
        .find(|f| f.name == "shout")
        .unwrap();
    assert_eq!(entry.kind, None, "custom handlers report kind: None");
    assert!(entry.aliases.is_empty());
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
        // Every parameter is JSONLogic since 3.9, so a handler reads it
        // through the config's resolver rather than off the field.
        let observed = input.resolve_response_path(ctx)?.unwrap_or_default();
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
    let located: Vec<(&str, &str, Option<&str>)> = refs
        .iter()
        .map(|r| (r.workflow_id, r.task_id, r.connector.as_static()))
        .collect();
    assert_eq!(
        located,
        vec![
            ("wf_a", "call", Some("user_service")),
            ("wf_b", "db", Some("pg_main")),
        ]
    );

    // The rename guard itself.
    assert!(
        refs.iter()
            .any(|r| r.connector.as_static() == Some("pg_main"))
    );
    assert!(
        !refs
            .iter()
            .any(|r| r.connector.as_static() == Some("retired_connector"))
    );
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

// =============================================================================
// Custom JSONLogic operators — EngineBuilder::with_datalogic_operator
// =============================================================================

/// One workflow whose `map` logic calls the custom operator.
fn shout_workflow() -> Workflow {
    Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [{ "id": "t", "name": "t",
                    "function": { "name": "map", "input": { "mappings": [
                        { "path": "data.loud",
                          "logic": { "shout": [{ "var": "data.word" }] } }
                    ] } } }]
    }"#,
    )
    .expect("workflow json")
}

#[tokio::test]
async fn custom_datalogic_operator_is_live_through_the_full_engine_path() {
    let engine = Engine::builder()
        .with_workflow(shout_workflow())
        .with_datalogic_operator("shout", Shout)
        .build()
        .expect("engine should build");

    let mut message = Message::from_value(&json!({}));
    set_nested_value(&mut message.context, "data.word", dv(json!("quiet")));
    engine.process_message(&mut message).await.expect("process");
    assert_eq!(message.context["data"]["loud"], dv(json!("QUIET")));
}

#[tokio::test]
async fn custom_datalogic_operator_survives_hot_reload() {
    // `with_new_workflows` builds a fresh datalogic engine; the registration
    // must be retained and re-applied or the operator silently vanishes on
    // the first reload — the exact regression this test exists to catch.
    let engine = Engine::builder()
        .with_workflow(shout_workflow())
        .with_datalogic_operator("shout", Shout)
        .build()
        .expect("engine should build");

    let reloaded = engine
        .with_new_workflows(vec![shout_workflow()])
        .expect("hot reload should recompile the operator call");

    let mut message = Message::from_value(&json!({}));
    set_nested_value(&mut message.context, "data.word", dv(json!("still quiet")));
    reloaded
        .process_message(&mut message)
        .await
        .expect("process");
    assert_eq!(message.context["data"]["loud"], dv(json!("STILL QUIET")));
}

#[tokio::test]
async fn unregistered_operator_name_stays_inert_template_data() {
    // Templating mode: an object keyed by an unknown name is not an error —
    // it echoes as literal data. Registering a name is what turns it live,
    // so this pins the OFF state of the same vocabulary the two tests above
    // pin ON.
    let engine = Engine::builder()
        .with_workflow(shout_workflow())
        .build()
        .expect("engine should build without the operator");

    let mut message = Message::from_value(&json!({}));
    set_nested_value(&mut message.context, "data.word", dv(json!("quiet")));
    engine.process_message(&mut message).await.expect("process");
    assert_eq!(
        message.context["data"]["loud"],
        dv(json!({ "shout": ["quiet"] }))
    );
}

// =============================================================================
// #[non_exhaustive] on Task / TaskGroup / Workflow — this file is a separate
// crate, so the attribute is in force here exactly as it is for a downstream
// user. These tests are the migration path, executed.
// =============================================================================

#[test]
fn a_task_is_built_through_its_constructor_and_then_assigned() {
    let mut task = Task::action(
        "charge",
        "Charge card",
        FunctionConfig::Custom {
            name: "billing".to_string(),
            input: json!({}),
            compiled_input: None,
        },
    );

    // Every field a caller has business setting is still writable.
    task.description = Some("Takes the payment".to_string());
    task.condition = json!({"var": "data.ready"});
    task.continue_on_error = true;
    task.terminal = true;
    task.halt_on = dataflow_rs::HaltOn::Failure;

    // And readable.
    assert_eq!(task.id, "charge");
    assert_eq!(task.name, "Charge card");
    assert!(task.terminal);
    assert_eq!(task.halt_on, dataflow_rs::HaltOn::Failure);
    assert_eq!(task.function.function_name(), "billing");

    // The engine internals a struct literal used to force callers to name are
    // set correctly without being mentioned.
    assert_eq!(
        &*task.id_arc, "charge",
        "Task::action keeps the Arc mirror in step with `id`"
    );
}

#[tokio::test]
async fn a_constructor_built_workflow_runs_the_same_as_a_parsed_one() {
    // The two construction paths must agree, or the constructor route would be
    // a second-class citizen after making struct literals unavailable.
    let mut task = Task::action(
        "greet",
        "Greet",
        FunctionConfig::Custom {
            name: "shout".to_string(),
            input: json!({}),
            compiled_input: None,
        },
    );
    task.continue_on_error = true;

    let mut built = Workflow::new();
    built.id = "w".to_string();
    built.name = "w".to_string();
    built.tasks = vec![task];

    let parsed = Workflow::from_json(
        r#"{"id": "w", "name": "w", "priority": 0,
            "tasks": [{"id": "greet", "name": "Greet", "continue_on_error": true,
                       "function": {"name": "shout", "input": {}}}]}"#,
    )
    .unwrap();

    assert_eq!(built.id, parsed.id);
    assert_eq!(built.tasks.len(), parsed.tasks.len());
    assert_eq!(built.tasks[0].id, parsed.tasks[0].id);
    assert_eq!(
        built.tasks[0].continue_on_error,
        parsed.tasks[0].continue_on_error
    );

    // And the constructor-built one actually executes.
    let engine = Engine::builder()
        .with_workflow(built)
        .register("shout", LoggingTask)
        .build()
        .expect("a constructor-built workflow builds");

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();
}

#[test]
fn task_groups_are_readable_from_a_parsed_workflow() {
    // `TaskGroup` is never built by hand — `end` indexes the flattened task
    // list and means nothing on its own — but a consumer inspecting a parsed
    // workflow must still be able to read one.
    let workflow = Workflow::from_json(
        r#"{"id": "w", "name": "w", "priority": 0,
            "tasks": [{"id": "guard", "condition": true, "terminal": true,
                       "tasks": [{"id": "inner", "name": "inner",
                                  "function": {"name": "map",
                                               "input": {"mappings": []}}}]}]}"#,
    )
    .unwrap();

    let group = workflow.tasks[0]
        .group_starts
        .first()
        .expect("the group opens at the task it encloses");

    assert_eq!(group.id, "guard");
    assert!(group.terminal);
}
