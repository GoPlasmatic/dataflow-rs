use crate::engine::error::{DataflowError, Result};
use crate::engine::message::{Change, Message};
use crate::engine::FunctionHandler;
use datalogic_rs::DataLogic;
use serde_json::Value;
use std::sync::{Arc, Mutex};
/// A validation task that evaluates JSONLogic rules against message content.
///
/// This task allows validating message structure and content against business rules
/// expressed using JSONLogic expressions. It returns validation results that can be
/// used for conditional processing and routing logic.
pub struct ValidationFunction {
    /// Reference to DataLogic instance for parsing rules
    data_logic: Arc<Mutex<DataLogic>>,
}

// SAFETY: These implementations are technically unsound because DataLogic contains
// RefCell and Cell which are not thread-safe. In practice, we'll ensure that
// ValidationTask is only used in a single-threaded context, or we'll use thread-local
// instances of DataLogic.
unsafe impl Send for ValidationFunction {}
unsafe impl Sync for ValidationFunction {}

impl ValidationFunction {
    pub fn new(data_logic: Arc<Mutex<DataLogic>>) -> Self {
        Self { data_logic }
    }
}

impl FunctionHandler for ValidationFunction {
    fn execute(&self, message: &mut Message, input: &Value) -> Result<(usize, Vec<Change>)> {
        // Extract validation configuration from the input
        let rule_value = match input.get("rule") {
            Some(rule) => rule,
            None => {
                return Err(DataflowError::Validation(
                    "Validation rule not provided".to_string(),
                ))
            }
        };

        let rule_result = self
            .data_logic
            .lock()
            .map_err(|_| DataflowError::Unknown("Failed to acquire data_logic lock".to_string()))?
            .evaluate_json(rule_value, &message.data, None)
            .map_err(|e| {
                DataflowError::LogicEvaluation(format!("Failed to evaluate rule: {}", e))
            })?;

        // Convert result to boolean
        let is_valid = rule_result.as_bool().unwrap_or(false);

        Ok((if is_valid { 200 } else { 400 }, vec![]))
    }
}
