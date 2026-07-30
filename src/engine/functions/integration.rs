use crate::engine::error::Result;
use crate::engine::task_context::TaskContext;
use datalogic_rs::Logic;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Configuration for the http_call integration function.
///
/// The actual HTTP implementation is provided by the service layer via AsyncFunctionHandler.
/// This struct provides typed config validation and pre-compilation of JSONLogic expressions.
///
/// Unknown keys are rejected. A misspelled field previously parsed cleanly and
/// was discarded, so an `http_call` task could make its request and silently
/// throw the response away with no error at build time and none at dispatch.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpCallConfig {
    /// Named connector reference (resolved by service layer)
    pub connector: String,

    /// HTTP method
    #[serde(default = "default_method")]
    pub method: HttpMethod,

    /// Static path string
    #[serde(default)]
    pub path: Option<String>,

    /// JSONLogic expression to compute path dynamically
    #[serde(default)]
    pub path_logic: Option<Value>,

    /// Engine-internal: pre-compiled `path_logic`, populated by
    /// `LogicCompiler`. Read it through [`HttpCallConfig::resolve_path`] rather
    /// than directly — that method applies the static-`path` fallback and is the
    /// sanctioned read. Not part of the stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub compiled_path_logic: Option<Arc<Logic>>,

    /// Static headers
    #[serde(default)]
    pub headers: HashMap<String, String>,

    /// Static request body
    #[serde(default)]
    pub body: Option<Value>,

    /// JSONLogic expression to compute body dynamically
    #[serde(default)]
    pub body_logic: Option<Value>,

    /// Engine-internal: pre-compiled `body_logic`, populated by
    /// `LogicCompiler`. Read it through [`HttpCallConfig::resolve_body`]. Not
    /// part of the stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub compiled_body_logic: Option<Arc<Logic>>,

    /// JSONPath/dot-path to extract from response and merge into context.
    ///
    /// `output` is accepted as an alias, so a service layer can present one
    /// destination-field name across its whole function catalogue. Supplying
    /// both keys is a `duplicate field` error rather than a precedence rule.
    #[serde(default, alias = "output")]
    pub response_path: Option<String>,

    /// Request timeout in milliseconds (default: 30000)
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

/// HTTP methods supported by `http_call`.
///
/// This crate does not implement `http_call` — the transport is supplied by the
/// service layer via `AsyncFunctionHandler` — so every consumer converts this
/// into their own HTTP client's method type. [`HttpMethod::as_str`] is the
/// intended bridge (e.g. `Method::from_bytes(m.as_str().as_bytes())`); the crate
/// deliberately takes no HTTP-client dependency of its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    #[default]
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    /// Every method a workflow may name in an `http_call` task.
    ///
    /// This is the vocabulary a service layer can validate its own
    /// operator-facing method allow-lists against, instead of mirroring the
    /// variant list by hand.
    ///
    /// Scoped narrowly to `http_call`: this is **not** a general list of HTTP
    /// methods, and should not be reused to validate, say, inbound route
    /// definitions, which may legitimately accept `HEAD` or `OPTIONS`.
    pub const ALL: &'static [HttpMethod] = &[
        HttpMethod::Get,
        HttpMethod::Post,
        HttpMethod::Put,
        HttpMethod::Patch,
        HttpMethod::Delete,
    ];

    /// Canonical uppercase token, identical to the spelling `Deserialize`
    /// accepts — `from_value(json!(m.as_str()))` round-trips to `m` for every
    /// variant.
    pub const fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Delete => "DELETE",
        }
    }

    /// Whether re-sending the request is safe (RFC 9110 idempotency), so a
    /// caller may retry a timeout without risking a duplicate side effect.
    ///
    /// Written as an exhaustive `match` rather than a `matches!` so that adding
    /// a variant is a compile error here rather than a silent classification as
    /// non-idempotent.
    pub const fn is_idempotent(&self) -> bool {
        match self {
            HttpMethod::Get | HttpMethod::Put | HttpMethod::Delete => true,
            HttpMethod::Post | HttpMethod::Patch => false,
        }
    }
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

fn default_method() -> HttpMethod {
    HttpMethod::Get
}

fn default_timeout() -> u64 {
    30000
}

/// Configuration for the enrich integration function.
///
/// Enrichment calls an external service and merges the response into the message context.
///
/// Unknown keys are rejected, as for [`HttpCallConfig`]. Note that the
/// destination field here is `merge_path` and takes **no** alias — only
/// `HttpCallConfig::response_path` accepts `output`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrichConfig {
    /// Named connector reference
    pub connector: String,

    /// HTTP method for the enrichment call
    #[serde(default = "default_method")]
    pub method: HttpMethod,

    /// Static path
    #[serde(default)]
    pub path: Option<String>,

    /// JSONLogic expression to compute path dynamically
    #[serde(default)]
    pub path_logic: Option<Value>,

    /// Engine-internal: pre-compiled `path_logic`, populated by
    /// `LogicCompiler`. Read it through [`EnrichConfig::resolve_path`]. Not part
    /// of the stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub compiled_path_logic: Option<Arc<Logic>>,

    /// Dot-path where enrichment data is merged into the message context
    pub merge_path: String,

    /// Request timeout in milliseconds (default: 30000)
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,

    /// What to do on enrichment failure
    #[serde(default)]
    pub on_error: EnrichErrorAction,
}

impl HttpCallConfig {
    /// Resolve the request path: `path_logic` evaluated against the message
    /// context when compiled, otherwise the static `path`. `Ok(None)` when
    /// neither is set.
    ///
    /// A non-string result is coerced to its compact JSON form — `7` becomes
    /// `"7"`, `{"a":1}` becomes `"{\"a\":1}"` — because these values end up in a
    /// URL. See [`crate::TaskContext::eval_to_plain_string`].
    ///
    /// # Errors
    ///
    /// Propagates [`crate::DataflowError::LogicEvaluation`] if the expression
    /// fails to evaluate. It does **not** fall back to the static `path` on
    /// failure: a compiled expression that errors is a real problem, and silently
    /// substituting a different URL would hide it.
    pub fn resolve_path(&self, ctx: &TaskContext<'_>) -> Result<Option<String>> {
        match self.compiled_path_logic.as_deref() {
            Some(logic) => Ok(Some(ctx.eval_to_plain_string(logic)?)),
            None => Ok(self.path.clone()),
        }
    }

    /// Resolve the request body: `body_logic` when compiled, otherwise the
    /// static `body`. `Ok(None)` when neither is set.
    ///
    /// # Errors
    ///
    /// As [`Self::resolve_path`] — an evaluation failure propagates rather than
    /// falling back.
    pub fn resolve_body(&self, ctx: &TaskContext<'_>) -> Result<Option<Value>> {
        match self.compiled_body_logic.as_deref() {
            Some(logic) => Ok(Some(ctx.eval_json(logic)?)),
            None => Ok(self.body.clone()),
        }
    }
}

impl EnrichConfig {
    /// Resolve the enrichment path: `path_logic` when compiled, otherwise the
    /// static `path`. `Ok(None)` when neither is set.
    ///
    /// # Errors
    ///
    /// As [`HttpCallConfig::resolve_path`].
    pub fn resolve_path(&self, ctx: &TaskContext<'_>) -> Result<Option<String>> {
        match self.compiled_path_logic.as_deref() {
            Some(logic) => Ok(Some(ctx.eval_to_plain_string(logic)?)),
            None => Ok(self.path.clone()),
        }
    }
}

impl PublishKafkaConfig {
    /// Resolve the message key from `key_logic`. `Ok(None)` when it is not set —
    /// there is no static key field, so a `None` key is the caller's to interpret
    /// (Kafka treats a null key as "partition round-robin").
    ///
    /// Coerced to a plain string, matching [`HttpCallConfig::resolve_path`].
    ///
    /// # Errors
    ///
    /// As [`HttpCallConfig::resolve_path`].
    pub fn resolve_key(&self, ctx: &TaskContext<'_>) -> Result<Option<String>> {
        match self.compiled_key_logic.as_deref() {
            Some(logic) => Ok(Some(ctx.eval_to_plain_string(logic)?)),
            None => Ok(None),
        }
    }

    /// Resolve the message value from `value_logic`. `Ok(None)` when it is not
    /// set — the fallback (typically "serialize the whole message") stays the
    /// caller's policy.
    ///
    /// Returns `Option<Value>`, **not** `Option<String>`, deliberately: a
    /// producer that does `serde_json::to_string` unconditionally would put
    /// different bytes on the wire for a string-valued payload than
    /// [`Self::resolve_key`]'s plain-string coercion does. Keeping this as a
    /// `Value` leaves that choice where it belongs.
    ///
    /// # Errors
    ///
    /// As [`HttpCallConfig::resolve_path`].
    pub fn resolve_value(&self, ctx: &TaskContext<'_>) -> Result<Option<Value>> {
        match self.compiled_value_logic.as_deref() {
            Some(logic) => Ok(Some(ctx.eval_json(logic)?)),
            None => Ok(None),
        }
    }
}

/// What to do when enrichment fails
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EnrichErrorAction {
    /// Fail the task (default)
    #[default]
    Fail,
    /// Skip enrichment and continue
    Skip,
}

/// Configuration for the publish_kafka integration function.
///
/// The actual Kafka producer is provided by the service layer via AsyncFunctionHandler.
///
/// Unknown keys are rejected, as for [`HttpCallConfig`].
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishKafkaConfig {
    /// Named connector reference
    pub connector: String,

    /// Target topic name
    pub topic: String,

    /// JSONLogic expression to compute the message key
    #[serde(default)]
    pub key_logic: Option<Value>,

    /// Engine-internal: pre-compiled `key_logic`, populated by
    /// `LogicCompiler`. Read it through [`PublishKafkaConfig::resolve_key`]. Not
    /// part of the stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub compiled_key_logic: Option<Arc<Logic>>,

    /// JSONLogic expression to compute the message value
    #[serde(default)]
    pub value_logic: Option<Value>,

    /// Engine-internal: pre-compiled `value_logic`, populated by
    /// `LogicCompiler`. Read it through [`PublishKafkaConfig::resolve_value`].
    /// Not part of the stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub compiled_value_logic: Option<Arc<Logic>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn as_str_round_trips_through_deserialize() {
        // Ties `as_str` to `#[serde(rename_all = "UPPERCASE")]` rather than to a
        // guess: the canonical token must be exactly what Deserialize accepts.
        for m in HttpMethod::ALL {
            let parsed: HttpMethod = serde_json::from_value(json!(m.as_str()))
                .unwrap_or_else(|e| panic!("'{}' should deserialize: {e}", m.as_str()));
            assert_eq!(parsed, *m);
        }
    }

    #[test]
    fn lowercase_method_is_rejected() {
        // Makes the round-trip above a real constraint — it would also be
        // satisfied by a case-insensitive parse, which this rules out.
        assert!(serde_json::from_value::<HttpMethod>(json!("get")).is_err());
        assert!(serde_json::from_value::<HttpMethod>(json!("Post")).is_err());
        assert!(serde_json::from_value::<HttpMethod>(json!("HEAD")).is_err());
    }

    #[test]
    fn all_covers_every_variant() {
        // Adding a variant makes this match non-exhaustive — a compile error,
        // which is the reminder to extend `ALL`. `as_str` and `is_idempotent`
        // are exhaustive matches so the compiler already guards those; `ALL` is
        // hand-maintained and needs its own guard.
        for m in HttpMethod::ALL {
            match m {
                HttpMethod::Get
                | HttpMethod::Post
                | HttpMethod::Put
                | HttpMethod::Patch
                | HttpMethod::Delete => {}
            }
        }
        assert_eq!(HttpMethod::ALL.len(), 5);
    }

    #[test]
    fn is_idempotent_follows_rfc_9110() {
        assert!(HttpMethod::Get.is_idempotent());
        assert!(HttpMethod::Put.is_idempotent());
        assert!(HttpMethod::Delete.is_idempotent());
        assert!(!HttpMethod::Post.is_idempotent());
        assert!(!HttpMethod::Patch.is_idempotent());
    }

    #[test]
    fn display_matches_as_str() {
        for m in HttpMethod::ALL {
            assert_eq!(m.to_string(), m.as_str());
        }
    }

    #[test]
    fn default_method_is_get() {
        assert_eq!(HttpMethod::default(), HttpMethod::Get);
        // `HttpCallConfig` relies on this via `default_method`.
        assert_eq!(default_method(), HttpMethod::Get);
    }

    use crate::engine::message::Message;
    use crate::engine::utils::set_nested_value;
    use datavalue::OwnedDataValue;

    fn dv(v: serde_json::Value) -> OwnedDataValue {
        OwnedDataValue::from(&v)
    }

    fn engine() -> Arc<datalogic_rs::Engine> {
        Arc::new(
            datalogic_rs::Engine::builder()
                .with_templating(true)
                .build(),
        )
    }

    /// A message with a few readable values in `data`.
    fn fresh_message() -> Message {
        let mut m = Message::from_value(&json!({}));
        set_nested_value(&mut m.context, "data.id", dv(json!("abc")));
        set_nested_value(&mut m.context, "data.n", dv(json!(7)));
        set_nested_value(&mut m.context, "data.obj", dv(json!({"a": 1})));
        m
    }

    /// Stamp a compiled expression into a slot, mirroring what `LogicCompiler`
    /// does — so these tests exercise the same slot state the engine produces.
    fn compile(dl: &Arc<datalogic_rs::Engine>, logic: serde_json::Value) -> Option<Arc<Logic>> {
        Some(dl.compile_arc(&logic).expect("logic should compile"))
    }

    fn http_config() -> HttpCallConfig {
        serde_json::from_value(json!({ "connector": "c" })).unwrap()
    }

    fn enrich_config() -> EnrichConfig {
        serde_json::from_value(json!({ "connector": "c", "merge_path": "data.out" })).unwrap()
    }

    fn kafka_config() -> PublishKafkaConfig {
        serde_json::from_value(json!({ "connector": "c", "topic": "t" })).unwrap()
    }

    #[test]
    fn http_resolve_path_covers_all_four_slot_combinations() {
        let dl = engine();
        let mut m = fresh_message();
        let ctx = TaskContext::new(&mut m, &dl);

        // logic present -> evaluated string
        let mut cfg = http_config();
        cfg.compiled_path_logic = compile(&dl, json!({"var": "data.id"}));
        assert_eq!(cfg.resolve_path(&ctx).unwrap(), Some("abc".to_string()));

        // logic absent, static path present
        let mut cfg = http_config();
        cfg.path = Some("/static".to_string());
        assert_eq!(cfg.resolve_path(&ctx).unwrap(), Some("/static".to_string()));

        // both absent
        assert_eq!(http_config().resolve_path(&ctx).unwrap(), None);

        // both present -> logic wins
        let mut cfg = http_config();
        cfg.path = Some("/static".to_string());
        cfg.compiled_path_logic = compile(&dl, json!({"var": "data.id"}));
        assert_eq!(cfg.resolve_path(&ctx).unwrap(), Some("abc".to_string()));
    }

    #[test]
    fn http_resolve_body_covers_all_four_slot_combinations() {
        let dl = engine();
        let mut m = fresh_message();
        let ctx = TaskContext::new(&mut m, &dl);

        let mut cfg = http_config();
        cfg.compiled_body_logic = compile(&dl, json!({"var": "data.obj"}));
        assert_eq!(cfg.resolve_body(&ctx).unwrap(), Some(json!({"a": 1})));

        let mut cfg = http_config();
        cfg.body = Some(json!({"static": true}));
        assert_eq!(
            cfg.resolve_body(&ctx).unwrap(),
            Some(json!({"static": true}))
        );

        assert_eq!(http_config().resolve_body(&ctx).unwrap(), None);

        let mut cfg = http_config();
        cfg.body = Some(json!({"static": true}));
        cfg.compiled_body_logic = compile(&dl, json!({"var": "data.obj"}));
        assert_eq!(cfg.resolve_body(&ctx).unwrap(), Some(json!({"a": 1})));
    }

    #[test]
    fn enrich_resolve_path_covers_all_four_slot_combinations() {
        let dl = engine();
        let mut m = fresh_message();
        let ctx = TaskContext::new(&mut m, &dl);

        let mut cfg = enrich_config();
        cfg.compiled_path_logic = compile(&dl, json!({"var": "data.id"}));
        assert_eq!(cfg.resolve_path(&ctx).unwrap(), Some("abc".to_string()));

        let mut cfg = enrich_config();
        cfg.path = Some("/lookup".to_string());
        assert_eq!(cfg.resolve_path(&ctx).unwrap(), Some("/lookup".to_string()));

        assert_eq!(enrich_config().resolve_path(&ctx).unwrap(), None);

        let mut cfg = enrich_config();
        cfg.path = Some("/lookup".to_string());
        cfg.compiled_path_logic = compile(&dl, json!({"var": "data.id"}));
        assert_eq!(cfg.resolve_path(&ctx).unwrap(), Some("abc".to_string()));
    }

    #[test]
    fn kafka_resolve_key_and_value() {
        let dl = engine();
        let mut m = fresh_message();
        let ctx = TaskContext::new(&mut m, &dl);

        // No static fallback exists for either, so absent logic is Ok(None).
        let cfg = kafka_config();
        assert_eq!(cfg.resolve_key(&ctx).unwrap(), None);
        assert_eq!(cfg.resolve_value(&ctx).unwrap(), None);

        let mut cfg = kafka_config();
        cfg.compiled_key_logic = compile(&dl, json!({"var": "data.id"}));
        cfg.compiled_value_logic = compile(&dl, json!({"var": "data.obj"}));
        assert_eq!(cfg.resolve_key(&ctx).unwrap(), Some("abc".to_string()));
        // `resolve_value` returns a Value, not a String — a producer that
        // serializes unconditionally must not be forced through the key's
        // plain-string coercion.
        assert_eq!(cfg.resolve_value(&ctx).unwrap(), Some(json!({"a": 1})));
    }

    #[test]
    fn path_resolution_coerces_non_strings_for_the_url() {
        let dl = engine();
        let mut m = fresh_message();
        let ctx = TaskContext::new(&mut m, &dl);

        // A number becomes its digits, not "7" with quotes.
        let mut cfg = http_config();
        cfg.compiled_path_logic = compile(&dl, json!({"var": "data.n"}));
        assert_eq!(cfg.resolve_path(&ctx).unwrap(), Some("7".to_string()));

        // A container becomes compact JSON.
        let mut cfg = http_config();
        cfg.compiled_path_logic = compile(&dl, json!({"var": "data.obj"}));
        assert_eq!(
            cfg.resolve_path(&ctx).unwrap(),
            Some("{\"a\":1}".to_string())
        );
    }

    #[test]
    fn a_failing_expression_propagates_instead_of_falling_back() {
        let dl = engine();
        let mut m = fresh_message();
        let ctx = TaskContext::new(&mut m, &dl);

        // Static field is set, so a silent fallback would look like success.
        let mut cfg = http_config();
        cfg.path = Some("/static".to_string());
        cfg.compiled_path_logic = compile(&dl, json!({"+": ["abc", 1]}));

        match cfg.resolve_path(&ctx) {
            Err(crate::engine::error::DataflowError::LogicEvaluation(msg)) => {
                assert!(!msg.is_empty());
            }
            other => panic!("expected LogicEvaluation, got {other:?}"),
        }

        // Same for body.
        let mut cfg = http_config();
        cfg.body = Some(json!({"static": true}));
        cfg.compiled_body_logic = compile(&dl, json!({"+": ["abc", 1]}));
        assert!(cfg.resolve_body(&ctx).is_err());
    }
}
