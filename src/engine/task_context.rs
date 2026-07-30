//! # Task context
//!
//! Wraps the per-call state passed to every `AsyncFunctionHandler::execute`
//! call: the message under processing, a handle to the shared datalogic
//! engine, and an audit-trail accumulator. Exposes typed helpers so handlers
//! don't have to reach into `crate::engine::utils::{get,set}_nested_value`
//! or hand-build `Change` entries.
//!
//! Custom handlers should treat `TaskContext` as their *only* mutation surface
//! for `message.context`: the `set` family records a `Change` automatically
//! when `message.capture_changes` is true, keeping the audit trail in sync
//! with the data without per-handler boilerplate.

use crate::engine::error::{DataflowError, ErrorInfo, Result};
use crate::engine::message::{Change, Message};
use crate::engine::utils::{get_nested_value, set_nested_value};
use datalogic_rs::{Engine as DatalogicEngine, Logic};
use datavalue::OwnedDataValue;
use serde_json::Value as JsonValue;
use std::sync::Arc;

/// Per-call execution context handed to `AsyncFunctionHandler::execute`.
///
/// Borrows the message and datalogic engine for the duration of the handler
/// call; collects `Change` entries that the workflow executor folds into the
/// audit trail when the handler returns. Drop semantics are trivial — there
/// is nothing to flush; the executor extracts the buffered changes via
/// `into_changes()`.
pub struct TaskContext<'a> {
    message: &'a mut Message,
    datalogic: &'a Arc<DatalogicEngine>,
    /// Changes accumulated through the `set*` family. Only populated when
    /// `message.capture_changes` is true; otherwise pushes are no-ops to
    /// keep the bulk-pipeline fast path allocation-free.
    changes: Vec<Change>,
}

impl<'a> TaskContext<'a> {
    /// Construct a new context. Mostly engine-internal — handlers receive a
    /// pre-built `&mut TaskContext` from the executor — but exposed `pub` so
    /// tests and benchmarks can drive `AsyncFunctionHandler::execute`
    /// directly without going through `Engine::process_message`.
    pub fn new(message: &'a mut Message, datalogic: &'a Arc<DatalogicEngine>) -> Self {
        Self {
            message,
            datalogic,
            changes: Vec::new(),
        }
    }

    /// Borrow the message under processing. Use this when you need to inspect
    /// the message id, payload, or audit trail; for reading and mutating the
    /// `data` / `metadata` / `temp_data` context, prefer the typed helpers on
    /// `TaskContext` itself.
    #[inline]
    pub fn message(&self) -> &Message {
        self.message
    }

    /// Mutable access to the message. Prefer the typed helpers (`set`,
    /// `add_error`) over poking at `message.context` directly — direct
    /// mutations bypass the audit trail.
    #[inline]
    pub fn message_mut(&mut self) -> &mut Message {
        self.message
    }

    /// Shared datalogic engine, in case the handler needs to evaluate ad-hoc
    /// JSONLogic. Most handlers can ignore this argument.
    #[inline]
    pub fn datalogic(&self) -> &Arc<DatalogicEngine> {
        self.datalogic
    }

    /// Read-only view of `data`. Returns `&OwnedDataValue::Null` if missing
    /// (mirrors the `Index` fallback semantics of `serde_json::Value`).
    #[inline]
    pub fn data(&self) -> &OwnedDataValue {
        self.message.data()
    }

    /// Read-only view of `metadata`.
    #[inline]
    pub fn metadata(&self) -> &OwnedDataValue {
        self.message.metadata()
    }

    /// Read-only view of `temp_data`.
    #[inline]
    pub fn temp_data(&self) -> &OwnedDataValue {
        self.message.temp_data()
    }

    /// The full `{data, metadata, temp_data}` tree — the root every workflow
    /// JSONLogic expression is written against.
    ///
    /// [`Self::data`] / [`Self::metadata`] / [`Self::temp_data`] expose the three
    /// slots individually; this is the whole-context accessor, so handlers do not
    /// have to reach through `ctx.message().context`.
    ///
    /// Note `payload` is **not** part of this tree, and therefore not part of the
    /// JSONLogic evaluation context — `{"var": "payload.foo"}` resolves to
    /// nothing. Parse the payload into `data` first.
    #[inline]
    pub fn context(&self) -> &OwnedDataValue {
        &self.message.context
    }

    /// Evaluate a pre-compiled expression against the message context, on the
    /// worker thread's pooled arena.
    ///
    /// The same path [`crate::engine::executor::evaluate_condition`] takes, but
    /// returning the value instead of collapsing it to a bool, and surfacing
    /// evaluation failures as `Err` instead of `false`. That difference is
    /// deliberate: a condition that fails to evaluate should not run its task,
    /// whereas a handler reading a config value needs to know the read failed.
    ///
    /// # Errors
    ///
    /// [`DataflowError::LogicEvaluation`] if the expression fails to evaluate.
    pub fn eval(&self, logic: &Logic) -> Result<OwnedDataValue> {
        crate::engine::executor::eval_to_owned(self.datalogic, logic, &self.message.context)
            .map_err(|e| DataflowError::LogicEvaluation(e.to_string()))
    }

    /// As [`Self::eval`], projected straight from the arena to
    /// `serde_json::Value` in one walk — no `OwnedDataValue` intermediate and no
    /// `serde_json::from_value` rebuild.
    pub fn eval_json(&self, logic: &Logic) -> Result<JsonValue> {
        crate::engine::executor::eval_to_json(self.datalogic, logic, &self.message.context)
            .map_err(|e| DataflowError::LogicEvaluation(e.to_string()))
    }

    /// As [`Self::eval`], coerced to a *plain* string: a JSON string result
    /// yields its contents, anything else its compact JSON form.
    ///
    /// # This disagrees with datalogic-rs on purpose
    ///
    /// datalogic-rs's `String: FromDataValue` — and therefore
    /// `Session::eval_str` — keeps the JSON quoting, so a string result comes
    /// back from it as `"\"abc\""`. This method returns `abc`.
    ///
    /// The name says `plain_string` rather than `to_string` precisely so the
    /// difference is visible at the call site: two string semantics in one
    /// ecosystem is a footgun, and these values end up in URL paths and message
    /// keys. A test pins both sides, so it fails if either changes.
    pub fn eval_to_plain_string(&self, logic: &Logic) -> Result<String> {
        crate::engine::executor::eval_to_plain_string(self.datalogic, logic, &self.message.context)
            .map_err(|e| DataflowError::LogicEvaluation(e.to_string()))
    }

    /// Look up a value by dot-path against the full context tree (rooted at
    /// the unified `{data, metadata, temp_data}` object). Returns `None` if
    /// the path doesn't resolve.
    ///
    /// Use the same path syntax as JSONLogic: `"data.user.name"`,
    /// `"temp_data.items.0"`, `"metadata.progress.status_code"`.
    #[inline]
    pub fn get(&self, path: &str) -> Option<&OwnedDataValue> {
        get_nested_value(&self.message.context, path)
    }

    /// Set a value at a dot-path on the context. Records a `Change` on the
    /// audit trail when `message.capture_changes` is true; otherwise the
    /// write happens but no audit entry is buffered.
    ///
    /// Intermediate objects/arrays are created on demand; see
    /// [`crate::engine::utils::set_nested_value`] for the exact semantics
    /// (numeric segments → arrays, `#` prefix → escaped object key, etc.).
    pub fn set(&mut self, path: &str, value: OwnedDataValue) {
        if self.message.capture_changes {
            let old_value = get_nested_value(&self.message.context, path)
                .cloned()
                .unwrap_or(OwnedDataValue::Null);
            let new_value = value.clone();
            self.changes.push(Change {
                path: Arc::from(path),
                old_value,
                new_value,
            });
        }
        set_nested_value(&mut self.message.context, path, value);
    }

    /// Same as [`Self::set`] but accepts a `serde_json::Value` (bridges
    /// through `OwnedDataValue::from`). Convenience for handlers that
    /// already speak `serde_json::Value`.
    #[inline]
    pub fn set_json(&mut self, path: &str, value: &JsonValue) {
        self.set(path, OwnedDataValue::from(value));
    }

    /// Append an error to `message.errors`. Convenience for
    /// `ctx.message_mut().add_error(...)`.
    #[inline]
    pub fn add_error(&mut self, error: ErrorInfo) {
        self.message.add_error(error);
    }

    /// Drain the accumulated changes. The workflow executor calls this after
    /// the handler returns to fold them into the audit trail; tests and
    /// benchmarks driving the trait directly can use it to inspect what the
    /// handler buffered.
    #[inline]
    pub fn into_changes(self) -> Vec<Change> {
        self.changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::executor::with_arena;
    use crate::engine::utils::set_nested_value;
    use serde_json::json;

    fn dv(v: serde_json::Value) -> OwnedDataValue {
        OwnedDataValue::from(&v)
    }

    fn engine() -> Arc<DatalogicEngine> {
        Arc::new(DatalogicEngine::builder().with_templating(true).build())
    }

    /// A message with one key in each of the three context slots.
    fn populated() -> Message {
        let mut m = Message::from_value(&json!({"payload_key": "payload_value"}));
        set_nested_value(&mut m.context, "data.x", dv(json!("dx")));
        set_nested_value(&mut m.context, "metadata.x", dv(json!("mx")));
        set_nested_value(&mut m.context, "temp_data.x", dv(json!("tx")));
        m
    }

    #[test]
    fn context_matches_the_three_slot_accessors() {
        let mut m = populated();
        let dl = engine();
        let ctx = TaskContext::new(&mut m, &dl);

        let whole = ctx.context();
        assert_eq!(&whole["data"], ctx.data());
        assert_eq!(&whole["metadata"], ctx.metadata());
        assert_eq!(&whole["temp_data"], ctx.temp_data());
        assert_eq!(whole, &ctx.message().context);
    }

    #[test]
    fn eval_roots_at_the_unified_context_not_data_alone() {
        let mut m = populated();
        let dl = engine();
        let ctx = TaskContext::new(&mut m, &dl);

        for (path, expected) in [
            ("data.x", "dx"),
            ("metadata.x", "mx"),
            ("temp_data.x", "tx"),
        ] {
            let logic = dl.compile_arc(&json!({"var": path})).unwrap();
            assert_eq!(ctx.eval(&logic).unwrap(), dv(json!(expected)));
            assert_eq!(ctx.eval_json(&logic).unwrap(), json!(expected));
            assert_eq!(ctx.eval_to_plain_string(&logic).unwrap(), expected);
        }
    }

    #[test]
    fn payload_is_not_in_the_eval_context() {
        // Stays true through the new surface: `payload` is a separate field on
        // Message and never part of the JSONLogic root.
        let mut m = populated();
        let dl = engine();
        let ctx = TaskContext::new(&mut m, &dl);

        let logic = dl
            .compile_arc(&json!({"var": "payload.payload_key"}))
            .unwrap();
        assert_eq!(ctx.eval(&logic).unwrap(), OwnedDataValue::Null);
        assert_eq!(ctx.eval_json(&logic).unwrap(), serde_json::Value::Null);
        assert_eq!(ctx.eval_to_plain_string(&logic).unwrap(), "null");
    }

    #[test]
    fn eval_json_covers_every_result_kind() {
        let mut m = Message::from_value(&json!({}));
        let dl = engine();
        let ctx = TaskContext::new(&mut m, &dl);

        for expected in [
            json!(null),
            json!(true),
            json!(42),
            json!(1.5),
            json!("abc"),
            json!([1, 2]),
            json!({"a": 1}),
            json!({"a": [1, {"b": "c"}], "d": {"e": [true, null]}}),
        ] {
            let logic = dl.compile_arc(&expected).unwrap();
            assert_eq!(
                ctx.eval_json(&logic).unwrap(),
                expected,
                "round-trip for {expected}"
            );
            // The owned and JSON projections agree on the same result.
            assert_eq!(
                serde_json::Value::from(&ctx.eval(&logic).unwrap()),
                expected
            );
        }
    }

    #[test]
    fn eval_to_plain_string_unquotes_strings_and_compacts_the_rest() {
        let mut m = Message::from_value(&json!({}));
        let dl = engine();
        let ctx = TaskContext::new(&mut m, &dl);

        let cases = [
            (json!("abc"), "abc"),
            (json!(""), ""),
            (json!(null), "null"),
            (json!(true), "true"),
            (json!(42), "42"),
            (json!({"a": 1}), "{\"a\":1}"),
            (json!([1, 2]), "[1,2]"),
        ];
        for (input, expected) in cases {
            let logic = dl.compile_arc(&input).unwrap();
            assert_eq!(
                ctx.eval_to_plain_string(&logic).unwrap(),
                expected,
                "for {input}"
            );
        }
    }

    #[test]
    fn eval_to_plain_string_diverges_from_datalogics_own_string_projection() {
        // This test IS the documentation of the divergence — it must fail if
        // either side changes. datalogic-rs's `String: FromDataValue` keeps the
        // JSON quoting; ours does not.
        let mut m = Message::from_value(&json!({}));
        let dl = engine();
        let ctx = TaskContext::new(&mut m, &dl);

        // Non-ASCII plus an embedded quote, to cover escaping too.
        let raw = "héllo \"world\" 世界";
        let logic = dl.compile_arc(&json!(raw)).unwrap();

        // Ours: contents, byte-identical.
        assert_eq!(ctx.eval_to_plain_string(&logic).unwrap(), raw);

        // datalogic's: JSON-quoted and escaped.
        let via_session = dl.session().eval_str(&logic, &m.context).unwrap();
        assert_ne!(
            via_session, raw,
            "if these agree, the divergence this method exists for is gone"
        );
        assert!(
            via_session.starts_with('"') && via_session.contains("\\\""),
            "datalogic keeps the quoting and escaping, got: {via_session}"
        );
    }

    #[test]
    fn eval_surfaces_an_error_where_evaluate_condition_returns_false() {
        // Asserted together so the difference is intentional and visible: a
        // condition that fails should not run its task; a handler reading a
        // config value needs to know the read failed.
        let mut m = Message::from_value(&json!({}));
        let dl = engine();

        // `+` over a non-numeric operand fails to evaluate.
        let bad = dl.compile_arc(&json!({"+": ["abc", 1]})).unwrap();

        let condition_result =
            crate::engine::executor::evaluate_condition(&dl, Some(&bad), &m.context);

        let ctx = TaskContext::new(&mut m, &dl);
        let eval_result = ctx.eval(&bad);

        match (&condition_result, &eval_result) {
            (Ok(false), Err(DataflowError::LogicEvaluation(msg))) => {
                assert!(!msg.is_empty(), "the error message must not be empty");
            }
            other => panic!(
                "expected evaluate_condition Ok(false) alongside eval Err(LogicEvaluation), got {other:?}"
            ),
        }
    }

    #[test]
    fn consecutive_evals_and_interleaved_sets_both_work() {
        // The arena is rewound between calls, not corrupted; and an eval between
        // two `set`s leaves the buffered Changes intact.
        let mut m = populated();
        let dl = engine();
        let first = dl.compile_arc(&json!({"var": "data.x"})).unwrap();
        let second = dl.compile_arc(&json!({"var": "metadata.x"})).unwrap();

        let mut ctx = TaskContext::new(&mut m, &dl);

        assert_eq!(ctx.eval_json(&first).unwrap(), json!("dx"));
        assert_eq!(ctx.eval_json(&second).unwrap(), json!("mx"));
        assert_eq!(ctx.eval_json(&first).unwrap(), json!("dx"));

        ctx.set("data.written", dv(json!(1)));
        assert_eq!(ctx.eval_json(&first).unwrap(), json!("dx"));
        ctx.set("data.written2", dv(json!(2)));

        let changes = ctx.into_changes();
        let paths: Vec<&str> = changes.iter().map(|c| &*c.path).collect();
        assert_eq!(paths, vec!["data.written", "data.written2"]);
    }

    #[test]
    fn eval_inside_a_with_arena_scope_falls_back_instead_of_panicking() {
        // `TaskContext::new` is pub so a test or bench can construct one inside a
        // `with_arena` closure. The `try_borrow_mut` fallback makes that return
        // `Ok` on a fresh Bump rather than panicking out of the arena scope.
        let mut m = populated();
        let dl = engine();
        let logic = dl.compile_arc(&json!({"var": "data.x"})).unwrap();

        let got = with_arena(|_| {
            let ctx = TaskContext::new(&mut m, &dl);
            ctx.eval_json(&logic)
        });

        assert_eq!(got.unwrap(), json!("dx"));
    }
}
