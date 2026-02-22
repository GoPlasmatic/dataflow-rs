use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

/// Configuration for the http_call integration function.
///
/// The actual HTTP implementation is provided by the service layer via AsyncFunctionHandler.
/// This struct provides typed config validation and pre-compilation of JSONLogic expressions.
#[derive(Debug, Clone, Deserialize)]
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

    /// Cache index for compiled path_logic
    #[serde(skip)]
    pub path_logic_index: Option<usize>,

    /// Static headers
    #[serde(default)]
    pub headers: HashMap<String, String>,

    /// Static request body
    #[serde(default)]
    pub body: Option<Value>,

    /// JSONLogic expression to compute body dynamically
    #[serde(default)]
    pub body_logic: Option<Value>,

    /// Cache index for compiled body_logic
    #[serde(skip)]
    pub body_logic_index: Option<usize>,

    /// JSONPath/dot-path to extract from response and merge into context
    #[serde(default)]
    pub response_path: Option<String>,

    /// Request timeout in milliseconds (default: 30000)
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

/// HTTP methods supported by http_call
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    #[default]
    Get,
    Post,
    Put,
    Patch,
    Delete,
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
#[derive(Debug, Clone, Deserialize)]
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

    /// Cache index for compiled path_logic
    #[serde(skip)]
    pub path_logic_index: Option<usize>,

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
#[derive(Debug, Clone, Deserialize)]
pub struct PublishKafkaConfig {
    /// Named connector reference
    pub connector: String,

    /// Target topic name
    pub topic: String,

    /// JSONLogic expression to compute the message key
    #[serde(default)]
    pub key_logic: Option<Value>,

    /// Cache index for compiled key_logic
    #[serde(skip)]
    pub key_logic_index: Option<usize>,

    /// JSONLogic expression to compute the message value
    #[serde(default)]
    pub value_logic: Option<Value>,

    /// Cache index for compiled value_logic
    #[serde(skip)]
    pub value_logic_index: Option<usize>,
}
