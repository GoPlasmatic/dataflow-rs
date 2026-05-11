//! # Internal Function Execution Module
//!
//! Executes built-in functions (map, validation, filter, log) using pre-compiled
//! `Arc<Logic>` produced by the workflow compiler. Each evaluation goes through
//! the datalogic v5 `Engine::evaluate` API directly against an `&OwnedDataValue`
//! context — no `serde_json::Value` intermediate on the hot path.
//!
//! The bump arena is owned per-call: each `InternalExecutor::execute_*` entry
//! constructs a fresh `Bump::with_capacity(16 * 1024)` and passes it down to
//! the underlying function config. The arena drops at end of call, returning
//! its chunks to the global allocator's free-list. Sized to cover a typical
//! workflow's per-call high-water mark without an early growth event.

use crate::engine::error::Result;
use crate::engine::functions::{FilterConfig, LogConfig, MapConfig, ValidationConfig};
use crate::engine::message::{Change, Message};
use bumpalo::Bump;
use datalogic_rs::{Engine, Logic};
use datavalue::OwnedDataValue;
use log::error;
use serde_json::Value;
use std::sync::Arc;

/// Default pre-allocated arena capacity for one task call.
const ARENA_CAPACITY: usize = 16 * 1024;

/// Evaluate `compiled` against `context` using the supplied arena, returning
/// the result as an owned `OwnedDataValue` (no `serde_json::Value` round-trip).
#[inline]
pub(crate) fn eval_to_owned(
    engine: &Engine,
    compiled: &Logic,
    context: &OwnedDataValue,
    arena: &Bump,
) -> std::result::Result<OwnedDataValue, datalogic_rs::Error> {
    let r = engine.evaluate(compiled, context, arena)?;
    Ok(r.to_owned())
}

/// Executes internal functions using pre-compiled logic for optimal performance.
pub struct InternalExecutor {
    /// Shared datalogic Engine for evaluation (`Send + Sync`).
    engine: Arc<Engine>,
    /// Reference to the compiled logic cache.
    logic_cache: Vec<Arc<Logic>>,
}

impl InternalExecutor {
    /// Create a new InternalExecutor wired to a shared v5 Engine.
    pub fn new(engine: Arc<Engine>, logic_cache: Vec<Arc<Logic>>) -> Self {
        Self {
            engine,
            logic_cache,
        }
    }

    /// Get a reference to the datalogic Engine instance
    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    /// Get a reference to the logic cache
    pub fn logic_cache(&self) -> &Vec<Arc<Logic>> {
        &self.logic_cache
    }

    /// Execute the internal map function.
    pub fn execute_map(
        &self,
        message: &mut Message,
        config: &MapConfig,
    ) -> Result<(usize, Vec<Change>)> {
        let arena = Bump::with_capacity(ARENA_CAPACITY);
        config.execute(message, &self.engine, &self.logic_cache, &arena)
    }

    /// Execute the internal map function with trace support.
    pub fn execute_map_with_trace(
        &self,
        message: &mut Message,
        config: &MapConfig,
    ) -> Result<(usize, Vec<Change>, Vec<Value>)> {
        let arena = Bump::with_capacity(ARENA_CAPACITY);
        config.execute_with_trace(message, &self.engine, &self.logic_cache, &arena)
    }

    /// Execute the internal validation function.
    pub fn execute_validation(
        &self,
        message: &mut Message,
        config: &ValidationConfig,
    ) -> Result<(usize, Vec<Change>)> {
        let arena = Bump::with_capacity(ARENA_CAPACITY);
        config.execute(message, &self.engine, &self.logic_cache, &arena)
    }

    /// Execute the internal log function.
    pub fn execute_log(
        &self,
        message: &mut Message,
        config: &LogConfig,
    ) -> Result<(usize, Vec<Change>)> {
        let arena = Bump::with_capacity(ARENA_CAPACITY);
        config.execute(message, &self.engine, &self.logic_cache, &arena)
    }

    /// Execute the internal filter function.
    pub fn execute_filter(
        &self,
        message: &mut Message,
        config: &FilterConfig,
    ) -> Result<(usize, Vec<Change>)> {
        let arena = Bump::with_capacity(ARENA_CAPACITY);
        config.execute(message, &self.engine, &self.logic_cache, &arena)
    }

    /// Evaluate a workflow or task condition using the cached compiled logic.
    /// The supplied context covers `data`, `metadata`, `temp_data`, and `payload`.
    pub fn evaluate_condition(
        &self,
        condition_index: Option<usize>,
        context: &OwnedDataValue,
    ) -> Result<bool> {
        match condition_index {
            Some(index) if index < self.logic_cache.len() => {
                let compiled = &self.logic_cache[index];
                let arena = Bump::with_capacity(ARENA_CAPACITY);
                match eval_to_owned(&self.engine, compiled, context, &arena) {
                    Ok(value) => Ok(matches!(value, OwnedDataValue::Bool(true))),
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
