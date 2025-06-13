use crate::engine::error::{DataflowError, Result};
use crate::engine::functions::FUNCTION_DATA_LOGIC;
use crate::engine::message::{Change, Message};
use crate::engine::AsyncFunctionHandler;
use async_trait::async_trait;
use serde_json::{json, Value};
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
        let validation_result = FUNCTION_DATA_LOGIC.with(|data_logic_cell| {
            let mut data_logic = data_logic_cell.borrow_mut();
            data_logic.reset_arena();

            if let Some(rules_arr) = rules.as_array() {
                for rule in rules_arr {
                    // Extract rule components
                    let rule_logic = rule.get("logic").ok_or_else(|| {
                        DataflowError::Validation("Missing logic in rule".to_string())
                    })?;

                    let rule_path = rule.get("path").and_then(Value::as_str).unwrap_or("data");

                    let data_to_validate = if rule_path == "data" {
                        &json!({rule_path: message.data})
                    } else if rule_path == "metadata" {
                        &json!({rule_path: message.metadata})
                    } else if rule_path == "temp_data" {
                        &json!({rule_path: message.temp_data})
                    } else {
                        &json!({rule_path: message.data})
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

                                println!("Validation failed: {}", message_key);
                                return Ok((400, vec![]));
                            }
                        }
                        Err(e) => {
                            println!("Error evaluating rule: {}", e);
                            return Err(DataflowError::LogicEvaluation(format!(
                                "Error evaluating rule: {}",
                                e
                            )));
                        }
                    }
                }
            }

            // Only create changes if there are actual validation results to report
            // Don't create a change from null to null as this causes duplicate audit entries
            let changes = if message.temp_data.get("validation").is_some() && 
                         !message.temp_data["validation"].is_null() {
                vec![Change {
                    path: "temp_data.validation".to_string(),
                    old_value: Value::Null,
                    new_value: message.temp_data["validation"].clone(),
                }]
            } else {
                // No validation results to record - return empty changes
                vec![]
            };

            Ok((200, changes))
        });

        validation_result
    }
}
