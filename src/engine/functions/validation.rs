use crate::engine::error::{DataflowError, ErrorInfo, Result};
use crate::engine::message::{Change, Message};
use crate::engine::utils::is_truthy;
use datalogic_rs::{CompiledLogic, DataLogic};
use log::{debug, error};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

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
    #[serde(skip)]
    pub logic_index: Option<usize>,
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
                logic_index: None,
            });
        }

        Ok(ValidationConfig {
            rules: parsed_rules,
        })
    }

    /// Execute the validation rules using pre-compiled logic
    pub fn execute(
        &self,
        message: &mut Message,
        datalogic: &Arc<DataLogic>,
        logic_cache: &[Arc<CompiledLogic>],
    ) -> Result<(usize, Vec<Change>)> {
        let changes = Vec::new();
        let mut validation_errors = Vec::new();

        // Combine all message fields for validation
        let data_to_validate = json!({
            "data": &message.data,
            "payload": message.payload.as_ref(),
            "metadata": &message.metadata,
            "temp_data": &message.temp_data
        });
        let data_to_validate = Arc::new(data_to_validate);

        // Process each validation rule
        for (idx, rule) in self.rules.iter().enumerate() {
            debug!("Processing validation rule {}: {}", idx, rule.message);

            // Get the compiled logic from cache
            let compiled_logic = match rule.logic_index {
                Some(index) if index < logic_cache.len() => &logic_cache[index],
                _ => {
                    error!("Validation: Logic not compiled for rule at index {}", idx);
                    validation_errors.push(ErrorInfo::simple(
                        "COMPILATION_ERROR".to_string(),
                        format!("Logic not compiled for rule at index: {}", idx),
                        None,
                    ));
                    continue;
                }
            };

            // Evaluate the validation rule using DataLogic v4
            // DataLogic v4 is thread-safe with Arc<CompiledLogic>, no spawn_blocking needed
            let result = datalogic.evaluate(compiled_logic, Arc::clone(&data_to_validate));

            match result {
                Ok(value) => {
                    // Check if validation passed (truthy value)
                    if !is_truthy(&value) {
                        debug!("Validation failed for rule {}: {}", idx, rule.message);
                        validation_errors.push(ErrorInfo::simple(
                            "VALIDATION_ERROR".to_string(),
                            rule.message.clone(),
                            Some(rule.path.clone()),
                        ));
                    } else {
                        debug!("Validation passed for rule {}", idx);
                    }
                }
                Err(e) => {
                    error!("Validation: Error evaluating rule {}: {:?}", idx, e);
                    validation_errors.push(ErrorInfo::simple(
                        "EVALUATION_ERROR".to_string(),
                        format!("Failed to evaluate rule {}: {}", idx, e),
                        None,
                    ));
                }
            }
        }

        // Add validation errors to message if any
        if !validation_errors.is_empty() {
            message.errors.extend(validation_errors);
            Ok((400, changes)) // Return 400 for validation failures
        } else {
            Ok((200, changes))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_validation_config_from_json() {
        let input = json!({
            "rules": [
                {
                    "logic": {"!!": [{"var": "data.required_field"}]},
                    "path": "data",
                    "message": "Required field is missing"
                },
                {
                    "logic": {">": [{"var": "data.age"}, 18]},
                    "message": "Must be over 18"
                }
            ]
        });

        let config = ValidationConfig::from_json(&input).unwrap();
        assert_eq!(config.rules.len(), 2);
        assert_eq!(config.rules[0].path, "data");
        assert_eq!(config.rules[0].message, "Required field is missing");
        assert_eq!(config.rules[1].path, "data"); // Default path
        assert_eq!(config.rules[1].message, "Must be over 18");
    }

    #[test]
    fn test_validation_config_missing_rules() {
        let input = json!({});
        let result = ValidationConfig::from_json(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_config_invalid_rules() {
        let input = json!({
            "rules": "not_an_array"
        });
        let result = ValidationConfig::from_json(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_config_missing_logic() {
        let input = json!({
            "rules": [
                {
                    "path": "data",
                    "message": "Some error"
                }
            ]
        });
        let result = ValidationConfig::from_json(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_config_defaults() {
        let input = json!({
            "rules": [
                {
                    "logic": {"var": "data.field"}
                }
            ]
        });

        let config = ValidationConfig::from_json(&input).unwrap();
        assert_eq!(config.rules[0].path, "data");
        assert_eq!(config.rules[0].message, "Validation failed");
    }
}
