//! # Validation Function Module
//!
//! This module provides rule-based validation capabilities using JSONLogic expressions.
//! The validation function evaluates a set of rules against message data and collects
//! any validation errors that occur.
//!
//! ## Features
//!
//! - Define validation rules using JSONLogic expressions
//! - Custom error messages for each rule
//! - Non-destructive: validation is read-only and doesn't modify message data
//! - Errors are collected in the message's error list
//!
//! ## Example Usage
//!
//! ```json
//! {
//!     "name": "validation",
//!     "input": {
//!         "rules": [
//!             {
//!                 "logic": {"!!": [{"var": "data.email"}]},
//!                 "message": "Email is required"
//!             },
//!             {
//!                 "logic": {">": [{"var": "data.age"}, 0]},
//!                 "message": "Age must be positive"
//!             }
//!         ]
//!     }
//! }
//! ```

use crate::engine::error::{DataflowError, ErrorInfo, Result};
use crate::engine::message::{Change, Message};
use datalogic_rs::{CompiledLogic, DataLogic};
use log::{debug, error};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// Configuration for the validation function containing a list of rules.
///
/// Each rule specifies a JSONLogic condition that must evaluate to `true`
/// for the validation to pass. If a rule evaluates to anything other than
/// `true`, its error message is added to the message's error list.
#[derive(Debug, Clone, Deserialize)]
pub struct ValidationConfig {
    /// List of validation rules to evaluate.
    pub rules: Vec<ValidationRule>,
}

/// A single validation rule with a condition and error message.
///
/// The rule's logic is evaluated against the message context. If it does not
/// return exactly `true`, the validation fails and the error message is recorded.
#[derive(Debug, Clone, Deserialize)]
pub struct ValidationRule {
    /// JSONLogic expression that must evaluate to `true` for validation to pass.
    /// Any other result (false, null, etc.) is considered a validation failure.
    pub logic: Value,

    /// Error message to display if validation fails.
    /// Defaults to "Validation failed" if not specified.
    pub message: String,

    /// Index into the compiled logic cache. Set during workflow compilation.
    #[serde(skip)]
    pub logic_index: Option<usize>,
}

impl ValidationConfig {
    /// Parses a `ValidationConfig` from a JSON value.
    ///
    /// # Arguments
    /// * `input` - JSON object containing a "rules" array
    ///
    /// # Errors
    /// Returns `DataflowError::Validation` if:
    /// - The "rules" field is missing
    /// - The "rules" field is not an array
    /// - Any rule is missing the "logic" field
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

    /// Executes all validation rules using pre-compiled logic.
    ///
    /// Evaluates each rule sequentially against the message context.
    /// This is a read-only operation that does not modify message data.
    ///
    /// # Arguments
    /// * `message` - The message to validate (errors are added to its error list)
    /// * `datalogic` - DataLogic instance for evaluation
    /// * `logic_cache` - Pre-compiled logic expressions
    ///
    /// # Returns
    /// * `Ok((200, []))` - All rules passed, no changes made
    /// * `Ok((400, []))` - One or more rules failed, errors added to message
    ///
    /// # Error Types
    /// Validation errors are recorded with the following codes:
    /// - `VALIDATION_ERROR` - Rule evaluated to non-true value
    /// - `EVALUATION_ERROR` - Rule evaluation failed with an error
    /// - `COMPILATION_ERROR` - Logic was not properly compiled
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

    #[test]
    fn test_validation_execute_passes() {
        use crate::engine::message::Message;

        let datalogic = Arc::new(DataLogic::with_preserve_structure());

        // Create test message with valid data
        let mut message = Message::new(Arc::new(json!({})));
        message.context["data"] = json!({
            "email": "test@example.com",
            "age": 25
        });

        // Create validation rules that should pass
        let mut config = ValidationConfig {
            rules: vec![
                ValidationRule {
                    logic: json!({"!!": [{"var": "data.email"}]}),
                    message: "Email is required".to_string(),
                    logic_index: None,
                },
                ValidationRule {
                    logic: json!({">": [{"var": "data.age"}, 18]}),
                    message: "Must be over 18".to_string(),
                    logic_index: None,
                },
            ],
        };

        // Compile the logic and set indices
        let mut logic_cache = Vec::new();
        for (i, rule) in config.rules.iter_mut().enumerate() {
            logic_cache.push(datalogic.compile(&rule.logic).unwrap());
            rule.logic_index = Some(i);
        }

        // Execute validation
        let result = config.execute(&mut message, &datalogic, &logic_cache);
        assert!(result.is_ok());

        let (status, changes) = result.unwrap();
        assert_eq!(status, 200);
        assert!(changes.is_empty()); // Validation doesn't create changes
        assert!(message.errors.is_empty()); // No validation errors
    }

    #[test]
    fn test_validation_execute_fails() {
        use crate::engine::message::Message;

        let datalogic = Arc::new(DataLogic::with_preserve_structure());

        // Create test message with invalid data
        let mut message = Message::new(Arc::new(json!({})));
        message.context["data"] = json!({
            "age": 15  // Missing email and age under 18
        });

        // Create validation rules
        let mut config = ValidationConfig {
            rules: vec![
                ValidationRule {
                    logic: json!({"!!": [{"var": "data.email"}]}),
                    message: "Email is required".to_string(),
                    logic_index: None,
                },
                ValidationRule {
                    logic: json!({">": [{"var": "data.age"}, 18]}),
                    message: "Must be over 18".to_string(),
                    logic_index: None,
                },
            ],
        };

        // Compile the logic and set indices
        let mut logic_cache = Vec::new();
        for (i, rule) in config.rules.iter_mut().enumerate() {
            logic_cache.push(datalogic.compile(&rule.logic).unwrap());
            rule.logic_index = Some(i);
        }

        // Execute validation
        let result = config.execute(&mut message, &datalogic, &logic_cache);
        assert!(result.is_ok());

        let (status, _changes) = result.unwrap();
        assert_eq!(status, 400); // Validation failure returns 400
        assert_eq!(message.errors.len(), 2); // Two validation errors

        // Check error messages
        let error_messages: Vec<&str> = message.errors.iter().map(|e| e.message.as_str()).collect();
        assert!(error_messages.contains(&"Email is required"));
        assert!(error_messages.contains(&"Must be over 18"));
    }

    #[test]
    fn test_validation_uncompiled_logic() {
        use crate::engine::message::Message;

        let datalogic = Arc::new(DataLogic::with_preserve_structure());

        let mut message = Message::new(Arc::new(json!({})));

        // Config with no logic_index set (uncompiled)
        let config = ValidationConfig {
            rules: vec![ValidationRule {
                logic: json!(true),
                message: "Test".to_string(),
                logic_index: None, // Not compiled
            }],
        };

        let logic_cache = Vec::new();
        let result = config.execute(&mut message, &datalogic, &logic_cache);
        assert!(result.is_ok());

        let (status, _) = result.unwrap();
        assert_eq!(status, 400); // Should fail due to compilation error
        assert!(!message.errors.is_empty());
        assert!(message.errors[0].code == "COMPILATION_ERROR");
    }
}
