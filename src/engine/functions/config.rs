use crate::engine::error::Result;
use crate::engine::functions::map::MapConfig;
use crate::engine::functions::validation::ValidationConfig;
use serde::Deserialize;
use serde_json::Value;

/// Enum containing all possible pre-parsed function configurations
#[derive(Debug, Clone, Deserialize)]
pub enum FunctionConfig {
    Map(MapConfig),
    Validation(ValidationConfig),
    /// For custom or unknown functions, store raw input
    Raw(Value),
}

impl FunctionConfig {
    /// Parse function configuration based on function name
    pub fn from_function_input(function_name: &str, input: &Value) -> Result<Self> {
        match function_name {
            "map" => Ok(FunctionConfig::Map(MapConfig::from_json(input)?)),
            "validation" | "validate" => Ok(FunctionConfig::Validation(
                ValidationConfig::from_json(input)?,
            )),
            _ => Ok(FunctionConfig::Raw(input.clone())),
        }
    }
}
