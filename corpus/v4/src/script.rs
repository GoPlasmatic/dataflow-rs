//! The corpus's one host function, v3's `script` (`corpus/v3/src/script.rs`)
//! on v4's `Function`. Its config is v3's input, compiled: a case's `script`
//! tasks compile here exactly as they run.
//!
//! In order, `call`:
//! 1. yields `yields` times;
//! 2. writes its identity to `identity`;
//! 3. applies `set`, each write through `Ctx::set`;
//! 4. records `errors` through `Ctx::record_error`;
//! 5. evaluates `outcome` and returns it.
//!
//! An error `outcome` is v3's variant as a service error carrying v3's code
//! and v3's `Display` text, so the record the engine writes, "Task {id}
//! error: {text}", is byte-equal to v3's.

use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::CompileCx;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::program::IssueCode;
use dataflow_rs::run::ErrorRecord;
use dataflow_rs::{Ctx, Error, Expr, Function, Outcome, Path, Result};
use serde_json::Value;
use std::borrow::Cow;

/// `script`: everything it does is in its config.
pub struct Script;

/// `script`'s input. An unknown key is refused, as every v4 config refuses
/// one: break 2a.
#[derive(FunctionConfig)]
pub struct ScriptConfig {
    /// The writes, in order.
    #[config(default_json = "[]")]
    pub set: Vec<Write>,
    /// The errors to add, in order.
    #[config(default_json = "[]")]
    pub errors: Vec<AddError>,
    /// Where to write the call's identity: workflow, task, loop counter and
    /// element index.
    pub identity: Option<Expr<Path>>,
    /// How many times to yield before anything else.
    pub yields: Option<Expr<u64>>,
    /// The outcome, evaluated last: `"success"`, `"skip"`, `"halt"`,
    /// `{"status": n}` or `{"error": {...}}`. Absent or `null` is success.
    pub outcome: Option<Expr>,
}

/// One write to `path`: of `value` (evaluated), of what `get` reads, or of
/// `raw` (never evaluated). Exactly one of the three is set, which the
/// compile checks (v3 failed the call instead).
#[derive(FunctionConfig)]
#[config(check = one_of)]
pub struct Write {
    /// Where the value is written.
    pub path: Expr<Path>,
    /// An expression, evaluated.
    pub value: Option<Expr>,
    /// A path, read; `null` when absent.
    pub get: Option<Expr<Path>>,
    /// The config value itself.
    pub raw: Option<Value>,
}

/// Refuses a write naming none, or more than one, of `value`, `get` and
/// `raw`.
fn one_of(write: &Write, cx: &mut CompileCx<'_>) {
    let named = [
        write.value.is_some(),
        write.get.is_some(),
        write.raw.is_some(),
    ];
    if named.iter().filter(|n| **n).count() != 1 {
        cx.issue(
            IssueCode::InputParse,
            "a script write needs exactly one of value, get and raw",
        );
    }
}

/// One error the call adds to the message.
#[derive(FunctionConfig)]
pub struct AddError {
    /// The error's code.
    pub code: String,
    /// The error's message.
    pub message: String,
}

impl Function for Script {
    type Config = ScriptConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &ScriptConfig) -> Result<Outcome> {
        if let Some(yields) = &cfg.yields {
            for _ in 0..ctx.eval(yields)? {
                tokio::task::yield_now().await;
            }
        }
        if let Some(path) = &cfg.identity {
            let path = ctx.eval(path)?;
            let identity = identity(ctx);
            ctx.set(&path, identity)?;
        }
        for write in &cfg.set {
            let path = ctx.eval(&write.path)?;
            let value = match (&write.value, &write.get, &write.raw) {
                (Some(value), _, _) => ctx.eval(value)?.into_owned(),
                (None, Some(get), _) => {
                    let at = ctx.eval(get)?;
                    ctx.get(&at).map_or(OwnedDataValue::Null, Cow::into_owned)
                }
                (None, None, Some(raw)) => OwnedDataValue::from(raw),
                (None, None, None) => unreachable!("`one_of` refuses a write with no value"),
            };
            ctx.set(&path, value)?;
        }
        for error in &cfg.errors {
            ctx.record_error(ErrorRecord::new(&error.code, &error.message));
        }
        match &cfg.outcome {
            None => Ok(Outcome::Ok),
            Some(outcome) => {
                let value = Value::from(&*ctx.eval(outcome)?);
                scripted(&value)
            }
        }
    }
}

/// The call's identity, keys in v3's order: v3 built it with `json!`, whose
/// map sorts them.
fn identity(ctx: &Ctx<'_>) -> OwnedDataValue {
    let info = ctx.info();
    OwnedDataValue::Object(vec![
        (
            "element_index".to_owned(),
            OwnedDataValue::from(info.element_index),
        ),
        (
            "loop_counter".to_owned(),
            OwnedDataValue::from(info.loop_counter),
        ),
        ("task_id".to_owned(), OwnedDataValue::from(info.task_id)),
        (
            "workflow_id".to_owned(),
            OwnedDataValue::from(info.workflow_id),
        ),
    ])
}

/// The evaluated `outcome`, as v3's `outcome()` reads it.
fn scripted(v: &Value) -> Result<Outcome> {
    match v {
        Value::Null => Ok(Outcome::Ok),
        Value::String(s) if s == "success" => Ok(Outcome::Ok),
        Value::String(s) if s == "skip" => Ok(Outcome::Skip),
        Value::String(s) if s == "halt" => Ok(Outcome::Halt),
        Value::Object(o) if o.len() == 1 && o.contains_key("status") => o["status"]
            .as_u64()
            .and_then(|s| u16::try_from(s).ok())
            .map(Outcome::Status)
            .ok_or_else(|| unknown(v)),
        Value::Object(o) if o.len() == 1 && o.contains_key("error") => Err(error(&o["error"])),
        _ => Err(unknown(v)),
    }
}

/// v3's error variant `e` names, as a service error with v3's code, v3's
/// `Display` text and v3's retryability.
fn error(e: &Value) -> Error {
    let message = e["message"].as_str().unwrap_or("scripted failure");
    let (code, text, retryable) = match e["variant"].as_str().unwrap_or("service") {
        // v3's service error: its code verbatim (an empty one is recorded as
        // `TASK_ERROR`, as v4's is), its bare message, its declared
        // retryability.
        "service" => (
            e["code"].as_str().unwrap_or_default().to_owned(),
            message.to_owned(),
            e["retryable"].as_bool().unwrap_or(false),
        ),
        "http" => {
            let status = e["status"]
                .as_u64()
                .and_then(|s| u16::try_from(s).ok())
                .unwrap_or(500);
            (
                "HTTP_ERROR".to_owned(),
                format!("HTTP error: {status} - {message}"),
                status >= 500 || matches!(status, 429 | 408 | 0),
            )
        }
        "timeout" => v3("TIMEOUT_ERROR", "Timeout error", message, true),
        "io" => v3("IO_ERROR", "IO error", message, true),
        "unknown" => v3("UNKNOWN_ERROR", "Unknown error", message, false),
        "task" => v3("TASK_ERROR", "Task error", message, false),
        "validation" => v3("VALIDATION_ERROR", "Validation error", message, false),
        "function" => v3("FUNCTION_ERROR", "Function execution error", message, false),
        "deserialization" => v3(
            "DESERIALIZATION_ERROR",
            "Deserialization error",
            message,
            false,
        ),
        "logic" => v3("LOGIC_ERROR", "Logic evaluation error", message, false),
        "workflow" => v3("WORKFLOW_ERROR", "Workflow error", message, false),
        other => v3(
            "VALIDATION_ERROR",
            "Validation error",
            &format!("script: unknown error variant {other}"),
            false,
        ),
    };
    Error::service(code, text).with_retryable(retryable)
}

/// A v3 variant's code, its `Display` text ("{prefix}: {message}") and its
/// retryability.
fn v3(code: &str, prefix: &str, message: &str, retryable: bool) -> (String, String, bool) {
    (code.to_owned(), format!("{prefix}: {message}"), retryable)
}

/// v3's error for an outcome it cannot read: a validation error.
fn unknown(v: &Value) -> Error {
    let (code, text, _) = v3(
        "VALIDATION_ERROR",
        "Validation error",
        &format!("script: unknown scripted outcome: {v}"),
        false,
    );
    Error::service(code, text)
}
