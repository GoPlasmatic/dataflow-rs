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

        debug!("Map: Executing {} mappings", self.mappings.len());

        // Create initial evaluation context - we'll reuse and update this Arc
        let mut eval_data: Option<Arc<Value>> = None;
        let mut message_modified = false;

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

            // Only recreate evaluation context if message was modified or this is the first iteration
            // This ensures subsequent mappings can see changes while avoiding unnecessary clones
            if eval_data.is_none() || message_modified {
                let eval_context = EvaluationContext::from_message(message);
                eval_data = Some(eval_context.to_arc_json());
                message_modified = false;
            }

            // Reuse the Arc by cloning the reference, not the data
            let current_eval_data = Arc::clone(eval_data.as_ref().unwrap());

            // Evaluate the transformation logic using DataLogic v4
            // DataLogic v4 is thread-safe with Arc<CompiledLogic>, no spawn_blocking needed
            let result = datalogic.evaluate(compiled_logic, current_eval_data);

            match result {
                Ok(transformed_value) => {
                    debug!(
                        "Map: Evaluated logic for path {} resulted in: {:?}",
                        mapping.path, transformed_value
                    );

                    // Skip mapping if the result is null
                    if transformed_value.is_null() {
                        debug!(
                            "Map: Skipping mapping for path {} as result is null",
                            mapping.path
                        );
                        continue;
                    }

                    // Get old value from the appropriate location
                    let old_value = if mapping.path.starts_with("temp_data") {
                        if mapping.path == "temp_data" {
                            Some(&message.temp_data)
                        } else if mapping.path.starts_with("temp_data.") {
                            get_nested_value(&message.temp_data, &mapping.path[10..])
                        } else {
                            None
                        }
                    } else if mapping.path.starts_with("metadata") {
                        if mapping.path == "metadata" {
                            Some(&message.metadata)
                        } else if mapping.path.starts_with("metadata.") {
                            get_nested_value(&message.metadata, &mapping.path[9..])
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
                    let new_value_arc = Arc::new(transformed_value.clone());

                    debug!(
                        "Recording change for path '{}': old={:?}, new={:?}",
                        mapping.path, old_value_arc, new_value_arc
                    );
                    changes.push(Change {
                        path: Arc::from(mapping.path.as_str()),
                        old_value: old_value_arc,
                        new_value: Arc::clone(&new_value_arc),
                    });

                    // Update the message data directly with the transformed value
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
                            if let Value::Object(new_map) = transformed_value {
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
                                message.temp_data = transformed_value;
                            }
                        } else {
                            set_nested_value(
                                &mut message.temp_data,
                                target_path,
                                transformed_value,
                            );
                        }
                        message_modified = true;  // Mark that we've modified the message
                    } else if mapping.path.starts_with("metadata") {
                        // Update metadata field
                        let target_path = if mapping.path == "metadata" {
                            "" // Root of metadata
                        } else if mapping.path.starts_with("metadata.") {
                            &mapping.path[9..] // Skip "metadata."
                        } else {
                            &mapping.path
                        };

                        if target_path.is_empty() {
                            // Merge with existing metadata instead of replacing
                            if let Value::Object(new_map) = transformed_value {
                                // If new value is an object, merge its fields
                                if let Value::Object(existing_map) = &mut message.metadata {
                                    // Merge new fields into existing object
                                    for (key, value) in new_map {
                                        existing_map.insert(key, value);
                                    }
                                } else {
                                    // If existing is not an object, replace with new object
                                    message.metadata = Value::Object(new_map);
                                }
                            } else {
                                // If new value is not an object, replace entirely
                                message.metadata = transformed_value;
                            }
                        } else {
                            set_nested_value(
                                &mut message.metadata,
                                target_path,
                                transformed_value,
                            );
                        }
                        message_modified = true;  // Mark that we've modified the message
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
                            if let Value::Object(new_map) = transformed_value {
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
                                message.data = transformed_value;
                            }
                        } else {
                            set_nested_value(
                                &mut message.data,
                                target_path,
                                transformed_value,
                            );
                        }
                        message_modified = true;  // Mark that we've modified the message
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
    use crate::engine::message::Message;
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

    #[test]
    fn test_map_metadata_assignment() {
        // Test that metadata field assignments work correctly
        let datalogic = Arc::new(DataLogic::with_preserve_structure());

        // Create test message
        let mut message = Message::new(Arc::new(json!({})));
        message.data = json!({
            "SwiftMT": {
                "message_type": "103"
            }
        });

        // Create mapping config that assigns from data to metadata
        let config = MapConfig {
            mappings: vec![MapMapping {
                path: "metadata.SwiftMT.message_type".to_string(),
                logic: json!({"var": "data.SwiftMT.message_type"}),
                logic_index: Some(0),
            }],
        };

        // Compile the logic
        let logic_cache = vec![datalogic.compile(&config.mappings[0].logic).unwrap()];

        // Execute the mapping
        let result = config.execute(&mut message, &datalogic, &logic_cache);
        assert!(result.is_ok());

        let (status, changes) = result.unwrap();
        assert_eq!(status, 200);
        assert_eq!(changes.len(), 1);

        // Verify metadata was updated
        assert_eq!(
            message
                .metadata
                .get("SwiftMT")
                .and_then(|v| v.get("message_type")),
            Some(&json!("103"))
        );
    }

    #[test]
    fn test_map_null_values_skip_assignment() {
        // Test that null evaluation results skip the mapping entirely
        let datalogic = Arc::new(DataLogic::with_preserve_structure());

        // Create test message with existing values
        let mut message = Message::new(Arc::new(json!({})));
        message.data = json!({
            "existing_field": "should_remain"
        });
        message.metadata = json!({
            "existing_meta": "should_remain"
        });

        // Create mapping config that would return null
        let config = MapConfig {
            mappings: vec![
                MapMapping {
                    path: "data.new_field".to_string(),
                    logic: json!({"var": "data.non_existent_field"}), // This will return null
                    logic_index: Some(0),
                },
                MapMapping {
                    path: "metadata.new_meta".to_string(),
                    logic: json!({"var": "data.another_non_existent"}), // This will return null
                    logic_index: Some(1),
                },
                MapMapping {
                    path: "data.actual_field".to_string(),
                    logic: json!("actual_value"), // This will succeed
                    logic_index: Some(2),
                },
            ],
        };

        // Compile the logic
        let logic_cache = vec![
            datalogic.compile(&config.mappings[0].logic).unwrap(),
            datalogic.compile(&config.mappings[1].logic).unwrap(),
            datalogic.compile(&config.mappings[2].logic).unwrap(),
        ];

        // Execute the mapping
        let result = config.execute(&mut message, &datalogic, &logic_cache);
        assert!(result.is_ok());

        let (status, changes) = result.unwrap();
        assert_eq!(status, 200);
        // Only one change should be recorded (the non-null mapping)
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path.as_ref(), "data.actual_field");

        // Verify that null mappings were skipped (fields don't exist)
        assert_eq!(message.data.get("new_field"), None);
        assert_eq!(message.metadata.get("new_meta"), None);

        // Verify existing fields remain unchanged
        assert_eq!(
            message.data.get("existing_field"),
            Some(&json!("should_remain"))
        );
        assert_eq!(
            message.metadata.get("existing_meta"),
            Some(&json!("should_remain"))
        );

        // Verify the successful mapping
        assert_eq!(
            message.data.get("actual_field"),
            Some(&json!("actual_value"))
        );
    }

    #[test]
    fn test_map_multiple_fields_including_metadata() {
        // Test mapping to data, metadata, and temp_data in one task
        let datalogic = Arc::new(DataLogic::with_preserve_structure());

        // Create test message
        let mut message = Message::new(Arc::new(json!({})));
        message.data = json!({
            "ISO20022_MX": {
                "document": {
                    "TxInf": {
                        "OrgnlGrpInf": {
                            "OrgnlMsgNmId": "pacs.008.001.08"
                        }
                    }
                }
            },
            "SwiftMT": {
                "message_type": "103"
            }
        });

        // Create mapping config with multiple mappings
        let mut config = MapConfig {
            mappings: vec![
                MapMapping {
                    path: "data.SwiftMT.message_type".to_string(),
                    logic: json!("103"),
                    logic_index: None,
                },
                MapMapping {
                    path: "metadata.SwiftMT.message_type".to_string(),
                    logic: json!({"var": "data.SwiftMT.message_type"}),
                    logic_index: None,
                },
                MapMapping {
                    path: "temp_data.original_msg_type".to_string(),
                    logic: json!({"var": "data.ISO20022_MX.document.TxInf.OrgnlGrpInf.OrgnlMsgNmId"}),
                    logic_index: None,
                },
            ],
        };

        // Compile the logic and set indices
        let mut logic_cache = Vec::new();
        for (i, mapping) in config.mappings.iter_mut().enumerate() {
            logic_cache.push(datalogic.compile(&mapping.logic).unwrap());
            mapping.logic_index = Some(i);
        }

        // Execute the mapping
        let result = config.execute(&mut message, &datalogic, &logic_cache);
        assert!(result.is_ok());

        let (status, changes) = result.unwrap();
        assert_eq!(status, 200);
        assert_eq!(changes.len(), 3);

        // Verify all fields were updated correctly
        assert_eq!(
            message
                .data
                .get("SwiftMT")
                .and_then(|v| v.get("message_type")),
            Some(&json!("103"))
        );
        assert_eq!(
            message
                .metadata
                .get("SwiftMT")
                .and_then(|v| v.get("message_type")),
            Some(&json!("103"))
        );
        assert_eq!(
            message.temp_data.get("original_msg_type"),
            Some(&json!("pacs.008.001.08"))
        );
    }
}
