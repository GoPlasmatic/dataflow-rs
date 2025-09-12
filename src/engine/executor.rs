//! # Internal Function Execution Module
//!
//! This module handles the efficient execution of built-in functions (map and validation)
//! using pre-compiled logic from DataLogic v4. It provides optimized execution paths for:
//!
//! - Data transformations with JSONLogic mappings
//! - Rule-based validation with custom error messages
//! - Efficient condition evaluation for workflows and tasks
//! - Thread-safe execution using Arc<CompiledLogic>

use crate::engine::error::Result;
use crate::engine::functions::{MapConfig, ValidationConfig};
use crate::engine::message::{Change, Message};
use crate::engine::utils::is_truthy;
use datalogic_rs::{CompiledLogic, DataLogic};
use log::error;
use serde_json::Value;
use std::sync::Arc;

/// Executes internal functions using pre-compiled logic for optimal performance.
///
/// The `InternalExecutor` provides:
/// - Efficient execution of map transformations using compiled logic
/// - Fast validation rule evaluation with detailed error reporting
/// - Condition evaluation for workflow and task control flow
/// - Thread-safe operation via Arc-wrapped compiled logic
pub struct InternalExecutor {
    /// Shared DataLogic instance for evaluation
    datalogic: Arc<DataLogic>,
    /// Reference to the compiled logic cache
    logic_cache: Vec<Arc<CompiledLogic>>,
}

impl InternalExecutor {
    /// Create a new InternalExecutor with DataLogic v4
    pub fn new(datalogic: Arc<DataLogic>, logic_cache: Vec<Arc<CompiledLogic>>) -> Self {
        Self {
            datalogic,
            logic_cache,
        }
    }

    /// Get a reference to the DataLogic instance
    pub fn datalogic(&self) -> &Arc<DataLogic> {
        &self.datalogic
    }

    /// Get a reference to the logic cache
    pub fn logic_cache(&self) -> &Vec<Arc<CompiledLogic>> {
        &self.logic_cache
    }

    /// Execute the internal map function with optimized data handling
    pub fn execute_map(
        &self,
        message: &mut Message,
        config: &MapConfig,
    ) -> Result<(usize, Vec<Change>)> {
        config.execute(message, &self.datalogic, &self.logic_cache)
    }

    /// Execute the internal validation function
    pub fn execute_validation(
        &self,
        message: &mut Message,
        config: &ValidationConfig,
    ) -> Result<(usize, Vec<Change>)> {
        config.execute(message, &self.datalogic, &self.logic_cache)
    }

    /// Evaluate a condition using cached compiled logic
    /// The data passed here should be the metadata field for workflow/task conditions
    pub fn evaluate_condition(
        &self,
        condition_index: Option<usize>,
        metadata: Arc<Value>,
    ) -> Result<bool> {
        match condition_index {
            Some(index) if index < self.logic_cache.len() => {
                let compiled_logic = &self.logic_cache[index];
                // Conditions typically evaluate against metadata directly
                let result = self.datalogic.evaluate(compiled_logic, metadata);

                match result {
                    Ok(value) => Ok(is_truthy(&value)),
                    Err(e) => {
                        error!("Failed to evaluate condition: {:?}", e);
                        Ok(false)
                    }
                }
            }
            _ => Ok(true), // No condition means always true
        }
    }
}
