//! Fixtures shared by the integration test binaries.
//!
//! Every file directly under `tests/` compiles as its own crate, so anything
//! more than one of them needs lives here and is pulled in with `mod common;`.
//! No single binary uses all of it — hence the module-wide dead-code allowance.

#![allow(dead_code)]

use async_trait::async_trait;
use dataflow_rs::datalogic_rs::bumpalo::Bump;
use dataflow_rs::engine::functions::AsyncFunctionHandler;
use dataflow_rs::{
    DataflowError, Engine, EngineBuilder, Result, TaskContext, TaskOutcome, Template,
    TemplateCompiler, Workflow,
};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Bridge helper for tests: build an `OwnedDataValue` from a `json!` literal.
pub fn dv(v: serde_json::Value) -> OwnedDataValue {
    OwnedDataValue::from(&v)
}

/// A `Workflow` from a `json!` literal.
pub fn workflow(definition: Value) -> Workflow {
    Workflow::from_json(&definition.to_string()).unwrap()
}

/// Evaluate `expr` against `data` on the engine's own datalogic instance —
/// the same path a handler calling `ctx.datalogic()` takes.
pub fn eval(engine: &Engine, expr: Value, data: Value) -> std::result::Result<Value, String> {
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

/// Uppercases its first argument — a stand-in custom operator.
pub struct Shout;

impl dataflow_rs::datalogic_rs::CustomOperator for Shout {
    fn evaluate<'a>(
        &self,
        args: &[&'a dataflow_rs::datalogic_rs::DataValue<'a>],
        _ctx: &mut dataflow_rs::datalogic_rs::operator::EvalContext<'_, 'a>,
        arena: &'a dataflow_rs::datalogic_rs::bumpalo::Bump,
    ) -> dataflow_rs::datalogic_rs::Result<&'a dataflow_rs::datalogic_rs::DataValue<'a>> {
        use dataflow_rs::datalogic_rs::ArenaExt;
        let s = args.first().and_then(|v| v.as_str()).unwrap_or_default();
        Ok(arena.string(&s.to_uppercase()))
    }
}

// -----------------------------------------------------------------------------
// Secrets — shared by `secrets.rs` and `secrets_isolation.rs`
// -----------------------------------------------------------------------------

/// Distinctive enough that a substring search is a real test.
pub const SECRET: &str = "s3cr3t-value-7f2a";
pub const NESTED: &str = "nested-hmac-91c0";

/// The store the secrets tests share: one top-level key and one nested.
pub fn secrets() -> Value {
    json!({
        "partner_key": SECRET,
        "partner": { "hmac": NESTED }
    })
}

#[derive(serde::Deserialize)]
pub struct SignInput {
    pub key: Template,
    pub body: Template,
}

/// The shape the store exists for: a handler reads the key through a
/// `Template` — in every projection the API offers — and writes only a
/// derived value, `data.sig = "<body length>:<key length>"`.
///
/// **Precondition:** the engine's store is [`secrets()`] and the task's `key`
/// resolves to [`SECRET`] — the projections are checked against that constant,
/// so registering `Sign` against any other store panics from inside the task
/// executor rather than failing the caller's own assertion.
pub struct Sign;

#[async_trait]
impl AsyncFunctionHandler for Sign {
    type Input = SignInput;

    fn compile_input(input: &mut Self::Input, c: &TemplateCompiler) -> Result<()> {
        input.key.compile(c, "key")?;
        input.body.compile(c, "body")
    }

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &Self::Input) -> Result<TaskOutcome> {
        let key: String = input.key.eval_into(ctx)?;
        assert_eq!(key, SECRET, "Template::eval_into");
        assert_eq!(input.key.eval(ctx)?, dv(json!(SECRET)), "Template::eval");
        assert_eq!(
            input.key.eval_to_plain_string(ctx)?,
            SECRET,
            "Template::eval_to_plain_string"
        );
        let body: String = input.body.eval_into(ctx)?;
        ctx.set(
            "data.sig",
            dv(json!(format!("{}:{}", body.len(), key.len()))),
        );
        Ok(TaskOutcome::Success)
    }
}

// -----------------------------------------------------------------------------
// One handler type, several registrations — shared by `templates.rs` and
// `authoring_validation.rs` (#56)
// -----------------------------------------------------------------------------

/// One handler *type* registered under several names, each instance carrying
/// which config key its manifest declared as a template — the plugin-host
/// shape from #56. Both build-time hooks decide on instance state, which is
/// what the receiver-taking `_with` forms exist for: `parse_input_with`
/// requires the declared key, so a config without it is a shape mismatch for
/// this registration; `compile_input_with` compiles only that key.
///
/// `execute` writes every key under `data.<task id>`: the declared one
/// evaluated, the others echoed verbatim — which is what lets a test see that
/// the *other* registration's key was left alone.
pub struct ManifestHandler {
    pub template_field: &'static str,
}

#[async_trait]
impl AsyncFunctionHandler for ManifestHandler {
    type Input = BTreeMap<String, Template>;

    fn parse_input_with(&self, input: &Value) -> Result<Self::Input> {
        let parsed = Self::parse_input(input)?;
        if !parsed.contains_key(self.template_field) {
            return Err(DataflowError::Validation(format!(
                "manifest declares `{}` as a template but the config has no such key",
                self.template_field
            )));
        }
        Ok(parsed)
    }

    fn compile_input_with(&self, input: &mut Self::Input, c: &TemplateCompiler) -> Result<()> {
        if let Some(field) = input.get_mut(self.template_field) {
            field.compile(c, self.template_field)?;
        }
        Ok(())
    }

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &Self::Input) -> Result<TaskOutcome> {
        let task = ctx.task_id().expect("set by the engine").to_string();
        for (key, field) in input {
            let path = format!("data.{task}.{key}");
            if field.is_compiled() {
                let value = field.eval(ctx)?;
                ctx.set(&path, value);
            } else {
                ctx.set_json(&path, field.as_json());
            }
        }
        Ok(TaskOutcome::Success)
    }
}

/// Both registrations of the #56 shape on one builder: `manifest_a` declares
/// `a` as its template, `manifest_b` declares `b`.
pub fn manifest_pair() -> EngineBuilder {
    Engine::builder()
        .register(
            "manifest_a",
            ManifestHandler {
                template_field: "a",
            },
        )
        .register(
            "manifest_b",
            ManifestHandler {
                template_field: "b",
            },
        )
}

// A simple async task implementation
#[derive(Debug)]
pub struct LoggingTask;

#[async_trait]
impl AsyncFunctionHandler for LoggingTask {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        println!("Executed task for message: {}", ctx.message().id());
        Ok(TaskOutcome::Success)
    }
}

// Handler that always returns Err — used by the single-error-channel
// regression tests.
pub struct FailingTask;

#[async_trait]
impl AsyncFunctionHandler for FailingTask {
    type Input = Value;

    async fn execute(&self, _ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        Err(dataflow_rs::DataflowError::Task("boom".to_string()))
    }
}

// Handler that returns a 500 status — used by the single-error-channel
// regression tests.
pub struct FivehundredTask;

#[async_trait]
impl AsyncFunctionHandler for FivehundredTask {
    type Input = Value;

    async fn execute(&self, _ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        Ok(TaskOutcome::Status(500))
    }
}

// Handler that fails with an engine-owned variant carrying its own
// classification — used by the error-code tests, which pin that the live path
// records `TIMEOUT_ERROR` rather than collapsing every variant to `TASK_ERROR`.
pub struct TimingOutTask;

#[async_trait]
impl AsyncFunctionHandler for TimingOutTask {
    type Input = Value;

    async fn execute(&self, _ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        Err(dataflow_rs::DataflowError::Timeout(
            "provider timed out".to_string(),
        ))
    }
}

// Handler that records an error and still succeeds. Reaches neither failure arm
// of `handle_task_result`, so it pins that the error-context path follows
// `message.errors()` rather than the failure arms.
pub struct AddErrorTask;

#[async_trait]
impl AsyncFunctionHandler for AddErrorTask {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        ctx.add_error(dataflow_rs::ErrorInfo::builder("CUSTOM_CODE", "handler-recorded").build());
        Ok(TaskOutcome::Success)
    }
}

// An async task implementation
pub struct AsyncLoggingTask;

#[async_trait]
impl AsyncFunctionHandler for AsyncLoggingTask {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        println!("Executed async task for message: {}", ctx.message().id());
        // Simulate async work
        tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
        Ok(TaskOutcome::Success)
    }
}

/// N map tasks, each writing one key, as one workflow.
pub fn n_map_task_workflow(n: usize) -> Workflow {
    let tasks: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{ "id": "t{i}", "name": "t{i}", "function": {{
                       "name": "map",
                       "input": {{ "mappings": [ {{ "path": "data.k{i}", "logic": {i} }} ] }} }} }}"#
            )
        })
        .collect();
    Workflow::from_json(&format!(
        r#"{{ "id": "w", "name": "w", "priority": 0, "condition": true,
              "tasks": [{}] }}"#,
        tasks.join(",")
    ))
    .unwrap()
}

/// One recorded observer callback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeenEvent {
    pub workflow_id: String,
    pub task_id: String,
    pub function: String,
    pub status: Option<u16>,
}

/// Records every event it is handed. `Mutex` is fine for a test; a real observer
/// must not block, per the trait contract.
#[derive(Default)]
pub struct RecordingObserver {
    events: std::sync::Mutex<Vec<SeenEvent>>,
}

impl RecordingObserver {
    pub fn seen(&self) -> Vec<SeenEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl dataflow_rs::ExecutionObserver for RecordingObserver {
    fn task_finished(&self, event: &dataflow_rs::TaskEvent<'_>) {
        self.events.lock().unwrap().push(SeenEvent {
            workflow_id: event.workflow_id.to_string(),
            task_id: event.task_id.to_string(),
            function: event.function.to_string(),
            status: event.status,
        });
    }
}
