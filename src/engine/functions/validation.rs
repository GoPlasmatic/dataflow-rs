use crate::engine::error::{DataflowError, ErrorInfo, Result};
use crate::engine::message::{Change, Message};
use datalogic_rs::{CompiledLogic, DataLogic};
use log::{debug, error};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// Pre-parsed configuration for validation function
#[derive(Debug, Clone, Deserialize)]
pub struct ValidationConfig {
    pub rules: Vec<ValidationRule>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ValidationRule {
    pub logic: Value,
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

            let message = rule
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Validation failed")
                .to_string();

            parsed_rules.push(ValidationRule {
                logic,
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

        // Use the cached context Arc from the message (validation is read-only)
        let context_arc = message.get_context_arc();

        // Process each validation rule
        for (idx, rule) in self.rules.iter().enumerate() {
            debug!("Processing validation rule {}: {}", idx, rule.message);

            // Get the compiled logic from cache with proper bounds checking
            let compiled_logic = match rule.logic_index {
                Some(index) => {
                    // Ensure index is valid before accessing
                    if index >= logic_cache.len() {
                        error!(
                            "Validation: Logic index {} out of bounds (cache size: {}) for rule at index {}",
                            index,
                            logic_cache.len(),
                            idx
                        );
                        validation_errors.push(ErrorInfo::simple_ref(
                            "COMPILATION_ERROR",
                            &format!(
                                "Logic index {} out of bounds for rule at index {}",
                                index, idx
                            ),
                            None,
                        ));
                        continue;
                    }
                    &logic_cache[index]
                }
                None => {
                    error!(
                        "Validation: Logic not compiled (no index) for rule at index {}",
                        idx
                    );
                    validation_errors.push(ErrorInfo::simple_ref(
                        "COMPILATION_ERROR",
                        &format!("Logic not compiled for rule at index: {}", idx),
                        None,
                    ));
                    continue;
                }
            };

            // Evaluate the validation rule using DataLogic v4
            // Reuse the same Arc for all rules - validation is read-only
            let result = datalogic.evaluate(compiled_logic, Arc::clone(&context_arc));

            match result {
                Ok(value) => {
                    // Check if validation passed (must be explicitly true)
                    if value != Value::Bool(true) {
                        debug!("Validation failed for rule {}: {}", idx, rule.message);
                        validation_errors.push(ErrorInfo::simple_ref(
                            "VALIDATION_ERROR",
                            &rule.message,
                            None,
                        ));
                    } else {
                        debug!("Validation passed for rule {}", idx);
                    }
                }
                Err(e) => {
                    error!("Validation: Error evaluating rule {}: {:?}", idx, e);
                    validation_errors.push(ErrorInfo::simple_ref(
                        "EVALUATION_ERROR",
                        &format!("Failed to evaluate rule {}: {}", idx, e),
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
        assert_eq!(config.rules[0].message, "Required field is missing");
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
        assert_eq!(config.rules[0].message, "Validation failed");
    }
}
