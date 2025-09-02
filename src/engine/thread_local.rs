use std::cell::RefCell;
use datalogic_rs::DataLogic;
use serde_json::Value;

use crate::engine::error::{DataflowError, Result};

// Thread-local storage for DataLogic instance and compiled workflows
thread_local! {
    // Single DataLogic instance per thread with 'static lifetime
    static DATA_LOGIC: RefCell<DataLogic<'static>> = RefCell::new(
        DataLogic::with_preserve_structure()
    );
}

/// Execute a function with the thread-local DataLogic instance
pub fn with_datalogic<F, R>(f: F) -> R
where
    F: FnOnce(&DataLogic) -> R,
{
    DATA_LOGIC.with(|dl| f(&dl.borrow()))
}

/// Evaluate a JSON logic expression using the thread-local DataLogic
pub fn evaluate_json(logic: &Value, data: &Value) -> Result<Value> {
    DATA_LOGIC.with(|dl| {
        let result = dl.borrow()
            .evaluate_json(logic, data)
            .map_err(|e| DataflowError::LogicEvaluation(format!("Error evaluating logic: {}", e)));
        
        // Debug logging
        if std::env::var("DEBUG_LOGIC").is_ok() {
            eprintln!("Logic evaluation:");
            eprintln!("  Logic: {}", logic);
            eprintln!("  Data: {}", data);
            eprintln!("  Result: {:?}", result);
        }
        
        result
    })
}

/// Evaluate a condition using thread-local DataLogic
pub fn evaluate_condition(condition: &Value, data: &Value) -> Result<bool> {
    // Short-circuit for simple boolean conditions
    if let Value::Bool(b) = condition {
        return Ok(*b);
    }
    
    DATA_LOGIC.with(|dl| {
        dl.borrow()
            .evaluate_json(condition, data)
            .map(|result| result.as_bool().unwrap_or(false))
            .map_err(|e| DataflowError::LogicEvaluation(format!("Error evaluating condition: {}", e)))
    })
}