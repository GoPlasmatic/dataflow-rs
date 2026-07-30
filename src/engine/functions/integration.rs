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

    /// Pre-compiled `path_logic`, populated by `LogicCompiler`.
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

    /// Pre-compiled `body_logic`, populated by `LogicCompiler`.
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

    /// Pre-compiled `path_logic`, populated by `LogicCompiler`.
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

    /// Pre-compiled `key_logic`, populated by `LogicCompiler`.
    #[serde(skip)]
    pub compiled_key_logic: Option<Arc<Logic>>,

    /// JSONLogic expression to compute the message value
    #[serde(default)]
    pub value_logic: Option<Value>,

    /// Pre-compiled `value_logic`, populated by `LogicCompiler`.
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
}
