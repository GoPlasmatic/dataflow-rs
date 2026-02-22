use crate::engine::functions::filter::FilterConfig;
use crate::engine::functions::integration::{EnrichConfig, HttpCallConfig, PublishKafkaConfig};
use crate::engine::functions::log::LogConfig;
use crate::engine::functions::map::MapConfig;
use crate::engine::functions::parse::ParseConfig;
use crate::engine::functions::publish::PublishConfig;
use crate::engine::functions::validation::ValidationConfig;
use serde::Deserialize;
use serde_json::Value;

/// Enum containing all possible function configurations
/// Uses internally tagged representation for clean deserialization
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum FunctionConfig {
    Map {
        name: MapName,
        input: MapConfig,
    },
    Validation {
        name: ValidationName,
        input: ValidationConfig,
    },
    ParseJson {
        name: ParseJsonName,
        input: ParseConfig,
    },
    ParseXml {
        name: ParseXmlName,
        input: ParseConfig,
    },
    PublishJson {
        name: PublishJsonName,
        input: PublishConfig,
    },
    PublishXml {
        name: PublishXmlName,
        input: PublishConfig,
    },
    Filter {
        name: FilterName,
        input: FilterConfig,
    },
    Log {
        name: LogName,
        input: LogConfig,
    },
    HttpCall {
        name: HttpCallName,
        input: HttpCallConfig,
    },
    Enrich {
        name: EnrichName,
        input: EnrichConfig,
    },
    PublishKafka {
        name: PublishKafkaName,
        input: PublishKafkaConfig,
    },
    /// For custom or unknown functions, store raw input
    Custom {
        name: String,
        input: Value,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MapName {
    Map,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ValidationName {
    Validation,
    Validate,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ParseJsonName {
    ParseJson,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ParseXmlName {
    ParseXml,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PublishJsonName {
    PublishJson,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PublishXmlName {
    PublishXml,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FilterName {
    Filter,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LogName {
    Log,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HttpCallName {
    HttpCall,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EnrichName {
    Enrich,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PublishKafkaName {
    PublishKafka,
}

impl FunctionConfig {
    /// Get the function name for this configuration
    pub fn function_name(&self) -> &str {
        match self {
            FunctionConfig::Map { .. } => "map",
            FunctionConfig::Validation { .. } => "validate",
            FunctionConfig::ParseJson { .. } => "parse_json",
            FunctionConfig::ParseXml { .. } => "parse_xml",
            FunctionConfig::PublishJson { .. } => "publish_json",
            FunctionConfig::PublishXml { .. } => "publish_xml",
            FunctionConfig::Filter { .. } => "filter",
            FunctionConfig::Log { .. } => "log",
            FunctionConfig::HttpCall { .. } => "http_call",
            FunctionConfig::Enrich { .. } => "enrich",
            FunctionConfig::PublishKafka { .. } => "publish_kafka",
            FunctionConfig::Custom { name, .. } => name,
        }
    }
}
