//! `Template` — JSONLogic-typed config fields on custom handlers, compiled at
//! `Engine::build()` and evaluated per message. Also the receiver-taking `_with`
//! twins of the two build-time hooks (#56).

use async_trait::async_trait;
use dataflow_rs::engine::functions::AsyncFunctionHandler;
use dataflow_rs::engine::message::Message;
use dataflow_rs::{Engine, Result, TaskContext, TaskOutcome, Template, TemplateCompiler, Workflow};
use serde_json::{Value, json};

mod common;

use common::{LoggingTask, dv, manifest_pair};

// =============================================================================
// Template — JSONLogic-typed config fields for custom handlers
// =============================================================================

#[derive(serde::Deserialize)]
struct GreetingInput {
    greeting: Template,
}

/// The handler: `type Input` holds one `Template` and asserts it was
/// compiled at build time, not per call.
struct GreetingHandler;

#[async_trait]
impl AsyncFunctionHandler for GreetingHandler {
    type Input = GreetingInput;

    fn compile_input(input: &mut Self::Input, c: &TemplateCompiler) -> Result<()> {
        input.greeting.compile(c, "greeting")
    }

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &Self::Input) -> Result<TaskOutcome> {
        // `is_compiled()` true here proves compilation happened at
        // Engine::builder().build() time, not on first call.
        assert!(input.greeting.is_compiled());
        let text: String = input.greeting.eval_into(ctx)?;
        ctx.set("data.greeting", dv(json!(text)));
        Ok(TaskOutcome::Success)
    }
}

#[tokio::test]
async fn a_handlers_template_field_is_compiled_at_build_and_evaluates_through_the_engine() {
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "seed", "name": "seed", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.name", "logic": "world" } ] } } },
            { "id": "greet", "name": "greet", "function": {
                "name": "greeting",
                "input": { "greeting": { "cat": ["hello, ", { "var": "data.name" }] } } } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflow(wf)
        .register("greeting", GreetingHandler)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(
        message.context["data"]["greeting"],
        dv(json!("hello, world"))
    );
}

#[tokio::test]
async fn the_compiled_template_survives_a_second_message_on_the_same_engine() {
    // Guards against the pooled-arena reset invalidating a retained compiled
    // result: the Template is compiled once at build() and must still
    // evaluate correctly on the second call through the same engine.
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "seed", "name": "seed", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.name", "logic": "world" } ] } } },
            { "id": "greet", "name": "greet", "function": {
                "name": "greeting",
                "input": { "greeting": { "cat": ["hi, ", { "var": "data.name" }] } } } }
        ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflow(wf)
        .register("greeting", GreetingHandler)
        .build()
        .unwrap();

    for _ in 0..2 {
        let mut message = Message::from_value(&json!({}));
        engine.process_message(&mut message).await.unwrap();
        assert_eq!(message.context["data"]["greeting"], dv(json!("hi, world")));
    }
}

/// Holds two `Template`s and evaluates both in one `execute` call — two
/// sequential borrows of the thread-local eval arena must not panic.
#[derive(serde::Deserialize)]
struct TwoTemplatesInput {
    first: Template,
    second: Template,
}

struct TwoTemplatesHandler;

#[async_trait]
impl AsyncFunctionHandler for TwoTemplatesHandler {
    type Input = TwoTemplatesInput;

    fn compile_input(input: &mut Self::Input, c: &TemplateCompiler) -> Result<()> {
        input.first.compile(c, "first")?;
        input.second.compile(c, "second")?;
        Ok(())
    }

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &Self::Input) -> Result<TaskOutcome> {
        let a: i64 = input.first.eval_into(ctx)?;
        let b: i64 = input.second.eval_into(ctx)?;
        ctx.set("data.sum", dv(json!(a + b)));
        Ok(TaskOutcome::Success)
    }
}

#[tokio::test]
async fn a_handler_evaluating_two_templates_in_one_call_does_not_panic() {
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [ { "id": "add", "name": "add", "function": {
            "name": "two_templates",
            "input": { "first": 3, "second": 4 } } } ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflow(wf)
        .register("two_templates", TwoTemplatesHandler)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();
    assert_eq!(message.context["data"]["sum"], dv(json!(7)));
}

/// A `Template` nested inside a `Vec<T>`, compiled given a matching
/// `compile_input` that walks the vec.
#[derive(serde::Deserialize)]
struct RuleEntry {
    label: String,
    expr: Template,
}

#[derive(serde::Deserialize)]
struct RuleListInput {
    rules: Vec<RuleEntry>,
}

struct RuleListHandler;

#[async_trait]
impl AsyncFunctionHandler for RuleListHandler {
    type Input = RuleListInput;

    fn compile_input(input: &mut Self::Input, c: &TemplateCompiler) -> Result<()> {
        for entry in &mut input.rules {
            entry.expr.compile(c, &entry.label)?;
        }
        Ok(())
    }

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &Self::Input) -> Result<TaskOutcome> {
        for entry in &input.rules {
            let v: Value = entry.expr.eval_into(ctx)?;
            ctx.set(&format!("data.{}", entry.label), dv(v));
        }
        Ok(TaskOutcome::Success)
    }
}

#[tokio::test]
async fn a_template_nested_inside_a_vec_is_compiled_and_evaluated() {
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [ { "id": "rules", "name": "rules", "function": {
            "name": "rule_list",
            "input": { "rules": [
                { "label": "a", "expr": 1 },
                { "label": "b", "expr": { "cat": ["x", "y"] } }
            ] } } } ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflow(wf)
        .register("rule_list", RuleListHandler)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();
    assert_eq!(message.context["data"]["a"], dv(json!(1)));
    assert_eq!(message.context["data"]["b"], dv(json!("xy")));
}

#[tokio::test]
async fn a_handler_with_no_template_fields_is_unaffected() {
    // The default no-op compile_input: the four pre-existing test handlers
    // (LoggingTask, FailingTask, FivehundredTask, AsyncLoggingTask) already
    // prove this compiles unchanged; this proves it also *runs* unchanged.
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [ { "id": "log", "name": "log", "function": {
            "name": "logger", "input": {} } } ]
    }"#,
    )
    .unwrap();

    let engine = Engine::builder()
        .with_workflow(wf)
        .register("logger", LoggingTask)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();
}

// =============================================================================
// parse_input_with / compile_input_with — one type, several registrations (#56)
// =============================================================================
//
// Default delegation needs no test of its own: every handler above overrides
// only the associated `compile_input`, so each test above already reaches it
// through the defaults of `parse_input_with` and `compile_input_with`. The
// precedence rule — the engine calls only the receiver forms — is pinned at
// the blanket impl in `src/engine/functions/mod.rs`, the one place it lives.
// `check_workflow` parity is in `authoring_validation.rs`.

#[tokio::test]
async fn one_handler_type_registered_twice_compiles_the_field_each_registration_declares() {
    // Same type, same config on both tasks: which field is a template is
    // decided by the registration, which only a receiver-taking hook can see.
    let wf = Workflow::from_json(
        r#"{
        "id": "w", "name": "w", "priority": 0, "condition": true,
        "tasks": [
            { "id": "seed", "name": "seed", "function": {
                "name": "map",
                "input": { "mappings": [ { "path": "data.name", "logic": "world" } ] } } },
            { "id": "first", "name": "first", "function": {
                "name": "manifest_a",
                "input": { "a": { "var": "data.name" }, "b": { "var": "data.name" } } } },
            { "id": "second", "name": "second", "function": {
                "name": "manifest_b",
                "input": { "a": { "var": "data.name" }, "b": { "var": "data.name" } } } }
        ]
    }"#,
    )
    .unwrap();

    let engine = manifest_pair().with_workflow(wf).build().unwrap();

    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    let literal = dv(json!({ "var": "data.name" }));
    assert_eq!(message.context["data"]["first"]["a"], dv(json!("world")));
    assert_eq!(message.context["data"]["first"]["b"], literal);
    assert_eq!(message.context["data"]["second"]["a"], literal);
    assert_eq!(message.context["data"]["second"]["b"], dv(json!("world")));
}
