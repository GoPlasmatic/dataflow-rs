use crate::engine::error::Result;
use crate::engine::functions::template::Template;
use crate::engine::task_context::TaskContext;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

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
    /// Named connector reference (resolved by the service layer).
    ///
    /// A JSONLogic expression, so one task can route by message content:
    /// `{"if": [{"var": "data.is_eu"}, "eu_gateway", "us_gateway"]}`. The
    /// static spelling `"payments_api"` is a literal and costs nothing. Read it
    /// through [`Self::resolve_connector`].
    pub connector: Template,

    /// HTTP method
    #[serde(default = "default_method")]
    pub method: HttpMethod,

    /// Request path, as a JSONLogic expression. Read it through
    /// [`Self::resolve_path`].
    ///
    /// Back-compat: before 3.9 this was a static `String` with a separate
    /// `path_logic` twin holding the expression, because a field could not be
    /// both. It can now, so the two collapsed and `path_logic` is kept as an
    /// alias so pre-3.9 definitions keep loading. Supplying both spellings is a
    /// `duplicate field` error, not a precedence rule.
    #[serde(default, alias = "path_logic")]
    pub path: Option<Template>,

    /// Request headers. Each value is a JSONLogic expression, so a header can
    /// carry a secret or a computed value:
    /// `{"Authorization": {"cat": ["Bearer ", {"secret": "api_token"}]}}`.
    ///
    /// A plain string value is a literal, so the static spelling is unchanged.
    /// Header *names* stay static — a name is not a computed value, and keeping
    /// them literal means no name can be swallowed as an operator.
    #[serde(default)]
    pub headers: HashMap<String, Template>,

    /// Request body, as a JSONLogic expression. Read it through
    /// [`Self::resolve_body`].
    ///
    /// Back-compat: as [`Self::path`], `body_logic` is kept as an alias for the
    /// pre-3.9 spelling.
    ///
    /// A literal object body needs its keys escaped when they collide with an
    /// operator name — `{"$cat": …}` for a body field actually called `cat` —
    /// because the engine evaluates in templating mode. See [`Template`].
    #[serde(default, alias = "body_logic")]
    pub body: Option<Template>,

    /// How the resolved body becomes request bytes (e.g. `"json"`, `"form"`,
    /// `"text"`).
    ///
    /// The value is **data, not API surface**: this crate does not validate or
    /// interpret it — the service layer owns the value table, its default for
    /// `None`, and the encoding behaviour. That split is deliberate: field
    /// *names* are fixed here by `deny_unknown_fields`, but a service layer can
    /// grow new *values* (say, `"multipart"`) without touching this crate.
    #[serde(default)]
    pub body_format: Option<Template>,

    /// JSONPath/dot-path to extract from response and merge into context.
    ///
    /// `output` is accepted as an alias, so a service layer can present one
    /// destination-field name across its whole function catalogue. Supplying
    /// both keys is a `duplicate field` error rather than a precedence rule.
    #[serde(default, alias = "output")]
    pub response_path: Option<Template>,

    /// How response bytes become the captured value (e.g. `"json"`, `"text"`).
    ///
    /// As [`Self::body_format`]: data, not API surface — uninterpreted by this
    /// crate, owned by the service layer.
    #[serde(default)]
    pub response_format: Option<Template>,

    /// Request timeout in milliseconds (default: 30000). Read it through
    /// [`Self::resolve_timeout_ms`].
    #[serde(default = "default_timeout")]
    pub timeout_ms: Template,
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
    pub const ALL: &'static [Self] = &[Self::Get, Self::Post, Self::Put, Self::Patch, Self::Delete];

    /// Canonical uppercase token, identical to the spelling `Deserialize`
    /// accepts — `from_value(json!(m.as_str()))` round-trips to `m` for every
    /// variant.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
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
            Self::Get | Self::Put | Self::Delete => true,
            Self::Post | Self::Patch => false,
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

fn default_timeout() -> Template {
    Template::from(Value::from(DEFAULT_TIMEOUT_MS))
}

/// The `timeout_ms` default, as a number rather than a `Template`, for hosts
/// that want the value without resolving an expression.
pub const DEFAULT_TIMEOUT_MS: u64 = 30000;

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
    /// Named connector reference. A JSONLogic expression, as
    /// [`HttpCallConfig::connector`]; read it through
    /// [`Self::resolve_connector`].
    pub connector: Template,

    /// HTTP method for the enrichment call
    #[serde(default = "default_method")]
    pub method: HttpMethod,

    /// Enrichment path, as a JSONLogic expression. Read it through
    /// [`Self::resolve_path`].
    ///
    /// Back-compat: `path_logic` is kept as an alias for the pre-3.9 spelling,
    /// as on [`HttpCallConfig::path`].
    #[serde(default, alias = "path_logic")]
    pub path: Option<Template>,

    /// Dot-path where enrichment data is merged into the message context, as a
    /// JSONLogic expression. Read it through [`Self::resolve_merge_path`].
    pub merge_path: Template,

    /// Request timeout in milliseconds (default: 30000). Read it through
    /// [`Self::resolve_timeout_ms`].
    #[serde(default = "default_timeout")]
    pub timeout_ms: Template,

    /// What to do on enrichment failure
    #[serde(default)]
    pub on_error: EnrichErrorAction,
}

/// Shared shape behind the optional `resolve_*` methods below: evaluate the
/// expression to a plain string when the field is set, `Ok(None)` when it is
/// not.
///
/// A non-string result is coerced to its compact JSON form — `7` becomes `"7"`,
/// `{"a":1}` becomes `"{\"a\":1}"` — because these values end up in a URL or a
/// partition key. See [`crate::TaskContext::eval_to_plain_string`].
///
/// # Errors
///
/// Propagates [`crate::DataflowError::LogicEvaluation`] if the expression fails
/// to evaluate. There is deliberately no fallback value: a compiled expression
/// that errors is a real problem, and silently substituting something else
/// would hide it.
fn resolve_opt_string(field: &Option<Template>, ctx: &TaskContext<'_>) -> Result<Option<String>> {
    field.as_ref().map(|t| t.resolve_string(ctx)).transpose()
}

/// As `resolve_opt_string`, but evaluated into a [`Value`] rather than
/// coerced to a string — for fields (like a request body) where the caller
/// wants the JSON shape, not a stringified one.
///
/// # Errors
///
/// As `resolve_opt_string`.
fn resolve_opt_value(field: &Option<Template>, ctx: &TaskContext<'_>) -> Result<Option<Value>> {
    field.as_ref().map(|t| t.eval_into(ctx)).transpose()
}

impl HttpCallConfig {
    /// Resolve the connector name for this message.
    ///
    /// # Errors
    ///
    /// As `resolve_opt_string`.
    pub fn resolve_connector(&self, ctx: &TaskContext<'_>) -> Result<String> {
        self.connector.resolve_string(ctx)
    }

    /// Resolve the request path. `Ok(None)` when no path is configured.
    ///
    /// # Errors
    ///
    /// As `resolve_opt_string`.
    pub fn resolve_path(&self, ctx: &TaskContext<'_>) -> Result<Option<String>> {
        resolve_opt_string(&self.path, ctx)
    }

    /// Resolve every request header for this message.
    ///
    /// Header names are copied through unchanged; only the values are
    /// expressions. A value that fails to evaluate fails the whole call rather
    /// than sending the request with that header missing — a dropped
    /// `Authorization` would otherwise surface as a confusing 401.
    ///
    /// # Errors
    ///
    /// As `resolve_opt_string`, for the first header value that fails.
    pub fn resolve_headers(&self, ctx: &TaskContext<'_>) -> Result<HashMap<String, String>> {
        self.headers
            .iter()
            .map(|(name, value)| Ok((name.clone(), value.resolve_string(ctx)?)))
            .collect()
    }

    /// Resolve the request body. `Ok(None)` when no body is configured.
    ///
    /// # Errors
    ///
    /// As [`Self::resolve_path`].
    pub fn resolve_body(&self, ctx: &TaskContext<'_>) -> Result<Option<Value>> {
        resolve_opt_value(&self.body, ctx)
    }

    /// Resolve the body encoding name. `Ok(None)` when unset — what that means
    /// is the service layer's call.
    ///
    /// # Errors
    ///
    /// As `resolve_opt_string`.
    pub fn resolve_body_format(&self, ctx: &TaskContext<'_>) -> Result<Option<String>> {
        resolve_opt_string(&self.body_format, ctx)
    }

    /// Resolve the dot-path the response is captured to. `Ok(None)` when unset.
    ///
    /// # Errors
    ///
    /// As `resolve_opt_string`.
    pub fn resolve_response_path(&self, ctx: &TaskContext<'_>) -> Result<Option<String>> {
        resolve_opt_string(&self.response_path, ctx)
    }

    /// Resolve the response decoding name. `Ok(None)` when unset.
    ///
    /// # Errors
    ///
    /// As `resolve_opt_string`.
    pub fn resolve_response_format(&self, ctx: &TaskContext<'_>) -> Result<Option<String>> {
        resolve_opt_string(&self.response_format, ctx)
    }

    /// Resolve the request timeout in milliseconds.
    ///
    /// # Errors
    ///
    /// As [`Template::resolve_u64`] — a non-numeric result is a configuration
    /// error, not something to silently default.
    pub fn resolve_timeout_ms(&self, ctx: &TaskContext<'_>) -> Result<u64> {
        self.timeout_ms.resolve_u64(ctx, "http_call timeout_ms")
    }
}

impl EnrichConfig {
    /// Resolve the connector name for this message.
    ///
    /// # Errors
    ///
    /// As `resolve_opt_string`.
    pub fn resolve_connector(&self, ctx: &TaskContext<'_>) -> Result<String> {
        self.connector.resolve_string(ctx)
    }

    /// Resolve the enrichment path. `Ok(None)` when no path is configured.
    ///
    /// # Errors
    ///
    /// As [`HttpCallConfig::resolve_path`].
    pub fn resolve_path(&self, ctx: &TaskContext<'_>) -> Result<Option<String>> {
        resolve_opt_string(&self.path, ctx)
    }

    /// Resolve the dot-path enrichment data is merged into.
    ///
    /// # Errors
    ///
    /// As `resolve_opt_string`.
    pub fn resolve_merge_path(&self, ctx: &TaskContext<'_>) -> Result<String> {
        self.merge_path.resolve_string(ctx)
    }

    /// Resolve the request timeout in milliseconds.
    ///
    /// # Errors
    ///
    /// As [`HttpCallConfig::resolve_timeout_ms`].
    pub fn resolve_timeout_ms(&self, ctx: &TaskContext<'_>) -> Result<u64> {
        self.timeout_ms.resolve_u64(ctx, "enrich timeout_ms")
    }
}

impl PublishKafkaConfig {
    /// Resolve the connector name for this message.
    ///
    /// # Errors
    ///
    /// As `resolve_opt_string`.
    pub fn resolve_connector(&self, ctx: &TaskContext<'_>) -> Result<String> {
        self.connector.resolve_string(ctx)
    }

    /// Resolve the target topic for this message.
    ///
    /// # Errors
    ///
    /// As `resolve_opt_string`.
    pub fn resolve_topic(&self, ctx: &TaskContext<'_>) -> Result<String> {
        self.topic.resolve_string(ctx)
    }

    /// Resolve the message key. `Ok(None)` when it is not set — Kafka treats a
    /// null key as "partition round-robin", so a `None` key is the caller's to
    /// interpret.
    ///
    /// Coerced to a plain string, matching [`HttpCallConfig::resolve_path`].
    ///
    /// # Errors
    ///
    /// As [`HttpCallConfig::resolve_path`].
    pub fn resolve_key(&self, ctx: &TaskContext<'_>) -> Result<Option<String>> {
        resolve_opt_string(&self.key, ctx)
    }

    /// Resolve the message value. `Ok(None)` when it is not set — the fallback
    /// (typically "serialize the whole message") stays the caller's policy.
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
        resolve_opt_value(&self.value, ctx)
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
    /// Named connector reference. A JSONLogic expression, as
    /// [`HttpCallConfig::connector`]; read it through
    /// [`Self::resolve_connector`].
    pub connector: Template,

    /// Target topic name, as a JSONLogic expression — so one task can route by
    /// message content, which is the ordinary Kafka pattern:
    /// `{"cat": ["orders.", {"var": "data.region"}]}`. Read it through
    /// [`Self::resolve_topic`].
    pub topic: Template,

    /// The message key, as a JSONLogic expression. Read it through
    /// [`Self::resolve_key`].
    ///
    /// Back-compat: `key_logic` is kept as an alias for the pre-3.9 spelling.
    /// This field was always an expression — the rename is for consistency with
    /// the other configs, where the `_logic` suffix marked the twin of a static
    /// field that no longer exists.
    #[serde(default, alias = "key_logic")]
    pub key: Option<Template>,

    /// The message value, as a JSONLogic expression. Read it through
    /// [`Self::resolve_value`].
    ///
    /// Back-compat: `value_logic` is kept as an alias, as for [`Self::key`].
    #[serde(default, alias = "value_logic")]
    pub value: Option<Template>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::compiler::datalogic_engine_builder;
    use crate::engine::functions::template::TemplateCompiler;
    use crate::engine::message::Message;
    use crate::engine::utils::set_nested_value;
    use datavalue::OwnedDataValue;
    use serde_json::json;
    use std::sync::Arc;

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

    fn engine() -> Arc<datalogic_rs::Engine> {
        Arc::new(datalogic_engine_builder().build())
    }

    fn dv(v: serde_json::Value) -> OwnedDataValue {
        OwnedDataValue::from(&v)
    }

    /// A message with a few readable values in `data`.
    fn fresh_message() -> Message {
        let mut m = Message::from_value(&json!({}));
        set_nested_value(&mut m.context, "data.id", dv(json!("abc")));
        set_nested_value(&mut m.context, "data.n", dv(json!(7)));
        set_nested_value(&mut m.context, "data.obj", dv(json!({"a": 1})));
        m
    }

    /// Parse a config and compile every parameter, exactly as `LogicCompiler`
    /// does — so these tests exercise the state the engine produces.
    fn http_config(extra: serde_json::Value) -> HttpCallConfig {
        let mut base = json!({ "connector": "c" });
        let obj = base.as_object_mut().unwrap();
        for (k, v) in extra.as_object().unwrap() {
            obj.insert(k.clone(), v.clone());
        }
        let mut cfg: HttpCallConfig = serde_json::from_value(base).expect("config should parse");
        let c = TemplateCompiler::new(engine());
        cfg.connector.compile(&c, "connector").unwrap();
        cfg.timeout_ms.compile(&c, "timeout_ms").unwrap();
        for v in cfg.headers.values_mut() {
            v.compile(&c, "header").unwrap();
        }
        for t in [
            &mut cfg.path,
            &mut cfg.body,
            &mut cfg.body_format,
            &mut cfg.response_path,
            &mut cfg.response_format,
        ]
        .into_iter()
        .flatten()
        {
            t.compile(&c, "field").unwrap();
        }
        cfg
    }

    #[test]
    fn format_fields_default_to_none() {
        // Every pre-existing config deserializes unchanged: absent format
        // fields are `None`, and what `None` means is the service layer's call.
        let cfg = http_config(json!({}));
        assert!(cfg.body_format.is_none());
        assert!(cfg.response_format.is_none());
    }

    #[test]
    fn misspelled_format_field_is_rejected() {
        // `deny_unknown_fields` covers the new names too: a typo fails at
        // parse time instead of silently sending the default encoding.
        let err = serde_json::from_value::<HttpCallConfig>(json!({
            "connector": "c",
            "body_fromat": "form",
        }))
        .expect_err("unknown field must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("body_fromat"), "{msg}");
        // The expected-field list in the error names both format fields — the
        // docs quote this text (integrations.md "Unknown fields are rejected").
        assert!(msg.contains("`body_format`"), "{msg}");
        assert!(msg.contains("`response_format`"), "{msg}");
    }

    #[test]
    fn the_pre_39_logic_spellings_still_deserialize() {
        // The back-compat aliases. A workflow written against 3.8 must load
        // unchanged — this is the whole reason the aliases exist.
        let http: HttpCallConfig = serde_json::from_value(json!({
            "connector": "c",
            "path_logic": {"var": "data.id"},
            "body_logic": {"var": "data.obj"},
        }))
        .expect("pre-3.9 http_call spelling must still load");
        assert_eq!(http.path.unwrap().as_json(), &json!({"var": "data.id"}));
        assert_eq!(http.body.unwrap().as_json(), &json!({"var": "data.obj"}));

        let enrich: EnrichConfig = serde_json::from_value(json!({
            "connector": "c",
            "merge_path": "data.out",
            "path_logic": {"var": "data.id"},
        }))
        .expect("pre-3.9 enrich spelling must still load");
        assert_eq!(enrich.path.unwrap().as_json(), &json!({"var": "data.id"}));

        let kafka: PublishKafkaConfig = serde_json::from_value(json!({
            "connector": "c",
            "topic": "t",
            "key_logic": {"var": "data.id"},
            "value_logic": {"var": "data.obj"},
        }))
        .expect("pre-3.9 publish_kafka spelling must still load");
        assert_eq!(kafka.key.unwrap().as_json(), &json!({"var": "data.id"}));
        assert_eq!(kafka.value.unwrap().as_json(), &json!({"var": "data.obj"}));
    }

    #[test]
    fn supplying_both_spellings_is_a_duplicate_field_error() {
        // Not a precedence rule — the same contract `response_path`/`output`
        // has always had. An author who set both did not mean one to win.
        let err = serde_json::from_value::<HttpCallConfig>(json!({
            "connector": "c",
            "path": "/a",
            "path_logic": {"var": "data.id"},
        }))
        .expect_err("both spellings must be rejected");
        assert!(err.to_string().contains("duplicate field"), "{err}");
    }

    #[test]
    fn a_static_config_resolves_to_exactly_what_was_authored() {
        // The static spelling of every parameter is a literal, so a 3.8-era
        // config behaves identically — and folds, so it costs no evaluation.
        let dl = engine();
        let mut m = fresh_message();
        let ctx = TaskContext::new(&mut m, &dl);

        let cfg = http_config(json!({
            "path": "/static",
            "headers": {"X-Env": "prod"},
            "body_format": "json",
            "response_path": "data.out",
        }));
        assert_eq!(cfg.resolve_connector(&ctx).unwrap(), "c");
        assert_eq!(cfg.resolve_path(&ctx).unwrap().as_deref(), Some("/static"));
        assert_eq!(cfg.resolve_headers(&ctx).unwrap()["X-Env"], "prod");
        assert_eq!(
            cfg.resolve_body_format(&ctx).unwrap().as_deref(),
            Some("json")
        );
        assert_eq!(
            cfg.resolve_response_path(&ctx).unwrap().as_deref(),
            Some("data.out")
        );
        assert_eq!(cfg.resolve_timeout_ms(&ctx).unwrap(), DEFAULT_TIMEOUT_MS);

        assert!(cfg.connector.is_constant(), "a literal connector must fold");
        assert!(
            cfg.timeout_ms.is_constant(),
            "the default timeout must fold"
        );
    }

    #[test]
    fn every_parameter_can_be_computed_from_the_message() {
        let dl = engine();
        let mut m = fresh_message();
        let ctx = TaskContext::new(&mut m, &dl);

        let cfg = http_config(json!({
            "connector": {"cat": ["gw_", {"var": "data.id"}]},
            "path": {"cat": ["/orders/", {"var": "data.id"}]},
            "headers": {"X-Request-Id": {"var": "data.id"}},
            "body": {"var": "data.obj"},
            "timeout_ms": {"var": "data.n"},
        }));

        assert_eq!(cfg.resolve_connector(&ctx).unwrap(), "gw_abc");
        assert_eq!(
            cfg.resolve_path(&ctx).unwrap().as_deref(),
            Some("/orders/abc")
        );
        assert_eq!(cfg.resolve_headers(&ctx).unwrap()["X-Request-Id"], "abc");
        assert_eq!(cfg.resolve_body(&ctx).unwrap(), Some(json!({"a": 1})));
        assert_eq!(cfg.resolve_timeout_ms(&ctx).unwrap(), 7);
    }

    #[test]
    fn an_escaped_body_key_is_sent_as_data_not_evaluated() {
        // The reason `body` and `body_logic` could collapse at all: a literal
        // body field named after an operator is now expressible.
        let dl = engine();
        let mut m = fresh_message();
        let ctx = TaskContext::new(&mut m, &dl);

        let cfg = http_config(json!({ "body": {"$cat": ["a", "b"]} }));
        assert_eq!(
            cfg.resolve_body(&ctx).unwrap(),
            Some(json!({"cat": ["a", "b"]}))
        );

        // Unescaped, the same key is still the operator.
        let cfg = http_config(json!({ "body": {"cat": ["a", "b"]} }));
        assert_eq!(cfg.resolve_body(&ctx).unwrap(), Some(json!("ab")));
    }

    #[test]
    fn header_values_are_coerced_to_plain_strings() {
        // A header carries bytes, not JSON — a number must not arrive quoted.
        let dl = engine();
        let mut m = fresh_message();
        let ctx = TaskContext::new(&mut m, &dl);

        let cfg = http_config(json!({ "headers": {"X-Count": {"var": "data.n"}} }));
        assert_eq!(cfg.resolve_headers(&ctx).unwrap()["X-Count"], "7");
    }

    #[test]
    fn a_failing_header_fails_the_call_rather_than_being_dropped() {
        // A silently missing `Authorization` surfaces as a confusing 401.
        let dl = engine();
        let mut m = fresh_message();
        let ctx = TaskContext::new(&mut m, &dl);

        let cfg = http_config(json!({ "headers": {"X-Bad": {"+": ["abc", 1]}} }));
        assert!(cfg.resolve_headers(&ctx).is_err());
    }

    #[test]
    fn path_resolution_coerces_non_strings_for_the_url() {
        let dl = engine();
        let mut m = fresh_message();
        let ctx = TaskContext::new(&mut m, &dl);

        // A number becomes its digits, not "7" with quotes.
        let cfg = http_config(json!({ "path": {"var": "data.n"} }));
        assert_eq!(cfg.resolve_path(&ctx).unwrap(), Some("7".to_string()));

        // A container becomes compact JSON.
        let cfg = http_config(json!({ "path": {"var": "data.obj"} }));
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

        let cfg = http_config(json!({ "path": {"+": ["abc", 1]} }));
        match cfg.resolve_path(&ctx) {
            Err(crate::engine::error::DataflowError::LogicEvaluation(msg)) => {
                assert!(!msg.is_empty());
            }
            other => panic!("expected LogicEvaluation, got {other:?}"),
        }

        let cfg = http_config(json!({ "body": {"+": ["abc", 1]} }));
        assert!(cfg.resolve_body(&ctx).is_err());
    }

    #[test]
    fn a_non_numeric_timeout_is_a_configuration_error() {
        // `resolve_u64` refuses rather than defaulting: a `timeout_ms` whose
        // path is missing resolves to null, and silently becoming 0 would make
        // every request fail instantly with no explanation.
        let dl = engine();
        let mut m = fresh_message();
        let ctx = TaskContext::new(&mut m, &dl);

        let cfg = http_config(json!({ "timeout_ms": {"var": "data.nope"} }));
        let err = cfg
            .resolve_timeout_ms(&ctx)
            .expect_err("a null timeout must be rejected");
        assert!(err.to_string().contains("timeout_ms"), "{err}");
    }

    #[test]
    fn enrich_and_kafka_resolve_their_own_parameters() {
        let dl = engine();
        let c = TemplateCompiler::new(engine());
        let mut m = fresh_message();
        let ctx = TaskContext::new(&mut m, &dl);

        let mut enrich: EnrichConfig = serde_json::from_value(json!({
            "connector": "lookup",
            "path": {"cat": ["/users/", {"var": "data.id"}]},
            "merge_path": {"cat": ["data.users.", {"var": "data.id"}]},
        }))
        .unwrap();
        enrich.connector.compile(&c, "connector").unwrap();
        enrich.merge_path.compile(&c, "merge_path").unwrap();
        enrich.timeout_ms.compile(&c, "timeout_ms").unwrap();
        enrich.path.as_mut().unwrap().compile(&c, "path").unwrap();

        assert_eq!(enrich.resolve_connector(&ctx).unwrap(), "lookup");
        assert_eq!(
            enrich.resolve_path(&ctx).unwrap().as_deref(),
            Some("/users/abc")
        );
        assert_eq!(enrich.resolve_merge_path(&ctx).unwrap(), "data.users.abc");
        assert_eq!(enrich.resolve_timeout_ms(&ctx).unwrap(), DEFAULT_TIMEOUT_MS);

        // Dynamic topic routing is the ordinary Kafka pattern and was
        // impossible before 3.9.
        let mut kafka: PublishKafkaConfig = serde_json::from_value(json!({
            "connector": "bus",
            "topic": {"cat": ["orders.", {"var": "data.id"}]},
            "key": {"var": "data.id"},
            "value": {"var": "data.obj"},
        }))
        .unwrap();
        kafka.connector.compile(&c, "connector").unwrap();
        kafka.topic.compile(&c, "topic").unwrap();
        kafka.key.as_mut().unwrap().compile(&c, "key").unwrap();
        kafka.value.as_mut().unwrap().compile(&c, "value").unwrap();

        assert_eq!(kafka.resolve_connector(&ctx).unwrap(), "bus");
        assert_eq!(kafka.resolve_topic(&ctx).unwrap(), "orders.abc");
        assert_eq!(kafka.resolve_key(&ctx).unwrap().as_deref(), Some("abc"));
        // A `Value`, not a `String` — a producer that serializes
        // unconditionally must not be forced through the key's coercion.
        assert_eq!(kafka.resolve_value(&ctx).unwrap(), Some(json!({"a": 1})));
    }

    #[test]
    fn absent_optional_fields_resolve_to_none() {
        let dl = engine();
        let mut m = fresh_message();
        let ctx = TaskContext::new(&mut m, &dl);

        let cfg = http_config(json!({}));
        assert_eq!(cfg.resolve_path(&ctx).unwrap(), None);
        assert_eq!(cfg.resolve_body(&ctx).unwrap(), None);
        assert_eq!(cfg.resolve_body_format(&ctx).unwrap(), None);
        assert_eq!(cfg.resolve_response_path(&ctx).unwrap(), None);
        assert_eq!(cfg.resolve_response_format(&ctx).unwrap(), None);
        assert!(cfg.resolve_headers(&ctx).unwrap().is_empty());
    }
}
