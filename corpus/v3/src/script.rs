//! The corpus's one host function. Everything it does is in its config, so a
//! case can say what a handler does without code, and v4 has exactly one
//! function to port to replay the corpus.
//!
//! In order, `execute`:
//! 1. yields `yields` times (a template, so a fan-out element can choose);
//! 2. writes its identity to `identity`;
//! 3. applies `set`, each write through `ctx.set`;
//! 4. pushes `errors` through `ctx.add_error`;
//! 5. evaluates `outcome` and returns it.
//!
//! The evaluated `outcome` is `"success"`, `"skip"`, `"halt"`, `{"status": n}`
//! or `{"error": {"variant", "code"?, "message"?, "status"?, "retryable"?}}`.
//! Absent or `null` is `Success`.

use async_trait::async_trait;
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::{
    AsyncFunctionHandler, DataflowError, ErrorInfo, Result, TaskContext, TaskOutcome, Template,
    TemplateCompiler,
};
use serde::Deserialize;
use serde_json::{Value, json};

pub struct Script;

/// Unknown keys are accepted, as a v3 host's serde default does: break 2a's
/// case depends on it.
#[derive(Deserialize)]
pub struct ScriptInput {
    #[serde(default)]
    pub set: Vec<Write>,
    #[serde(default)]
    pub errors: Vec<AddError>,
    #[serde(default)]
    pub identity: Option<String>,
    #[serde(default)]
    pub yields: Option<Template>,
    #[serde(default)]
    pub outcome: Option<Template>,
}

/// Exactly one of `value` (a template, evaluated), `get` (a path read through
/// `ctx.get`, `null` when absent) or `raw` (the config value itself, never
/// evaluated).
#[derive(Deserialize)]
pub struct Write {
    pub path: String,
    #[serde(default)]
    pub value: Option<Template>,
    #[serde(default)]
    pub get: Option<String>,
    #[serde(default)]
    pub raw: Option<Value>,
}

#[derive(Deserialize)]
pub struct AddError {
    pub code: String,
    pub message: String,
}

#[async_trait]
impl AsyncFunctionHandler for Script {
    type Input = ScriptInput;

    fn compile_input(input: &mut Self::Input, c: &TemplateCompiler) -> Result<()> {
        for (i, write) in input.set.iter_mut().enumerate() {
            if let Some(value) = &mut write.value {
                value.compile(c, &format!("set[{i}].value"))?;
            }
        }
        if let Some(yields) = &mut input.yields {
            yields.compile(c, "yields")?;
        }
        if let Some(outcome) = &mut input.outcome {
            outcome.compile(c, "outcome")?;
        }
        Ok(())
    }

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &ScriptInput) -> Result<TaskOutcome> {
        if let Some(yields) = &input.yields {
            let n = Value::from(&yields.eval(ctx)?).as_u64().unwrap_or(0);
            for _ in 0..n {
                tokio::task::yield_now().await;
            }
        }
        if let Some(path) = &input.identity {
            let identity = json!({
                "workflow_id": ctx.workflow_id(),
                "task_id": ctx.task_id(),
                "loop_counter": ctx.loop_counter(),
                "element_index": ctx.element_index(),
            });
            ctx.set(path, OwnedDataValue::from(&identity));
        }
        for write in &input.set {
            let value = match (&write.value, &write.get, &write.raw) {
                (Some(template), None, None) => template.eval(ctx)?,
                (None, Some(path), None) => ctx.get(path).cloned().unwrap_or(OwnedDataValue::Null),
                (None, None, Some(raw)) => OwnedDataValue::from(raw),
                _ => {
                    return Err(DataflowError::Validation(format!(
                        "script: the write to {} needs exactly one of value, get and raw",
                        write.path
                    )));
                }
            };
            ctx.set(&write.path, value);
        }
        for error in &input.errors {
            ctx.add_error(ErrorInfo::builder(&error.code, &error.message).build());
        }
        match &input.outcome {
            None => Ok(TaskOutcome::Success),
            Some(template) => outcome(&Value::from(&template.eval(ctx)?)),
        }
    }
}

fn outcome(v: &Value) -> Result<TaskOutcome> {
    match v {
        Value::Null => Ok(TaskOutcome::Success),
        Value::String(s) if s == "success" => Ok(TaskOutcome::Success),
        Value::String(s) if s == "skip" => Ok(TaskOutcome::Skip),
        Value::String(s) if s == "halt" => Ok(TaskOutcome::Halt),
        Value::Object(o) if o.len() == 1 && o.contains_key("status") => {
            let status = o["status"].as_u64().and_then(|s| u16::try_from(s).ok());
            status.map(TaskOutcome::Status).ok_or_else(|| unknown(v))
        }
        Value::Object(o) if o.len() == 1 && o.contains_key("error") => Err(error(&o["error"])),
        _ => Err(unknown(v)),
    }
}

fn error(e: &Value) -> DataflowError {
    let message = e["message"]
        .as_str()
        .unwrap_or("scripted failure")
        .to_owned();
    match e["variant"].as_str().unwrap_or("service") {
        "service" => DataflowError::service(e["code"].as_str().unwrap_or_default(), message)
            .retryable(e["retryable"].as_bool().unwrap_or(false))
            .build(),
        "http" => DataflowError::http(
            e["status"]
                .as_u64()
                .and_then(|s| u16::try_from(s).ok())
                .unwrap_or(500),
            message,
        ),
        "timeout" => DataflowError::Timeout(message),
        "io" => DataflowError::Io(message),
        "unknown" => DataflowError::Unknown(message),
        "task" => DataflowError::Task(message),
        "validation" => DataflowError::Validation(message),
        "function" => DataflowError::function_execution(message, None),
        "deserialization" => DataflowError::Deserialization(message),
        "logic" => DataflowError::LogicEvaluation(message),
        "workflow" => DataflowError::Workflow(message),
        other => DataflowError::Validation(format!("script: unknown error variant {other}")),
    }
}

fn unknown(v: &Value) -> DataflowError {
    DataflowError::Validation(format!("script: unknown scripted outcome: {v}"))
}
