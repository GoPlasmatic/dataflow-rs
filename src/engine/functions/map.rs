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

        // Process each mapping
        for mapping in &self.mappings {
            debug!("Processing mapping to path: {}", mapping.path);

            // Get the compiled logic from cache with proper bounds checking
            let compiled_logic = match mapping.logic_index {
                Some(index) => {
                    // Ensure index is valid before accessing
                    if index >= logic_cache.len() {
                        error!(
                            "Map: Logic index {} out of bounds (cache size: {}) for mapping to {}",
                            index,
                            logic_cache.len(),
                            mapping.path
                        );
                        errors_encountered = true;
                        continue;
                    }
                    &logic_cache[index]
                }
                None => {
                    error!(
                        "Map: Logic not compiled (no index) for mapping to {}",
                        mapping.path
                    );
                    errors_encountered = true;
                    continue;
                }
            };

            // Create fresh evaluation context for each mapping to include previous changes
            // This ensures subsequent mappings can see changes made by previous mappings
            let eval_context = EvaluationContext::from_message(message);
            let eval_data = eval_context.to_arc_json();

            // Evaluate the transformation logic using DataLogic v4
            // DataLogic v4 is thread-safe with Arc<CompiledLogic>, no spawn_blocking needed
            let result = datalogic.evaluate(compiled_logic, eval_data);

            match result {
                Ok(transformed_value) => {
                    debug!(
                        "Map: Evaluated logic for path {} resulted in: {:?}",
                        mapping.path, transformed_value
                    );

                    // Get old value from the appropriate location
                    let old_value = if mapping.path.starts_with("temp_data") {
                        if mapping.path == "temp_data" {
                            Some(&message.temp_data)
                        } else if mapping.path.starts_with("temp_data.") {
                            get_nested_value(&message.temp_data, &mapping.path[10..])
                        } else {
                            None
                        }
                    } else {
                        let target_path = if mapping.path.starts_with("data.") {
                            &mapping.path[5..] // Skip "data."
                        } else {
                            &mapping.path
                        };
                        get_nested_value(&message.data, target_path)
                    };

                    let old_value_arc = Arc::new(old_value.cloned().unwrap_or(Value::Null));
                    // Create Arc once and share it
                    let new_value_arc = Arc::new(transformed_value);

                    changes.push(Change {
                        path: Arc::from(mapping.path.as_str()),
                        old_value: Arc::clone(&old_value_arc),
                        new_value: Arc::clone(&new_value_arc),
                    });

                    // Update the message data - extract from Arc to avoid double clone
                    // Handle different path prefixes to update the correct part of the message
                    if mapping.path.starts_with("temp_data") {
                        // Update temp_data field
                        let target_path = if mapping.path == "temp_data" {
                            "" // Root of temp_data
                        } else if mapping.path.starts_with("temp_data.") {
                            &mapping.path[10..] // Skip "temp_data."
                        } else {
                            &mapping.path
                        };

                        if target_path.is_empty() {
                            // Merge with existing temp_data instead of replacing
                            let new_value =
                                Arc::try_unwrap(new_value_arc).unwrap_or_else(|arc| (*arc).clone());
                            if let Value::Object(new_map) = new_value {
                                // If new value is an object, merge its fields
                                if let Value::Object(existing_map) = &mut message.temp_data {
                                    // Merge new fields into existing object
                                    for (key, value) in new_map {
                                        existing_map.insert(key, value);
                                    }
                                } else {
                                    // If existing is not an object, replace with new object
                                    message.temp_data = Value::Object(new_map);
                                }
                            } else {
                                // If new value is not an object, replace entirely
                                message.temp_data = new_value;
                            }
                        } else {
                            set_nested_value(
                                &mut message.temp_data,
                                target_path,
                                Arc::try_unwrap(new_value_arc).unwrap_or_else(|arc| (*arc).clone()),
                            );
                        }
                    } else {
                        // Update data field
                        let target_path = if mapping.path.starts_with("data.") {
                            &mapping.path[5..] // Skip "data."
                        } else if mapping.path == "data" {
                            "" // Root of data
                        } else {
                            &mapping.path
                        };

                        if target_path.is_empty() {
                            // Merge with existing data instead of replacing
                            let new_value =
                                Arc::try_unwrap(new_value_arc).unwrap_or_else(|arc| (*arc).clone());
                            if let Value::Object(new_map) = new_value {
                                // If new value is an object, merge its fields
                                if let Value::Object(existing_map) = &mut message.data {
                                    // Merge new fields into existing object
                                    for (key, value) in new_map {
                                        existing_map.insert(key, value);
                                    }
                                } else {
                                    // If existing is not an object, replace with new object
                                    message.data = Value::Object(new_map);
                                }
                            } else {
                                // If new value is not an object, replace entirely
                                message.data = new_value;
                            }
                        } else {
                            set_nested_value(
                                &mut message.data,
                                target_path,
                                Arc::try_unwrap(new_value_arc).unwrap_or_else(|arc| (*arc).clone()),
                            );
                        }
                    }
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
