//! # Template
//!
//! A config field whose authored JSON is a JSONLogic expression. Every
//! parameter of every built-in function is one of these, and custom handlers
//! declare them for their own config.
//!
//! A literal is JSONLogic for itself, so the static spelling an author already
//! writes — `"data.output"`, `30000`, `{"X-Env": "prod"}` — is a valid
//! `Template`. Those fold to a constant at compile time and are cached, so a
//! statically-authored parameter does no per-message work. See
//! [`Template::is_constant`].

use crate::engine::error::{DataflowError, Result};
use crate::engine::task_context::TaskContext;
use datalogic_rs::Logic;
use datavalue::OwnedDataValue;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::borrow::Cow;
use std::sync::Arc;

/// Coerce an already-evaluated value to a *plain* string: a string yields its
/// contents, anything else its compact JSON form.
///
/// Mirrors [`crate::engine::executor::eval_to_plain_string`] exactly, for the
/// constant-cache path that never reaches the evaluator.
/// `constant_and_evaluated_plain_strings_agree` pins the two together.
pub(crate) fn plain_string_of(value: &OwnedDataValue) -> String {
    match value {
        OwnedDataValue::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A config field whose authored JSON is a JSONLogic expression.
///
/// Deserializes from any JSON value and keeps it verbatim; the expression is
/// compiled once at engine construction (see [`crate::AsyncFunctionHandler::compile_input`])
/// and evaluated per message on the worker thread's pooled arena — unless it
/// folded to a constant, in which case the value is computed once at
/// construction and handed back directly.
///
/// # Literals and the `$` escape
///
/// A JSON scalar or array authored here is a literal: `"data.out"` resolves to
/// the string `data.out`, `30000` to the number. An *object* is where care is
/// needed, because the engine evaluates in templating mode: a single-key object
/// whose key matches an operator name is that operator. `{"cat": ["a", "b"]}`
/// resolves to `"ab"`, not to the object.
///
/// Prefix the key with [`Engine::template_key_escape`](crate::Engine::template_key_escape)
/// (`$`) to force the literal reading: `{"$cat": ["a", "b"]}` resolves to the
/// object `{"cat": ["a", "b"]}`. One prefix is stripped from every template key,
/// so a genuinely `$`-prefixed key doubles up — `{"$$oid": …}` emits `$oid`.
///
/// Before that escape existed a literal object with a colliding key was
/// inexpressible, which is why this type used to be documented as opt-in per
/// field. It no longer is: any config field may be a `Template`.
#[derive(Debug, Clone)]
pub struct Template {
    raw: Value,
    /// Everything [`Self::compile`] produces, behind one pointer.
    ///
    /// Boxed to keep `Template` small. Every parameter of every built-in is one
    /// of these — `HttpCallConfig` alone holds eight — and they live inside
    /// `FunctionConfig`, whose size is the size of its largest variant. Inline,
    /// the compiled state made that enum large enough for
    /// `clippy::large_enum_variant`. The indirection costs one deref on a path
    /// that is either cached or about to run a JSONLogic evaluation anyway.
    compiled: Option<Box<Compiled>>,
}

/// What compiling a [`Template`] produces.
#[derive(Debug, Clone)]
struct Compiled {
    logic: Arc<Logic>,
    /// `Some` when the expression folded to a compile-time constant — the value
    /// every `resolve_*` returns without touching the evaluator.
    constant: Option<OwnedDataValue>,
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

impl Default for Template {
    /// A `Template` over JSON `null`.
    ///
    /// Exists so config structs that derive `Default` still can. It is a
    /// placeholder, not a usable parameter — every config carrying one names
    /// the field as required, so a `Default`-built config is a base to fill in.
    fn default() -> Self {
        Self::from(Value::Null)
    }
}

impl From<Value> for Template {
    /// An uncompiled `Template` over `raw`. For hosts and tests building a
    /// config struct directly rather than deserializing one; `LogicCompiler`
    /// still has to compile it before any `resolve_*` call will succeed.
    fn from(raw: Value) -> Self {
        Self {
            raw,
            compiled: None,
        }
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

        // The datalogic compiler folds every static sub-expression it can
        // prove, so an expression with no data dependency — which is what a
        // statically-authored parameter is — collapses to a single literal
        // node. Evaluate it once here and keep the result: that is what makes
        // "every parameter is JSONLogic" cost nothing for the static spelling.
        //
        // `is_constant`, not `is_static`. `is_static` also reports true for a
        // rule the compiler *tried* to fold and could not because folding
        // errored (`{"/": [1, 0]}` divides by zero); evaluating those here
        // would move a runtime error to build time. A constant rule, by
        // contrast, has already been reduced to a value and cannot fail.
        let constant = if compiled.is_constant() {
            let empty = OwnedDataValue::Object(Vec::new());
            Some(
                crate::engine::executor::eval_to_owned(&c.engine, &compiled, &empty)
                    .map_err(|e| DataflowError::LogicEvaluation(format!("{label}: {e}")))?,
            )
        } else {
            None
        };

        self.compiled = Some(Box::new(Compiled {
            logic: compiled,
            constant,
        }));
        Ok(())
    }

    /// Whether the expression folded to a compile-time constant, so every
    /// `resolve_*` call returns a cached value instead of evaluating.
    ///
    /// True for the static spelling of any parameter — a scalar, an array, or
    /// an object template with no `var` in it. False once anything reads the
    /// message. Callers that precompute a derived form (a split write path, for
    /// instance) branch on this.
    ///
    /// Meaningless before [`Self::compile`]: an uncompiled `Template` reports
    /// `false` because nothing has been folded yet, not because the expression
    /// is dynamic.
    pub fn is_constant(&self) -> bool {
        self.constant().is_some()
    }

    /// The folded constant, when the expression compiled to one.
    fn constant(&self) -> Option<&OwnedDataValue> {
        self.compiled.as_ref().and_then(|c| c.constant.as_ref())
    }

    /// The folded constant coerced to a plain string, when the expression
    /// folded to one.
    ///
    /// Lets a caller do at compile time what [`Self::resolve_string`] would
    /// otherwise defer to the first message — which is how [`PathTemplate`]
    /// precomputes a static write path.
    ///
    /// [`PathTemplate`]: crate::PathTemplate
    pub fn constant_string(&self) -> Option<String> {
        self.constant().map(plain_string_of)
    }

    /// The parameter's value for this message: the cached constant when the
    /// expression folded, otherwise a fresh evaluation.
    ///
    /// This is the sanctioned read for a config parameter. [`Self::eval`] is
    /// the same thing without the constant cache, kept for handlers that hold
    /// a `Template` they compiled themselves.
    ///
    /// # Errors
    ///
    /// As [`Self::eval`].
    pub fn resolve(&self, ctx: &TaskContext<'_>) -> Result<OwnedDataValue> {
        if let Some(v) = self.constant() {
            return Ok(v.clone());
        }
        if let Some(v) = self.uncompiled_literal() {
            return Ok(v);
        }
        self.eval(ctx)
    }

    /// The authored value, for a config that never went through
    /// `LogicCompiler` — a struct built by hand in a test, a benchmark, or a
    /// host helper.
    ///
    /// Only JSON scalars qualify. A scalar is unambiguously itself in
    /// JSONLogic, so reading it directly cannot disagree with what compilation
    /// would have produced. An object may be an operator call and an array's
    /// elements may each be one, so those still need the compiler and fall
    /// through to the "never compiled" error.
    ///
    /// This is what keeps the pre-3.9 contract for directly-constructed
    /// configs: before, these parameters were plain `String`/`u64` fields that
    /// needed no compilation at all.
    fn uncompiled_literal(&self) -> Option<OwnedDataValue> {
        if self.compiled.is_some() {
            return None;
        }
        match &self.raw {
            Value::String(_) | Value::Number(_) | Value::Bool(_) => {
                Some(OwnedDataValue::from(&self.raw))
            }
            _ => None,
        }
    }

    /// As [`Self::resolve`], coerced to a *plain* string — a string result
    /// yields its contents, anything else its compact JSON form. Use this
    /// wherever the value becomes a URL path, a header value, a topic name or a
    /// write path, where JSON quoting would be wrong.
    ///
    /// # Errors
    ///
    /// As [`Self::eval`].
    pub fn resolve_string(&self, ctx: &TaskContext<'_>) -> Result<String> {
        if let Some(v) = self.constant() {
            return Ok(plain_string_of(v));
        }
        if let Some(v) = self.uncompiled_literal() {
            return Ok(plain_string_of(&v));
        }
        self.eval_to_plain_string(ctx)
    }

    /// As [`Self::resolve_string`], against a context already resident in
    /// `arena`, for callers inside an arena scope that hold no [`TaskContext`].
    ///
    /// The built-in sync executors (`map`, `parse`, `publish`) run against an
    /// [`ArenaContext`](crate::engine::executor::ArenaContext) that earlier
    /// tasks in the same stretch already populated. Routing them through
    /// `TaskContext` would re-walk the whole owned context into the arena per
    /// parameter, which is exactly the cost that context exists to avoid.
    ///
    /// # Errors
    ///
    /// As [`Self::resolve_string`].
    pub(crate) fn resolve_string_in_arena(
        &self,
        p: crate::engine::functions::path_template::ParamCtx<'_>,
    ) -> Result<String> {
        Ok(self.resolve_str_in_arena(p)?.into_owned())
    }

    /// As [`Self::resolve_string_in_arena`], borrowing when it can.
    ///
    /// A constant string parameter — the static spelling of a source, a target,
    /// a topic — is already a `String` on the compiled template, so returning
    /// it by value allocates on every message. These resolve per task per
    /// message on the sync path, where that allocation is precisely the cost
    /// the constant cache exists to avoid.
    ///
    /// # Errors
    ///
    /// As [`Self::resolve_string`].
    pub(crate) fn resolve_str_in_arena(
        &self,
        p: crate::engine::functions::path_template::ParamCtx<'_>,
    ) -> Result<Cow<'_, str>> {
        match self.constant() {
            Some(OwnedDataValue::String(s)) => return Ok(Cow::Borrowed(s)),
            Some(other) => return Ok(Cow::Owned(plain_string_of(other))),
            None => {}
        }
        // Uncompiled literal string: borrow straight off the authored JSON.
        if self.compiled.is_none() {
            if let Value::String(s) = &self.raw {
                return Ok(Cow::Borrowed(s));
            }
            if let Some(v) = self.uncompiled_literal() {
                return Ok(Cow::Owned(plain_string_of(&v)));
            }
        }
        let logic = self.compiled_or_err("resolve_str_in_arena")?;
        let evaluated = p
            .engine()
            .evaluate(logic, *p.context(), p.arena())
            .map_err(|e| DataflowError::LogicEvaluation(e.to_string()))?;
        Ok(Cow::Owned(match evaluated {
            datavalue::DataValue::String(s) => s.to_string(),
            other => other.to_string(),
        }))
    }

    /// The compiled logic, or the "never compiled" error naming `method`.
    fn compiled_or_err(&self, method: &str) -> Result<&Logic> {
        self.compiled.as_ref().map(|c| &*c.logic).ok_or_else(|| {
            DataflowError::LogicEvaluation(format!(
                "Template::{method} called before Template::compile — the engine did not \
                 compile this field at construction time"
            ))
        })
    }

    /// As [`Self::resolve`], as a `u64` — for parameters like `timeout_ms`.
    ///
    /// # Errors
    ///
    /// As [`Self::eval`], plus [`DataflowError::Validation`] when the result is
    /// not a number that fits a `u64`. A timeout that evaluated to `null`
    /// because its path was missing is a configuration error worth reporting,
    /// not something to silently default.
    pub fn resolve_u64(&self, ctx: &TaskContext<'_>, label: &str) -> Result<u64> {
        let value = self.resolve(ctx)?;
        match &value {
            OwnedDataValue::Number(n) => {
                // Reject NaN, negatives and anything past u64 range before the
                // `as` cast, which would otherwise saturate or produce 0.
                let f = n.as_f64();
                (f.is_finite() && f >= 0.0 && f <= u64::MAX as f64).then_some(f as u64)
            }
            _ => None,
        }
        .ok_or_else(|| {
            DataflowError::Validation(format!(
                "{label} must evaluate to a non-negative number, got {value}"
            ))
        })
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
        let logic = self.compiled.as_ref().map(|c| &*c.logic).ok_or_else(|| {
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
        let logic = self.compiled.as_ref().map(|c| &*c.logic).ok_or_else(|| {
            DataflowError::LogicEvaluation(
                "Template::eval_into called before Template::compile — the engine did not \
                 compile this field at construction time"
                    .to_string(),
            )
        })?;
        let json = ctx.eval_json(logic)?;
        serde_json::from_value(json).map_err(DataflowError::from_serde)
    }

    /// As [`Self::eval`], coerced to a *plain* string via
    /// [`TaskContext::eval_to_plain_string`] — a JSON string result yields its
    /// contents, anything else its compact JSON form. Use this when the result
    /// is going into a URL path or a message key, where JSON quoting would be
    /// wrong.
    ///
    /// # Errors
    ///
    /// As [`Self::eval`].
    pub fn eval_to_plain_string(&self, ctx: &TaskContext<'_>) -> Result<String> {
        let logic = self.compiled.as_ref().map(|c| &*c.logic).ok_or_else(|| {
            DataflowError::LogicEvaluation(
                "Template::eval_to_plain_string called before Template::compile — the engine \
                 did not compile this field at construction time"
                    .to_string(),
            )
        })?;
        ctx.eval_to_plain_string(logic)
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
        Arc::new(crate::engine::compiler::datalogic_engine_builder().build())
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
    fn an_operator_named_key_evaluates_unless_it_is_escaped() {
        // The reason `Template` used to be opt-in per field, and the reason it
        // no longer needs to be. Templating makes a single-key object an
        // operator invocation; the `$` escape is what makes the literal
        // reading expressible at all.
        let dl = engine();
        let c = TemplateCompiler::new(Arc::clone(&dl));
        let mut m = Message::from_value(&json!({}));
        let ctx = TaskContext::new(&mut m, &dl);

        let mut op = template_from(json!({"cat": ["a", "b"]}));
        op.compile(&c, "lbl").unwrap();
        assert_eq!(op.eval_into::<Value>(&ctx).unwrap(), json!("ab"));

        let mut escaped = template_from(json!({"$cat": ["a", "b"]}));
        escaped.compile(&c, "lbl").unwrap();
        assert_eq!(
            escaped.eval_into::<Value>(&ctx).unwrap(),
            json!({"cat": ["a", "b"]}),
            "an escaped key must emit the literal object"
        );

        // One prefix is stripped, so a genuinely `$`-prefixed key doubles up.
        let mut doubled = template_from(json!({"$$oid": "abc"}));
        doubled.compile(&c, "lbl").unwrap();
        assert_eq!(
            doubled.eval_into::<Value>(&ctx).unwrap(),
            json!({"$oid": "abc"})
        );
    }

    #[test]
    fn the_static_spelling_of_every_parameter_folds_to_a_constant() {
        // This is what makes "every parameter is JSONLogic" free: the way an
        // author already writes a parameter costs no per-message evaluation.
        let c = TemplateCompiler::new(engine());
        for v in [
            json!("data.output"),
            json!(30000),
            json!(true),
            json!(["a", "b"]),
            json!({"cat": ["a", "b"]}), // folds: no data dependency
        ] {
            let mut t = template_from(v.clone());
            t.compile(&c, "lbl").unwrap();
            assert!(t.is_constant(), "{v} should fold to a constant");
        }

        // Anything that reads the message cannot fold.
        for v in [
            json!({"var": "data.x"}),
            json!({"cat": [{"var": "data.x"}]}),
        ] {
            let mut t = template_from(v.clone());
            t.compile(&c, "lbl").unwrap();
            assert!(!t.is_constant(), "{v} must not fold");
        }
    }

    #[test]
    fn constant_and_evaluated_plain_strings_agree() {
        // `resolve_string` short-circuits the evaluator for a constant, so its
        // coercion is a second implementation of `eval_to_plain_string`. If the
        // two ever disagree, a static parameter and its dynamic twin would put
        // different bytes in a URL.
        let dl = engine();
        let c = TemplateCompiler::new(Arc::clone(&dl));
        let mut m = Message::from_value(&json!({}));
        let ctx = TaskContext::new(&mut m, &dl);

        for v in [
            json!("abc"),
            json!(7),
            json!(true),
            json!(null),
            json!(["a", 1]),
        ] {
            let mut t = template_from(v.clone());
            t.compile(&c, "lbl").unwrap();
            assert!(t.is_constant(), "{v} should fold");
            assert_eq!(
                t.resolve_string(&ctx).unwrap(),
                t.eval_to_plain_string(&ctx).unwrap(),
                "cached and evaluated coercion disagree for {v}"
            );
        }
    }

    #[test]
    fn an_escaped_key_does_not_fold_to_a_constant() {
        // Worth pinning because it is counter-intuitive and costs something:
        // `{"$cat": …}` has no data dependency, yet the compiler keeps it as a
        // node rather than folding it, so an escaped literal is re-materialised
        // per message where an unescaped one is cached.
        //
        // Only `resolve` (and its typed siblings) are affected — the *value* is
        // identical either way, which is what the assertion below fixes. If a
        // future datalogic release starts folding escaped keys this test fails
        // and the only change needed is to delete it.
        let dl = engine();
        let c = TemplateCompiler::new(Arc::clone(&dl));
        let mut m = Message::from_value(&json!({}));
        let ctx = TaskContext::new(&mut m, &dl);

        let mut t = template_from(json!({"$a": 1}));
        t.compile(&c, "lbl").unwrap();
        assert!(!t.is_constant(), "escaped keys are not folded today");
        assert_eq!(t.eval_into::<Value>(&ctx).unwrap(), json!({"a": 1}));
        assert_eq!(
            t.resolve_string(&ctx).unwrap(),
            t.eval_to_plain_string(&ctx).unwrap()
        );
    }

    #[test]
    fn resolve_u64_accepts_numbers_and_rejects_everything_else() {
        let dl = engine();
        let c = TemplateCompiler::new(Arc::clone(&dl));
        let mut m = Message::from_value(&json!({}));
        let ctx = TaskContext::new(&mut m, &dl);

        let mut ok = template_from(json!(30000));
        ok.compile(&c, "lbl").unwrap();
        assert_eq!(ok.resolve_u64(&ctx, "timeout_ms").unwrap(), 30000);

        // A missing path resolves to null rather than erroring, so without this
        // check a mistyped timeout would silently become 0.
        for bad in [json!(null), json!("30000"), json!(-1), json!({"a": 1})] {
            let mut t = template_from(bad.clone());
            t.compile(&c, "lbl").unwrap();
            let err = t
                .resolve_u64(&ctx, "timeout_ms")
                .expect_err("{bad} must be rejected");
            assert!(err.to_string().contains("timeout_ms"), "{err}");
        }
    }

    #[test]
    fn eval_to_plain_string_unquotes_and_coerces_non_strings() {
        let dl = engine();
        let c = TemplateCompiler::new(Arc::clone(&dl));
        let mut m = Message::from_value(&json!({}));
        let ctx = TaskContext::new(&mut m, &dl);

        let mut string_t = template_from(json!("abc"));
        string_t.compile(&c, "lbl").unwrap();
        assert_eq!(string_t.eval_to_plain_string(&ctx).unwrap(), "abc");

        let mut num_t = template_from(json!(7));
        num_t.compile(&c, "lbl").unwrap();
        assert_eq!(num_t.eval_to_plain_string(&ctx).unwrap(), "7");

        let mut obj_t = template_from(json!({"a": 1}));
        obj_t.compile(&c, "lbl").unwrap();
        assert_eq!(obj_t.eval_to_plain_string(&ctx).unwrap(), "{\"a\":1}");
    }

    #[test]
    fn eval_to_plain_string_before_compile_errors_without_panicking() {
        let mut m = Message::from_value(&json!({}));
        let dl = engine();
        let ctx = TaskContext::new(&mut m, &dl);
        let t = template_from(json!("abc"));

        match t.eval_to_plain_string(&ctx) {
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
