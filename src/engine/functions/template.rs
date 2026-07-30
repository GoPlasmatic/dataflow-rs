//! # Template
//!
//! A config field whose authored JSON is a JSONLogic expression, for custom
//! handlers. This is the same shape the three built-in integration configs use
//! for their `*_logic` fields (`path_logic`, `body_logic`, `key_logic`,
//! `value_logic`) — `Template` makes that pattern available to any
//! [`crate::AsyncFunctionHandler`] without hand-rolling the raw/compiled pair.

use crate::engine::error::{DataflowError, Result};
use crate::engine::task_context::TaskContext;
use datalogic_rs::Logic;
use datavalue::OwnedDataValue;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::sync::Arc;

/// A config field whose authored JSON is a JSONLogic expression.
///
/// Deserializes from any JSON value and keeps it verbatim; the expression is
/// compiled once at engine construction (see [`crate::AsyncFunctionHandler::compile_input`])
/// and evaluated per message on the worker thread's pooled arena.
///
/// Declare this type only on fields the workflow author is told are JSONLogic —
/// the `*_logic` convention this crate's own built-ins use. Do **not** use it for
/// freeform config values: `LogicCompiler` builds its datalogic engine with
/// templating enabled, so a single-key object whose key happens to match an
/// operator name (`{"cat": ["a", "b"]}`) evaluates as that operator rather than
/// being returned as a literal object. That is existing behaviour for the
/// built-in `*_logic` fields, not a `Template`-specific regression — it is the
/// reason this type is opt-in per field rather than a blanket JSON wrapper.
#[derive(Debug, Clone)]
pub struct Template {
    raw: Value,
    compiled: Option<Arc<Logic>>,
}

// Hand-written rather than `#[serde(from = "Value")]` plus `impl From<Value>`:
// a container-level `from` builds the target solely through `From`, so a
// field-level `#[serde(skip)]` on `compiled` would be inert and misleading next
// to a manual `From` impl anyway. This is the same five lines, explicit about
// which path runs.
impl<'de> Deserialize<'de> for Template {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Ok(Self {
            raw: Value::deserialize(d)?,
            compiled: None,
        })
    }
}

impl Template {
    /// Compile the expression. Called once at engine construction via
    /// [`crate::AsyncFunctionHandler::compile_input`]. `label` is used only in
    /// the error message, matching `LogicCompiler`'s
    /// `"<what> for task <id> in workflow <id>"` convention.
    ///
    /// # Errors
    ///
    /// [`DataflowError::LogicEvaluation`] if the expression fails to compile,
    /// with `label` prefixed onto the message.
    pub fn compile(&mut self, c: &TemplateCompiler, label: &str) -> Result<()> {
        let compiled = c
            .engine
            .compile_arc(&self.raw)
            .map_err(|e| DataflowError::LogicEvaluation(format!("{label}: {e}")))?;
        self.compiled = Some(compiled);
        Ok(())
    }

    /// Evaluate against the message context, on the worker thread's pooled bump
    /// arena.
    ///
    /// # Errors
    ///
    /// [`DataflowError::LogicEvaluation`] if [`Self::compile`] was never called —
    /// naming the field is the caller's job via `label`, since this type has no
    /// field name of its own to report — or if evaluation itself fails.
    pub fn eval(&self, ctx: &TaskContext<'_>) -> Result<OwnedDataValue> {
        let logic = self.compiled.as_deref().ok_or_else(|| {
            DataflowError::LogicEvaluation(
                "Template::eval called before Template::compile — the engine did not compile \
                 this field at construction time"
                    .to_string(),
            )
        })?;
        ctx.eval(logic)
    }

    /// As [`Self::eval`], deserialized into `T`.
    ///
    /// Routes through `serde_json::Value` — [`TaskContext::eval_json`] then
    /// `serde_json::from_value` — so it costs one extra walk and rebuild past
    /// [`Self::eval`]. Prefer `eval` when `T` is `OwnedDataValue` or when you
    /// only need to inspect the result, not deserialize it into a caller type.
    ///
    /// # Errors
    ///
    /// As [`Self::eval`], plus a deserialization error if the evaluated JSON does
    /// not fit `T`.
    pub fn eval_into<T: serde::de::DeserializeOwned>(&self, ctx: &TaskContext<'_>) -> Result<T> {
        let logic = self.compiled.as_deref().ok_or_else(|| {
            DataflowError::LogicEvaluation(
                "Template::eval_into called before Template::compile — the engine did not \
                 compile this field at construction time"
                    .to_string(),
            )
        })?;
        let json = ctx.eval_json(logic)?;
        serde_json::from_value(json).map_err(DataflowError::from_serde)
    }

    /// The authored JSON, unchanged. For handlers that need to report or
    /// re-serialize their own config.
    pub fn as_json(&self) -> &Value {
        &self.raw
    }

    /// Whether [`Self::compile`] has run. Mainly for tests and for callers that
    /// want to assert the build pass reached them.
    pub fn is_compiled(&self) -> bool {
        self.compiled.is_some()
    }
}

/// Handed to [`crate::AsyncFunctionHandler::compile_input`] to compile a
/// handler's `Template` fields at engine construction.
///
/// Wraps the same `Arc<datalogic_rs::Engine>` `LogicCompiler` uses internally,
/// so a compiled `Template` is evaluable by the engine that will run the
/// message. A newtype rather than a bare `Arc<datalogic_rs::Engine>` so fields
/// can be added later without changing `compile_input`'s signature.
pub struct TemplateCompiler {
    engine: Arc<datalogic_rs::Engine>,
}

impl TemplateCompiler {
    pub(crate) fn new(engine: Arc<datalogic_rs::Engine>) -> Self {
        Self { engine }
    }

    /// The shared datalogic engine, for handlers that need to compile something
    /// other than a `Template` field directly.
    pub fn engine(&self) -> &datalogic_rs::Engine {
        &self.engine
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::message::Message;
    use serde_json::json;

    fn engine() -> Arc<datalogic_rs::Engine> {
        Arc::new(
            datalogic_rs::Engine::builder()
                .with_templating(true)
                .build(),
        )
    }

    fn template_from(v: Value) -> Template {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn deserializes_from_every_json_shape_and_as_json_is_verbatim() {
        for v in [
            json!({"a": 1}),
            json!([1, 2, 3]),
            json!("hello"),
            json!(42),
            json!(true),
            json!(null),
            json!({}),
        ] {
            let t = template_from(v.clone());
            assert_eq!(t.as_json(), &v);
            assert!(!t.is_compiled());
        }
    }

    #[test]
    fn eval_before_compile_errors_without_panicking() {
        let dl = engine();
        let mut m = Message::from_value(&json!({}));
        let ctx = TaskContext::new(&mut m, &dl);
        let t = template_from(json!({"var": "data.x"}));

        match t.eval(&ctx) {
            Err(DataflowError::LogicEvaluation(msg)) => {
                assert!(
                    msg.contains("compile"),
                    "message should name the cause: {msg}"
                );
            }
            other => panic!("expected LogicEvaluation, got {other:?}"),
        }
    }

    #[test]
    fn compile_on_a_malformed_expression_names_the_label() {
        // datalogic-rs's templating mode is deliberately permissive at compile
        // time: an unrecognised operator key compiles as a literal (or, at the
        // top level, a structured-object template) rather than erroring — this
        // is existing engine behaviour, not something `Template` controls, and
        // it is why a static "known operators" table would mislead (see #26's
        // scope notes). The one thing that reliably fails to *compile* — as
        // opposed to failing at *evaluation* — is rule nesting past the
        // engine's `MAX_COMPILE_DEPTH` (256), verified directly against
        // datalogic-rs 5.1.1 before writing this test.
        let c = TemplateCompiler::new(engine());
        let mut too_deep = json!(1);
        for _ in 0..300 {
            too_deep = json!({"var": too_deep});
        }
        let mut t = template_from(too_deep);

        match t.compile(&c, "my_field for task t in workflow w") {
            Err(DataflowError::LogicEvaluation(msg)) => {
                assert!(
                    msg.contains("my_field for task t in workflow w"),
                    "got: {msg}"
                );
            }
            other => panic!("expected LogicEvaluation, got {other:?}"),
        }
    }

    #[test]
    fn a_literal_template_evaluates_to_that_literal() {
        let dl = engine();
        let c = TemplateCompiler::new(Arc::clone(&dl));
        let mut m = Message::from_value(&json!({}));
        let ctx = TaskContext::new(&mut m, &dl);

        for v in [
            json!("hello"),
            json!(42),
            json!({}),
            json!({"a": 1, "b": 2}),
        ] {
            let mut t = template_from(v.clone());
            t.compile(&c, "lbl").unwrap();
            assert_eq!(t.eval_into::<Value>(&ctx).unwrap(), v);
        }
    }

    #[test]
    fn a_single_key_operator_name_evaluates_as_the_operator() {
        // Pinned deliberately: this is why Template is opt-in per field, not a
        // blanket wrapper. LogicCompiler enables templating, and this crate's
        // TaskContext-backed evaluation goes through the same engine.
        let dl = engine();
        let c = TemplateCompiler::new(Arc::clone(&dl));
        let mut m = Message::from_value(&json!({}));
        let ctx = TaskContext::new(&mut m, &dl);

        let mut t = template_from(json!({"cat": ["a", "b"]}));
        t.compile(&c, "lbl").unwrap();
        assert_eq!(t.eval_into::<Value>(&ctx).unwrap(), json!("ab"));
    }

    #[test]
    fn non_ascii_result_round_trips() {
        let dl = engine();
        let c = TemplateCompiler::new(Arc::clone(&dl));
        let mut m = Message::from_value(&json!({}));
        let ctx = TaskContext::new(&mut m, &dl);

        let mut t = template_from(json!({"cat": ["über-", "größe"]}));
        t.compile(&c, "lbl").unwrap();
        assert_eq!(t.eval_into::<String>(&ctx).unwrap(), "über-größe");
    }

    #[test]
    fn reading_an_absent_path_matches_the_engines_missing_path_result() {
        let dl = engine();
        let c = TemplateCompiler::new(Arc::clone(&dl));
        let mut m = Message::from_value(&json!({}));
        let ctx = TaskContext::new(&mut m, &dl);

        let mut t = template_from(json!({"var": "data.nope"}));
        t.compile(&c, "lbl").unwrap();
        // Not an error — the same "resolves to Null" behaviour as the built-in
        // *_logic fields on a missing path.
        assert_eq!(t.eval(&ctx).unwrap(), OwnedDataValue::Null);
    }
}
