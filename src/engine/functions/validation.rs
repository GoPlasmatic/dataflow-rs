use crate::engine::AsyncFunctionHandler;
use crate::engine::error::{DataflowError, ErrorInfo, Result};
use crate::engine::functions::FunctionConfig;
use crate::engine::message::{Change, Message};
use crate::engine::thread_local;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::vec;

/// Pre-parsed configuration for validation function
#[derive(Debug, Clone, Deserialize)]
pub struct ValidationConfig {
    pub rules: Vec<ValidationRule>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ValidationRule {
    pub logic: Value,
    pub path: String,
    pub message: String,
}

impl ValidationConfig {
    pub fn from_json(input: &Value) -> Result<Self> {
        let rules = input.get("rules").ok_or_else(|| {
            DataflowError::Validation("Missing 'rules' array in input".to_string())
        })?;

        let rules_arr = rules
            .as_array()
            .ok_or_else(|| DataflowError::Validation("'rules' must be an array".to_string()))?;

        let mut parsed_rules = Vec::new();

        for rule in rules_arr {
            let logic = rule
                .get("logic")
                .ok_or_else(|| DataflowError::Validation("Missing 'logic' in rule".to_string()))?
                .clone();

            let path = rule
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("data")
                .to_string();

            let message = rule
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Validation failed")
                .to_string();

            parsed_rules.push(ValidationRule {
                logic,
                path,
                message,
            });
        }

        Ok(ValidationConfig {
            rules: parsed_rules,
        })
    }
}

/// A validation task function that uses JsonLogic expressions to validate message data.
///
/// This function executes validation rules defined in JsonLogic against data in the message,
/// and reports validation failures by adding errors to the message's errors array.
pub struct ValidationFunction;

impl Default for ValidationFunction {
    fn default() -> Self {
        Self::new()
    }
}

impl ValidationFunction {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AsyncFunctionHandler for ValidationFunction {
    async fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
    ) -> Result<(usize, Vec<Change>)> {
        // Extract the pre-parsed validation configuration
        let validation_config = match config {
            FunctionConfig::Validation(config) => config,
            _ => {
                return Err(DataflowError::Validation(
                    "Invalid configuration type for validation function".to_string(),
                ));
            }
        };

        // Process each rule
        for rule in &validation_config.rules {
            let rule_logic = &rule.logic;
            let rule_path = &rule.path;
            let error_message = &rule.message;

            let data_to_validate = if rule_path == "data" || rule_path.starts_with("data.") {
                &json!({"data": message.data})
            } else if rule_path == "metadata" || rule_path.starts_with("metadata.") {
                &json!({"metadata": message.metadata})
            } else if rule_path == "temp_data" || rule_path.starts_with("temp_data.") {
                &json!({"temp_data": message.temp_data})
            } else {
                &json!({"data": message.data})
            };

            // Evaluate the rule using thread-local DataLogic
            match thread_local::evaluate_json(rule_logic, data_to_validate) {
                Ok(v) => {
                    if !v.as_bool().unwrap_or(false) {
                        let message_key = error_message.clone();

                        println!("Validation failed: {message_key}");

                        // Store the validation error in the message's errors array
                        message.errors.push(ErrorInfo::new(
                            None,
                            None,
                            DataflowError::Validation(message_key.clone()),
                        ));

                        // Store validation failure in temp_data for tracking
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
                                errors_array.push(json!(message_key));
                            }
                        }

                        // Continue checking other rules instead of returning immediately
                    }
                }
                Err(e) => {
                    println!("Error evaluating rule: {e}");
                    return Err(DataflowError::LogicEvaluation(format!(
                        "Error evaluating rule: {e}"
                    )));
                }
            }
        }

        // Check if any validation errors occurred
        let has_validation_errors = message
            .temp_data
            .get("validation_errors")
            .and_then(|v| v.as_array())
            .map(|arr| !arr.is_empty())
            .unwrap_or(false);

        // Create changes to track validation results
        let mut changes = vec![];

        if message.temp_data.get("validation").is_some()
            && !message.temp_data["validation"].is_null()
        {
            changes.push(Change {
                path: "temp_data.validation".to_string(),
                old_value: Value::Null,
                new_value: message.temp_data["validation"].clone(),
            });
        }

        if has_validation_errors {
            changes.push(Change {
                path: "temp_data.validation_errors".to_string(),
                old_value: Value::Null,
                new_value: message.temp_data["validation_errors"].clone(),
            });
        }

        // Return appropriate status code
        if has_validation_errors {
            Ok((400, changes))
        } else {
            Ok((200, changes))
        }
    }
}
