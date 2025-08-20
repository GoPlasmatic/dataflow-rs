use crate::engine::AsyncFunctionHandler;
use crate::engine::error::{DataflowError, ErrorInfo, Result};
use crate::engine::functions::FUNCTION_DATA_LOGIC;
use crate::engine::message::{Change, Message};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::vec;
/// A validation task function that uses JsonLogic expressions to validate message data.
///
/// This function executes validation rules defined in JsonLogic against data in the message,
/// and reports validation failures by adding errors to the message's errors array.
pub struct ValidationFunction {
    // No longer needs data_logic field
}

// SAFETY: These implementations are technically unsound because DataLogic contains
// RefCell and Cell which are not thread-safe. In practice, we'll ensure that
// ValidationTask is only used in a single-threaded context, or we'll use thread-local
// instances of DataLogic.
unsafe impl Send for ValidationFunction {}
unsafe impl Sync for ValidationFunction {}

impl Default for ValidationFunction {
    fn default() -> Self {
        Self::new()
    }
}

impl ValidationFunction {
    pub fn new() -> Self {
        Self { /* no fields */ }
    }
}

#[async_trait]
impl AsyncFunctionHandler for ValidationFunction {
    async fn execute(&self, message: &mut Message, input: &Value) -> Result<(usize, Vec<Change>)> {
        // Extract rules from input
        let rules = input
            .get("rules")
            .ok_or_else(|| DataflowError::Validation("Missing rules array".to_string()))?;

        // Use thread-local DataLogic

        FUNCTION_DATA_LOGIC.with(|data_logic_cell| {
            let mut data_logic = data_logic_cell.borrow_mut();
            data_logic.reset_arena();

            if let Some(rules_arr) = rules.as_array() {
                for rule in rules_arr {
                    // Extract rule components
                    let rule_logic = rule.get("logic").ok_or_else(|| {
                        DataflowError::Validation("Missing logic in rule".to_string())
                    })?;

                    let rule_path = rule.get("path").and_then(Value::as_str).unwrap_or("data");

                    let data_to_validate = if rule_path == "data" || rule_path.starts_with("data.")
                    {
                        &json!({"data": message.data})
                    } else if rule_path == "metadata" || rule_path.starts_with("metadata.") {
                        &json!({"metadata": message.metadata})
                    } else if rule_path == "temp_data" || rule_path.starts_with("temp_data.") {
                        &json!({"temp_data": message.temp_data})
                    } else {
                        &json!({"data": message.data})
                    };

                    // Evaluate the rule
                    match data_logic.evaluate_json(rule_logic, data_to_validate, None) {
                        Ok(v) => {
                            if !v.as_bool().unwrap_or(false) {
                                let message_key = rule
                                    .get("message")
                                    .and_then(Value::as_str)
                                    .unwrap_or("Validation failed")
                                    .to_string();

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
        })
    }
}
