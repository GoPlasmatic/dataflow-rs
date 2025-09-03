//! # Internal Function Execution Module
//!
//! This module handles the efficient execution of built-in functions (map and validation)
//! using pre-compiled logic. It provides optimized execution paths for:
//!
//! - Data transformations with JSONLogic mappings
//! - Rule-based validation with custom error messages
//! - Efficient condition evaluation for workflows and tasks
//! - Minimal allocation through lazy data structure initialization

use crate::engine::error::{DataflowError, ErrorInfo, Result};
use crate::engine::functions::{MapConfig, ValidationConfig};
use crate::engine::message::{Change, Message};
use datalogic_rs::value::ToJson;
use datalogic_rs::{DataLogic, Logic};
use log::{debug, error};
use serde_json::{Value, json};

/// Executes internal functions using pre-compiled logic for optimal performance.
///
/// The `InternalExecutor` provides:
/// - Efficient execution of map transformations using compiled logic
/// - Fast validation rule evaluation with detailed error reporting
/// - Condition evaluation for workflow and task control flow
/// - Lazy initialization to avoid unnecessary allocations
///
/// By using pre-compiled logic from the cache, the executor avoids all
/// runtime compilation overhead, ensuring predictable low-latency execution.
pub struct InternalExecutor<'a> {
    /// Reference to the DataLogic instance for evaluation
    datalogic: &'a DataLogic<'static>,
    /// Reference to the compiled logic cache
    logic_cache: &'a Vec<Logic<'static>>,
}

impl<'a> InternalExecutor<'a> {
    /// Create a new InternalExecutor
    pub fn new(datalogic: &'a DataLogic<'static>, logic_cache: &'a Vec<Logic<'static>>) -> Self {
        Self {
            datalogic,
            logic_cache,
        }
    }

    /// Execute the internal map function with optimized data handling
    pub fn execute_map(
        &self,
        message: &mut Message,
        config: &MapConfig,
    ) -> Result<(usize, Vec<Change>)> {
        let mut changes = Vec::with_capacity(config.mappings.len());

        // Check if we need parsed data for compiled logic
        let needs_parsed_data = config.mappings.iter().any(|m| m.logic_index.is_some());

        // Determine if we need data for evaluation
        let needs_data = needs_parsed_data
            || config
                .mappings
                .iter()
                .any(|m| m.logic.is_object() || m.logic.is_array());

        // Initialize mutable data_for_eval if needed
        let mut data_for_eval = if needs_data {
            Some(json!({
                "data": &message.data,
                "metadata": &message.metadata,
                "temp_data": &message.temp_data,
            }))
        } else {
            None
        };

        for mapping in &config.mappings {
            let target_path = &mapping.path;
            let logic = &mapping.logic;

            // Parse data for compiled logic evaluation if needed
            let parsed_data = if let Some(logic_index) = mapping.logic_index {
                if logic_index < self.logic_cache.len() && data_for_eval.is_some() {
                    Some(
                        self.datalogic
                            .parse_data_json(data_for_eval.as_ref().unwrap())
                            .map_err(|e| {
                                error!("Failed to parse data for evaluation: {e:?}");
                                DataflowError::LogicEvaluation(format!("Error parsing data: {}", e))
                            })?,
                    )
                } else {
                    None
                }
            } else {
                None
            };

            // Evaluate using compiled logic if available
            let result = if let Some(logic_index) = mapping.logic_index {
                if logic_index < self.logic_cache.len() && parsed_data.is_some() {
                    let compiled_logic = &self.logic_cache[logic_index];
                    let eval_result = self
                        .datalogic
                        .evaluate(compiled_logic, parsed_data.as_ref().unwrap())
                        .map_err(|e| {
                            error!("Failed to evaluate compiled logic: {e:?}");
                            DataflowError::LogicEvaluation(format!("Error evaluating logic: {}", e))
                        })?;
                    eval_result.to_json()
                } else if let Some(ref data) = data_for_eval {
                    self.datalogic.evaluate_json(logic, data).map_err(|e| {
                        error!("Failed to evaluate logic: {e:?}");
                        DataflowError::LogicEvaluation(format!("Error evaluating logic: {}", e))
                    })?
                } else {
                    // Fallback for edge cases
                    logic.clone()
                }
            } else {
                // For simple values (strings, numbers, etc.), just clone them directly
                if !logic.is_object() && !logic.is_array() {
                    logic.clone()
                } else if let Some(ref data) = data_for_eval {
                    self.datalogic.evaluate_json(logic, data).map_err(|e| {
                        error!("Failed to evaluate logic: {e:?}");
                        DataflowError::LogicEvaluation(format!("Error evaluating logic: {}", e))
                    })?
                } else {
                    logic.clone()
                }
            };

            if result.is_null() {
                continue;
            }

            // Determine target object based on path prefix
            let (target_object, adjusted_path) = self.resolve_target_path(message, target_path);

            // Set the result at the target path
            let old_value = self.set_value_at_path(target_object, adjusted_path, &result)?;

            changes.push(Change {
                path: target_path.clone(),
                old_value,
                new_value: result.clone(),
            });

            // Update data_for_eval with the new value so subsequent mappings see the changes
            if let Some(ref mut data) = data_for_eval {
                // Update the appropriate field in data_for_eval based on the path
                if let Some(adjusted_path) = target_path.strip_prefix("data.") {
                    if let Some(data_obj) = data.get_mut("data") {
                        let _ = self.set_value_at_path(data_obj, adjusted_path, &result);
                    }
                } else if let Some(adjusted_path) = target_path.strip_prefix("temp_data.") {
                    if let Some(temp_data_obj) = data.get_mut("temp_data") {
                        let _ = self.set_value_at_path(temp_data_obj, adjusted_path, &result);
                    }
                } else if let Some(adjusted_path) = target_path.strip_prefix("metadata.")
                    && let Some(metadata_obj) = data.get_mut("metadata")
                {
                    let _ = self.set_value_at_path(metadata_obj, adjusted_path, &result);
                }
            }
        }

        Ok((200, changes))
    }

    /// Execute the internal validation function
    pub fn execute_validate(
        &self,
        message: &mut Message,
        config: &ValidationConfig,
    ) -> Result<(usize, Vec<Change>)> {
        // Pre-parse data for different validation paths
        let data_json = json!({"data": &message.data});
        let metadata_json = json!({"metadata": &message.metadata});
        let temp_data_json = json!({"temp_data": &message.temp_data});

        // For now, we'll skip the caching optimization since DataValue has lifetime issues
        // This will be addressed in a future optimization

        for rule in &config.rules {
            let rule_logic = &rule.logic;
            let rule_path = &rule.path;
            let error_message = &rule.message;

            // Evaluate using compiled logic if available
            let validation_result = if let Some(logic_index) = rule.logic_index {
                if logic_index < self.logic_cache.len() {
                    let compiled_logic = &self.logic_cache[logic_index];
                    let data_to_validate = if rule_path == "data" || rule_path.starts_with("data.")
                    {
                        &data_json
                    } else if rule_path == "metadata" || rule_path.starts_with("metadata.") {
                        &metadata_json
                    } else {
                        &temp_data_json
                    };

                    if let Ok(data_val) = self.datalogic.parse_data_json(data_to_validate) {
                        self.datalogic
                            .evaluate(compiled_logic, data_val)
                            .map(|v| v.as_bool().unwrap_or(false))
                            .unwrap_or(false)
                    } else {
                        false
                    }
                } else {
                    // Fallback to JSON evaluation
                    let data_to_validate = if rule_path == "data" || rule_path.starts_with("data.")
                    {
                        &data_json
                    } else if rule_path == "metadata" || rule_path.starts_with("metadata.") {
                        &metadata_json
                    } else {
                        &temp_data_json
                    };

                    self.datalogic
                        .evaluate_json(rule_logic, data_to_validate)
                        .map(|v| v.as_bool().unwrap_or(false))
                        .unwrap_or(false)
                }
            } else {
                // Direct evaluation for non-compiled logic
                let data_to_validate = if rule_path == "data" || rule_path.starts_with("data.") {
                    &data_json
                } else if rule_path == "metadata" || rule_path.starts_with("metadata.") {
                    &metadata_json
                } else {
                    &temp_data_json
                };

                self.datalogic
                    .evaluate_json(rule_logic, data_to_validate)
                    .map(|v| v.as_bool().unwrap_or(false))
                    .unwrap_or(false)
            };

            if !validation_result {
                debug!("Validation failed: {}", error_message);

                // Store the validation error
                message.errors.push(ErrorInfo::new(
                    None,
                    None,
                    DataflowError::Validation(error_message.clone()),
                ));

                // Store validation failure in temp_data
                self.record_validation_error(message, error_message);
            }
        }

        // Check if any validation errors occurred
        let has_validation_errors = message
            .temp_data
            .get("validation_errors")
            .and_then(|v| v.as_array())
            .map(|arr| !arr.is_empty())
            .unwrap_or(false);

        let status = if has_validation_errors { 400 } else { 200 };
        Ok((status, vec![]))
    }

    /// Evaluate logic using compiled index or direct evaluation
    pub fn evaluate_logic(
        &self,
        logic_index: Option<usize>,
        logic: &Value,
        data: &Value,
    ) -> Result<Value> {
        if let Some(index) = logic_index {
            debug!("Using compiled logic at index {}", index);
            if index < self.logic_cache.len() {
                let compiled_logic = &self.logic_cache[index];
                let data_value = self.datalogic.parse_data_json(data).map_err(|e| {
                    DataflowError::LogicEvaluation(format!("Error parsing data: {}", e))
                })?;

                let result = self
                    .datalogic
                    .evaluate(compiled_logic, data_value)
                    .map_err(|e| {
                        DataflowError::LogicEvaluation(format!("Error evaluating logic: {}", e))
                    })?;

                Ok(result.to_json())
            } else {
                Err(DataflowError::LogicEvaluation(format!(
                    "Logic index {} out of bounds",
                    index
                )))
            }
        } else {
            debug!("Evaluating logic directly (not compiled): {:?}", logic);
            self.datalogic.evaluate_json(logic, data).map_err(|e| {
                DataflowError::LogicEvaluation(format!("Error evaluating logic: {}", e))
            })
        }
    }

    /// Evaluate a condition
    pub fn evaluate_condition(
        &self,
        condition_index: Option<usize>,
        condition: &Value,
        data: &Value,
    ) -> Result<bool> {
        // Short-circuit for simple boolean conditions
        if let Value::Bool(b) = condition {
            debug!("Evaluating simple boolean condition: {}", b);
            return Ok(*b);
        }

        if let Some(index) = condition_index {
            debug!("Using compiled logic at index {}", index);
            if index < self.logic_cache.len() {
                let logic = &self.logic_cache[index];
                let data_value = self.datalogic.parse_data_json(data).map_err(|e| {
                    DataflowError::LogicEvaluation(format!("Error parsing data: {}", e))
                })?;

                let result = self
                    .datalogic
                    .evaluate(logic, data_value)
                    .map(|result| result.as_bool().unwrap_or(false))
                    .map_err(|e| {
                        DataflowError::LogicEvaluation(format!("Error evaluating condition: {}", e))
                    });
                debug!("Compiled logic evaluation result: {:?}", result);
                result
            } else {
                Err(DataflowError::LogicEvaluation(format!(
                    "Condition index {} out of bounds",
                    index
                )))
            }
        } else {
            debug!(
                "Evaluating condition directly (not compiled): {:?}",
                condition
            );
            let result = self
                .datalogic
                .evaluate_json(condition, data)
                .map(|result| result.as_bool().unwrap_or(false))
                .map_err(|e| {
                    DataflowError::LogicEvaluation(format!("Error evaluating condition: {}", e))
                });
            debug!("Direct evaluation result: {:?}", result);
            result
        }
    }

    /// Resolve target path for message field
    fn resolve_target_path<'b>(
        &self,
        message: &'b mut Message,
        target_path: &'b str,
    ) -> (&'b mut Value, &'b str) {
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
        }
    }

    /// Record validation error in temp_data
    fn record_validation_error(&self, message: &mut Message, error_message: &str) {
        if !message.temp_data.is_object() {
            message.temp_data = json!({});
        }
        if let Some(obj) = message.temp_data.as_object_mut() {
            if !obj.contains_key("validation_errors") {
                obj.insert("validation_errors".to_string(), json!([]));
            }
            if let Some(errors_array) = obj
                .get_mut("validation_errors")
                .and_then(|v| v.as_array_mut())
            {
                errors_array.push(json!(error_message));
            }
        }
    }

    /// Set a value at the specified path within the target object
    pub fn set_value_at_path(
        &self,
        target: &mut Value,
        path: &str,
        value: &Value,
    ) -> Result<Value> {
        let mut current = target;
        let mut old_value = Value::Null;
        let path_parts: Vec<&str> = path.split('.').collect();

        // Helper function to check if a string is a valid array index
        fn is_numeric_index(s: &str) -> bool {
            s.parse::<usize>().is_ok()
        }

        // Empty path means replace or merge the entire value
        if path.is_empty() {
            old_value = current.clone();
            // If both current and new value are objects, merge them
            if let (Value::Object(current_obj), Value::Object(new_obj)) =
                (current.clone(), value.clone())
            {
                let mut merged = current_obj;
                for (key, val) in new_obj {
                    merged.insert(key, val);
                }
                *current = Value::Object(merged);
            } else {
                // Otherwise, replace entirely
                *current = value.clone();
            }
            return Ok(old_value);
        }

        // Navigate to the parent of the target location
        for (i, part) in path_parts.iter().enumerate() {
            // Strip '#' prefix if present (used to indicate numeric keys that should be treated as strings)
            let (part_clean, force_string) = if let Some(stripped) = part.strip_prefix('#') {
                (stripped, true)
            } else {
                (*part, false)
            };

            // Check if it's numeric only if not forced to be a string
            let is_numeric = !force_string && is_numeric_index(part_clean);

            if i == path_parts.len() - 1 {
                // We're at the last part, so set the value
                if is_numeric {
                    // Handle numeric index - ensure current is an array
                    if !current.is_array() {
                        *current = Value::Array(vec![]);
                    }

                    if let Ok(index) = part_clean.parse::<usize>()
                        && let Value::Array(arr) = current
                    {
                        // Extend array if needed
                        while arr.len() <= index {
                            arr.push(Value::Null);
                        }
                        old_value = arr[index].clone();
                        arr[index] = value.clone();
                    }
                } else {
                    // Handle object key
                    if !current.is_object() {
                        *current = json!({});
                    }

                    if let Value::Object(map) = current {
                        old_value = map.get(part_clean).cloned().unwrap_or(Value::Null);

                        // If both the existing value and new value are objects, merge them
                        if let (Some(Value::Object(existing_obj)), Value::Object(new_obj)) =
                            (map.get(part_clean), value)
                        {
                            // Create a merged object by cloning the existing and adding new fields
                            let mut merged = existing_obj.clone();
                            for (key, val) in new_obj {
                                merged.insert(key.clone(), val.clone());
                            }
                            map.insert(part_clean.to_string(), Value::Object(merged));
                        } else {
                            // Otherwise, just replace the value as before
                            map.insert(part_clean.to_string(), value.clone());
                        }
                    }
                }
            } else {
                // Navigate deeper
                if is_numeric {
                    // Handle array navigation
                    if !current.is_array() {
                        *current = Value::Array(vec![]);
                    }

                    if let Ok(index) = part_clean.parse::<usize>()
                        && let Value::Array(arr) = current
                    {
                        while arr.len() <= index {
                            arr.push(Value::Null);
                        }
                        current = &mut arr[index];
                    }
                } else {
                    // Handle object navigation
                    if !current.is_object() {
                        *current = json!({});
                    }

                    if let Value::Object(map) = current {
                        if !map.contains_key(part_clean) {
                            // Look ahead to see if next part is numeric to decide what to create
                            let next_part = path_parts.get(i + 1).unwrap_or(&"");
                            // Strip '#' prefix from next part if present when checking
                            let next_clean = if let Some(stripped) = next_part.strip_prefix('#') {
                                stripped
                            } else {
                                next_part
                            };
                            if is_numeric_index(next_clean) && !next_part.starts_with('#') {
                                map.insert(part_clean.to_string(), Value::Array(vec![]));
                            } else {
                                map.insert(part_clean.to_string(), json!({}));
                            }
                        }
                        current = map.get_mut(part_clean).unwrap();
                    }
                }
            }
        }

        Ok(old_value)
    }
}
