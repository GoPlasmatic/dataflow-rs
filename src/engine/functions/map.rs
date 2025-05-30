use crate::engine::error::{DataflowError, Result};
use crate::engine::message::{Change, Message};
use crate::engine::AsyncFunctionHandler;
use crate::engine::functions::FUNCTION_DATA_LOGIC;
use async_trait::async_trait;
use serde_json::{json, Value};

/// A mapping function that transforms data using JSONLogic expressions.
///
/// This function allows mapping data from one location to another within
/// a message, applying transformations using JSONLogic expressions.
pub struct MapFunction {
    // No longer needs data_logic field
}

impl Default for MapFunction {
    fn default() -> Self {
        Self::new()
    }
}

impl MapFunction {
    /// Create a new MapFunction
    pub fn new() -> Self {
        Self {}
    }

    /// Set a value at the specified path within the target object
    fn set_value_at_path(&self, target: &mut Value, path: &str, value: Value) -> Result<Value> {
        let mut current = target;
        let mut old_value = Value::Null;
        let path_parts: Vec<&str> = path.split('.').collect();

        // Navigate to the parent of the target location
        for (i, part) in path_parts.iter().enumerate() {
            if i == path_parts.len() - 1 {
                // We're at the last part, so set the value
                if current.is_object() {
                    if let Value::Object(map) = current {
                        // Save the old value before replacing
                        old_value = map.get(*part).cloned().unwrap_or(Value::Null);
                        map.insert(part.to_string(), value.clone());
                    }
                } else if current.is_array() {
                    // Handle array indices with special care
                    if let Ok(index) = part.parse::<usize>() {
                        if let Value::Array(arr) = current {
                            // Extend array if needed
                            while arr.len() <= index {
                                arr.push(Value::Null);
                            }
                            // Save old value
                            old_value = arr[index].clone();
                            arr[index] = value.clone();
                        }
                    } else {
                        return Err(DataflowError::Validation(format!(
                            "Invalid array index: {}",
                            part
                        )));
                    }
                } else {
                    return Err(DataflowError::Validation(format!(
                        "Cannot set property '{}' on non-object value",
                        part
                    )));
                }
            } else {
                // We need to navigate deeper
                match current {
                    Value::Object(map) => {
                        if !map.contains_key(*part) {
                            map.insert(part.to_string(), json!({}));
                        }
                        current = map.get_mut(*part).unwrap();
                    }
                    Value::Array(arr) => {
                        if let Ok(index) = part.parse::<usize>() {
                            // Extend array if needed
                            while arr.len() <= index {
                                arr.push(json!({}));
                            }
                            current = &mut arr[index];
                        } else {
                            return Err(DataflowError::Validation(format!(
                                "Invalid array index: {}",
                                part
                            )));
                        }
                    }
                    _ => {
                        return Err(DataflowError::Validation(format!(
                            "Cannot navigate path '{}' on non-object value",
                            part
                        )));
                    }
                }
            }
        }

        Ok(old_value)
    }
}

#[async_trait]
impl AsyncFunctionHandler for MapFunction {
    async fn execute(&self, message: &mut Message, input: &Value) -> Result<(usize, Vec<Change>)> {
        // Extract mappings array from input
        let mappings = input.get("mappings").ok_or_else(|| {
            DataflowError::Validation("Missing 'mappings' array in input".to_string())
        })?;

        let mappings_arr = mappings
            .as_array()
            .ok_or_else(|| DataflowError::Validation("'mappings' must be an array".to_string()))?;

        let mut changes = Vec::new();

        // Process each mapping
        for mapping in mappings_arr {
            // Get path where to store the result
            let target_path = mapping.get("path").and_then(Value::as_str).ok_or_else(|| {
                DataflowError::Validation("Missing 'path' in mapping".to_string())
            })?;

            // Get the logic to evaluate
            let logic = mapping.get("logic").ok_or_else(|| {
                DataflowError::Validation("Missing 'logic' in mapping".to_string())
            })?;

            // Clone message data for evaluation context
            let data_clone = message.data.clone();
            let metadata_clone = message.metadata.clone();
            let temp_data_clone = message.temp_data.clone();

            // Create a combined data object with message fields for evaluation
            let data_for_eval = json!({
                "data": data_clone,
                "metadata": metadata_clone,
                "temp_data": temp_data_clone,
            });

            // Determine target object based on path prefix
            let (target_object, adjusted_path) =
                if let Some(path) = target_path.strip_prefix("data.") {
                    (&mut message.data, path)
                } else if let Some(path) = target_path.strip_prefix("metadata.") {
                    (&mut message.metadata, path)
                } else if let Some(path) = target_path.strip_prefix("temp_data.") {
                    (&mut message.temp_data, path)
                } else if target_path == "data" {
                    (&mut message.data, "")
                } else if target_path == "metadata" {
                    (&mut message.metadata, "")
                } else if target_path == "temp_data" {
                    (&mut message.temp_data, "")
                } else {
                    // Default to data
                    (&mut message.data, target_path)
                };

            // Evaluate the logic using thread-local DataLogic
            let result = FUNCTION_DATA_LOGIC.with(|data_logic_cell| {
                let data_logic = data_logic_cell.borrow_mut();

                data_logic
                    .evaluate_json(logic, &data_for_eval, None)
                    .map_err(|e| {
                        DataflowError::LogicEvaluation(format!("Failed to evaluate logic: {}", e))
                    })
            })?;

            // Set the result at the target path
            if adjusted_path.is_empty() {
                // Replace the entire object
                let old_value = std::mem::replace(target_object, result.clone());
                changes.push(Change {
                    path: target_path.to_string(),
                    old_value,
                    new_value: result,
                });
            } else {
                // Set a specific path
                let old_value =
                    self.set_value_at_path(target_object, adjusted_path, result.clone())?;
                changes.push(Change {
                    path: target_path.to_string(),
                    old_value,
                    new_value: result,
                });
            }
        }

        Ok((200, changes))
    }
}
