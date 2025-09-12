use crate::engine::error::{DataflowError, Result};
use crate::engine::message::{Change, EvaluationContext, Message};
use crate::engine::utils::{get_nested_value, set_nested_value};
use datalogic_rs::{CompiledLogic, DataLogic};
use log::{debug, error};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

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

    /// Execute the map transformations using pre-compiled logic
    pub fn execute(
        &self,
        message: &mut Message,
        datalogic: &Arc<DataLogic>,
        logic_cache: &[Arc<CompiledLogic>],
    ) -> Result<(usize, Vec<Change>)> {
        let mut changes = Vec::new();
        let mut errors_encountered = false;

        // Use evaluation context to avoid repeated JSON creation
        let eval_context = EvaluationContext::from_message(message);
        let eval_data = eval_context.to_arc_json();

        // Process each mapping
        for mapping in &self.mappings {
            debug!("Processing mapping to path: {}", mapping.path);

            // Get the compiled logic from cache
            let compiled_logic = match mapping.logic_index {
                Some(index) if index < logic_cache.len() => &logic_cache[index],
                _ => {
                    error!("Map: Logic not compiled for mapping to {}", mapping.path);
                    errors_encountered = true;
                    continue;
                }
            };

            // Evaluate the transformation logic using DataLogic v4
            // DataLogic v4 is thread-safe with Arc<CompiledLogic>, no spawn_blocking needed
            let result = datalogic.evaluate(compiled_logic, Arc::clone(&eval_data));

            match result {
                Ok(transformed_value) => {
                    // Store the transformed value in the target path
                    let old_value = get_nested_value(&message.data, &mapping.path);
                    let old_value_arc = Arc::new(old_value.cloned().unwrap_or(Value::Null));
                    // Create Arc once and share it
                    let new_value_arc = Arc::new(transformed_value);

                    changes.push(Change {
                        path: Arc::from(mapping.path.as_str()),
                        old_value: Arc::clone(&old_value_arc),
                        new_value: Arc::clone(&new_value_arc),
                    });

                    // Update the message data - extract from Arc to avoid double clone
                    set_nested_value(
                        &mut message.data,
                        &mapping.path,
                        Arc::try_unwrap(new_value_arc).unwrap_or_else(|arc| (*arc).clone()),
                    );
                    debug!("Successfully mapped to path: {}", mapping.path);
                }
                Err(e) => {
                    error!(
                        "Map: Error evaluating logic for path {}: {:?}",
                        mapping.path, e
                    );
                    errors_encountered = true;
                }
            }
        }

        // Return appropriate status based on results
        let status = if errors_encountered { 500 } else { 200 };
        Ok((status, changes))
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
