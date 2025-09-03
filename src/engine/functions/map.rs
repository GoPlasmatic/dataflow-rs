use crate::engine::error::{DataflowError, Result};
use serde::Deserialize;
use serde_json::Value;

/// Pre-parsed configuration for map function
#[derive(Debug, Clone, Deserialize)]
pub struct MapConfig {
    pub mappings: Vec<MapMapping>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MapMapping {
    pub path: String,
    pub logic: Value,
    #[serde(skip)]
    pub logic_index: Option<usize>,
}

impl MapConfig {
    pub fn from_json(input: &Value) -> Result<Self> {
        let mappings = input.get("mappings").ok_or_else(|| {
            DataflowError::Validation("Missing 'mappings' array in input".to_string())
        })?;

        let mappings_arr = mappings
            .as_array()
            .ok_or_else(|| DataflowError::Validation("'mappings' must be an array".to_string()))?;

        let mut parsed_mappings = Vec::new();

        for mapping in mappings_arr {
            let path = mapping
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| DataflowError::Validation("Missing 'path' in mapping".to_string()))?
                .to_string();

            let logic = mapping
                .get("logic")
                .ok_or_else(|| DataflowError::Validation("Missing 'logic' in mapping".to_string()))?
                .clone();

            parsed_mappings.push(MapMapping {
                path,
                logic,
                logic_index: None,
            });
        }

        Ok(MapConfig {
            mappings: parsed_mappings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_map_config_from_json() {
        let input = json!({
            "mappings": [
                {
                    "path": "data.field1",
                    "logic": {"var": "data.source"}
                },
                {
                    "path": "data.field2",
                    "logic": "static_value"
                }
            ]
        });

        let config = MapConfig::from_json(&input).unwrap();
        assert_eq!(config.mappings.len(), 2);
        assert_eq!(config.mappings[0].path, "data.field1");
        assert_eq!(config.mappings[1].path, "data.field2");
    }

    #[test]
    fn test_map_config_missing_mappings() {
        let input = json!({});
        let result = MapConfig::from_json(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_map_config_invalid_mappings() {
        let input = json!({
            "mappings": "not_an_array"
        });
        let result = MapConfig::from_json(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_map_config_missing_path() {
        let input = json!({
            "mappings": [
                {
                    "logic": {"var": "data.source"}
                }
            ]
        });
        let result = MapConfig::from_json(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_map_config_missing_logic() {
        let input = json!({
            "mappings": [
                {
                    "path": "data.field1"
                }
            ]
        });
        let result = MapConfig::from_json(&input);
        assert!(result.is_err());
    }
}
