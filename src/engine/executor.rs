use crate::engine::error::{DataflowError, ErrorInfo, Result};
use crate::engine::functions::{MapConfig, ValidationConfig};
use crate::engine::message::{Change, Message};
use datalogic_rs::value::ToJson;
use datalogic_rs::{DataLogic, Logic};
use log::{debug, error};
use serde_json::{Value, json};

/// Handles execution of internal map and validation functions
pub struct InternalExecutor<'a> {
    datalogic: &'a DataLogic<'static>,
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
        
        // Lazy initialization of data structures
        let data_for_eval = if needs_parsed_data || config.mappings.iter().any(|m| m.logic.is_object() || m.logic.is_array()) {
            Some(json!({
                "data": &message.data,
                "metadata": &message.metadata,
                "temp_data": &message.temp_data,
            }))
        } else {
            None
        };

        // Parse to DataValue once if we have any compiled logic
        let parsed_data = if needs_parsed_data && data_for_eval.is_some() {
            Some(self.datalogic.parse_data_json(data_for_eval.as_ref().unwrap()).map_err(|e| {
                error!("Failed to parse data for evaluation: {e:?}");
                DataflowError::LogicEvaluation(format!("Error parsing data: {}", e))
            })?)
        } else {
            None
        };

        for mapping in &config.mappings {
            let target_path = &mapping.path;
            let logic = &mapping.logic;

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
                new_value: result,
            });
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
                    let data_to_validate = if rule_path == "data" || rule_path.starts_with("data.") {
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
                
                let result = self.datalogic.evaluate(compiled_logic, data_value).map_err(|e| {
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

        // Empty path means replace the entire value
        if path.is_empty() {
            old_value = current.clone();
            *current = value.clone();
            return Ok(old_value);
        }

        // Navigate to the parent of the target location
        for (i, part) in path_parts.iter().enumerate() {
            let is_numeric = is_numeric_index(part);

            if i == path_parts.len() - 1 {
                // We're at the last part, so set the value
                if is_numeric {
                    // Handle numeric index - ensure current is an array
                    if !current.is_array() {
                        *current = Value::Array(vec![]);
                    }

                    if let Ok(index) = part.parse::<usize>() {
                        if let Value::Array(arr) = current {
                            // Extend array if needed
                            while arr.len() <= index {
                                arr.push(Value::Null);
                            }
                            old_value = arr[index].clone();
                            arr[index] = value.clone();
                        }
                    }
                } else {
                    // Handle object key
                    if !current.is_object() {
                        *current = json!({});
                    }

                    if let Value::Object(map) = current {
                        old_value = map
                            .get(part.to_string().as_str())
                            .cloned()
                            .unwrap_or(Value::Null);
                        map.insert(part.to_string(), value.clone());
                    }
                }
            } else {
                // Navigate deeper
                if is_numeric {
                    // Handle array navigation
                    if !current.is_array() {
                        *current = Value::Array(vec![]);
                    }

                    if let Ok(index) = part.parse::<usize>() {
                        if let Value::Array(arr) = current {
                            while arr.len() <= index {
                                arr.push(Value::Null);
                            }
                            current = &mut arr[index];
                        }
                    }
                } else {
                    // Handle object navigation
                    if !current.is_object() {
                        *current = json!({});
                    }

                    if let Value::Object(map) = current {
                        if !map.contains_key(part.to_string().as_str()) {
                            // Look ahead to see if next part is numeric to decide what to create
                            let next_part = path_parts.get(i + 1).unwrap_or(&"");
                            if is_numeric_index(next_part) {
                                map.insert(part.to_string(), Value::Array(vec![]));
                            } else {
                                map.insert(part.to_string(), json!({}));
                            }
                        }
                        current = map.get_mut(part.to_string().as_str()).unwrap();
                    }
                }
            }
        }

        Ok(old_value)
    }
}