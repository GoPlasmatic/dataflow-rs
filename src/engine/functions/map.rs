use crate::engine::error::{DataflowError, Result};
use crate::engine::message::{Change, Message};
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

        // Create the Arc once for all mappings
        let mut context_arc = Arc::new(message.context.clone());
        let mut context_modified = false;

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

            // Only recreate context Arc if it was modified
            // This ensures subsequent mappings can see changes while avoiding unnecessary clones
            if context_modified {
                context_arc = Arc::new(message.context.clone());
                context_modified = false;
            }

            // Reuse the Arc by cloning the reference, not the data
            let current_context = Arc::clone(&context_arc);

            // Evaluate the transformation logic using DataLogic v4
            // DataLogic v4 is thread-safe with Arc<CompiledLogic>, no spawn_blocking needed
            let result = datalogic.evaluate(compiled_logic, current_context);

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

                    // Get old value from the appropriate location in context
                    let old_value = get_nested_value(&message.context, &mapping.path);

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

                    // Update the context directly with the transformed value
                    // Check if we're replacing a root field (data, metadata, or temp_data)
                    if mapping.path == "data"
                        || mapping.path == "metadata"
                        || mapping.path == "temp_data"
                    {
                        // Merge with existing field instead of replacing entirely
                        if let Value::Object(new_map) = transformed_value {
                            // If new value is an object, merge its fields
                            if let Value::Object(existing_map) = &mut message.context[&mapping.path]
                            {
                                // Merge new fields into existing object
                                for (key, value) in new_map {
                                    existing_map.insert(key, value);
                                }
                            } else {
                                // If existing is not an object, replace with new object
                                message.context[&mapping.path] = Value::Object(new_map);
                            }
                        } else {
                            // If new value is not an object, replace entirely
                            message.context[&mapping.path] = transformed_value;
                        }
                    } else {
                        // Set nested value in context
                        set_nested_value(&mut message.context, &mapping.path, transformed_value);
                    }
                    context_modified = true; // Mark that we've modified the context
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
        message.context["data"] = json!({
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
            message.context["metadata"]
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
        message.context["data"] = json!({
            "existing_field": "should_remain"
        });
        message.context["metadata"] = json!({
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
        assert_eq!(message.context["data"].get("new_field"), None);
        assert_eq!(message.context["metadata"].get("new_meta"), None);

        // Verify existing fields remain unchanged
        assert_eq!(
            message.context["data"].get("existing_field"),
            Some(&json!("should_remain"))
        );
        assert_eq!(
            message.context["metadata"].get("existing_meta"),
            Some(&json!("should_remain"))
        );

        // Verify the successful mapping
        assert_eq!(
            message.context["data"].get("actual_field"),
            Some(&json!("actual_value"))
        );
    }

    #[test]
    fn test_map_multiple_fields_including_metadata() {
        // Test mapping to data, metadata, and temp_data in one task
        let datalogic = Arc::new(DataLogic::with_preserve_structure());

        // Create test message
        let mut message = Message::new(Arc::new(json!({})));
        message.context["data"] = json!({
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
            message.context["data"]
                .get("SwiftMT")
                .and_then(|v| v.get("message_type")),
            Some(&json!("103"))
        );
        assert_eq!(
            message.context["metadata"]
                .get("SwiftMT")
                .and_then(|v| v.get("message_type")),
            Some(&json!("103"))
        );
        assert_eq!(
            message.context["temp_data"].get("original_msg_type"),
            Some(&json!("pacs.008.001.08"))
        );
    }
}
