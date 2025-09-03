use crate::engine::functions::map::MapConfig;
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

impl FunctionConfig {
    /// Get the function name for this configuration
    pub fn function_name(&self) -> &str {
        match self {
            FunctionConfig::Map { .. } => "map",
            FunctionConfig::Validation { .. } => "validate",
            FunctionConfig::Custom { name, .. } => name,
        }
    }
}
