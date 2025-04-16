use crate::engine::error::{DataflowError, Result};
use crate::engine::message::{Change, Message};
use crate::engine::FunctionHandler;
use datalogic_rs::DataLogic;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// A function that maps data between different parts of a message
///
/// This function allows transforming data from one location in the message to another,
/// using JSONLogic expressions to perform the transformations.
pub struct MapFunction {
    /// Reference to DataLogic instance for evaluating expressions
    data_logic: Arc<Mutex<DataLogic>>,
}

impl Default for MapFunction {
    fn default() -> Self {
        Self::new()
    }
}

impl MapFunction {
    /// Create a new MapFunction with a provided DataLogic mutex
    pub fn new_with_mutex(data_logic: Arc<Mutex<DataLogic>>) -> Self {
        Self { data_logic }
    }

    /// Create a new MapFunction with a fresh DataLogic instance
    pub fn new() -> Self {
        Self {
            data_logic: Arc::new(Mutex::new(DataLogic::new())),
        }
    }
}

impl FunctionHandler for MapFunction {
    fn execute(&self, message: &mut Message, input: &Value) -> Result<(usize, Vec<Change>)> {
        // Extract mappings from input
        let mappings = input
            .get("mappings")
            .ok_or_else(|| DataflowError::Validation("No mappings provided".to_string()))?
            .as_array()
            .ok_or_else(|| DataflowError::Validation("Mappings must be an array".to_string()))?;

        if mappings.is_empty() {
            return Err(DataflowError::Validation(
                "Mappings array is empty".to_string(),
            ));
        }

        let mut changes = Vec::with_capacity(mappings.len());

        // Process each mapping
        for (i, mapping) in mappings.iter().enumerate() {
            // Get the target path
            let path = mapping
                .get("path")
                .ok_or_else(|| {
                    DataflowError::Validation(format!(
                        "Mapping at index {} does not have a 'path' field",
                        i
                    ))
                })?
                .as_str()
                .ok_or_else(|| {
                    DataflowError::Validation(format!("Path at index {} must be a string", i))
                })?;

            // Get the logic to apply
            let logic = mapping.get("logic").ok_or_else(|| {
                DataflowError::Validation(format!(
                    "Mapping at index {} does not have a 'logic' field",
                    i
                ))
            })?;

            // Create a context with all message data
            let context = json!({
                "data": message.data,
                "payload": message.payload,
                "metadata": message.metadata,
                "temp_data": message.temp_data
            });

            // Evaluate the logic
            let result = self
                .data_logic
                .lock()
                .map_err(|_| {
                    DataflowError::Unknown("Failed to acquire data_logic lock".to_string())
                })?
                .evaluate_json(logic, &context, None)
                .map_err(|e| {
                    DataflowError::LogicEvaluation(format!(
                        "Failed to evaluate logic at mapping index {}: {}",
                        i, e
                    ))
                })?;

            // Set the value in the message
            let path_parts: Vec<&str> = path.split('.').collect();

            if path_parts.is_empty() {
                return Err(DataflowError::Validation(format!(
                    "Path at index {} is empty",
                    i
                )));
            }

            let target_container = match path_parts[0] {
                "data" => &mut message.data,
                "payload" => &mut message.payload,
                "metadata" => &mut message.metadata,
                "temp_data" => &mut message.temp_data,
                _ => return Err(DataflowError::Validation(
                    format!("Invalid container '{}' at index {}. Must be one of: data, payload, metadata, temp_data", 
                            path_parts[0], i)
                )),
            };

            // Navigate to the target location
            let mut current = target_container;
            let mut old_value = Value::Null;

            for part in path_parts.iter().take(path_parts.len() - 1).skip(1) {
                if !current.is_object() {
                    *current = json!({});
                }

                if current.get(part).is_none() {
                    current[part] = json!({});
                }

                current = &mut current[part];
            }

            if path_parts.len() > 1 {
                let last_part = path_parts[path_parts.len() - 1];

                if !current.is_object() {
                    *current = json!({});
                }

                if current.get(last_part).is_some() {
                    old_value = current[last_part].clone();
                }

                current[last_part] = result.clone();
            } else {
                // If the path is just a single part, we're replacing the whole container
                old_value = current.clone();
                *current = result.clone();
            }

            // Record the change
            changes.push(Change {
                path: path.to_string(),
                old_value,
                new_value: result,
            });
        }

        Ok((200, changes))
    }
}
