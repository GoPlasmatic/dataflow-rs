//! # Internal Function Execution Module
//!
//! Executes built-in functions (map, validation, filter, log) using pre-compiled
//! `Arc<Logic>` produced by the workflow compiler. Each evaluation goes through
//! the datalogic v5 `Engine::evaluate` API. The bump arena is owned by a
//! thread-local pool so multi-million-message workloads amortise allocation
//! across the entire lifetime of each Tokio worker thread.

use crate::engine::error::Result;
use crate::engine::functions::{FilterConfig, LogConfig, MapConfig, ValidationConfig};
use crate::engine::message::{Change, Message};
use bumpalo::Bump;
use datalogic_rs::{Engine, FromDataValue, Logic};
use log::error;
use serde_json::Value;
use std::cell::RefCell;
use std::sync::Arc;

thread_local! {
    /// One arena per Tokio worker thread. `Engine` and `Arc<Logic>` are
    /// `Send + Sync` and shared across threads; `Bump` is `!Send` and lives
    /// here so each thread gets its own zero-contention scratch space.
    /// `with_capacity(16 * 1024)` pre-sizes the initial chunk to cover a
    /// typical workflow's per-call high-water mark without an early growth
    /// event. The chunk is rewound (not freed) between evaluations via
    /// `Bump::reset`, so subsequent calls reuse the same memory.
    static EVAL_ARENA: RefCell<Bump> = RefCell::new(Bump::with_capacity(16 * 1024));
}

/// Evaluate a pre-compiled logic node against `context`, projecting the
/// result back into an owned `serde_json::Value`.
///
/// Uses a thread-local arena (one `Bump` per Tokio worker thread). The arena
/// is rewound after every call so peak memory stays bounded by the single
/// largest evaluation, while the chunks themselves persist across calls —
/// no per-call OS allocation in steady state.
#[inline]
pub(crate) fn eval_to_json(
    engine: &Engine,
    compiled: &Logic,
    context: &Value,
) -> std::result::Result<Value, datalogic_rs::Error> {
    EVAL_ARENA.with(|cell| {
        let mut arena = cell.borrow_mut();
        // Reset *before* the call, not after — this way any borrow into the
        // arena returned by `engine.evaluate` lives strictly inside this
        // block, and the chunks from the previous call are reused.
        arena.reset();
        let result = {
            let r = engine.evaluate(compiled, context, &arena)?;
            Value::from_arena(r)?
        };
        Ok(result)
    })
}

/// Executes internal functions using pre-compiled logic for optimal performance.
pub struct InternalExecutor {
    /// Shared datalogic Engine for evaluation (`Send + Sync`, `Arc`-shared
    /// across all Tokio worker threads).
    engine: Arc<Engine>,
    /// Reference to the compiled logic cache (each entry is an `Arc<Logic>`
    /// so cheap clones move only the smart pointer, never the compiled tree).
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

    /// Execute the internal map function with optimized data handling
    pub fn execute_map(
        &self,
        message: &mut Message,
        config: &MapConfig,
    ) -> Result<(usize, Vec<Change>)> {
        config.execute(message, &self.engine, &self.logic_cache)
    }

    /// Execute the internal map function with trace support (captures per-mapping context snapshots)
    pub fn execute_map_with_trace(
        &self,
        message: &mut Message,
        config: &MapConfig,
    ) -> Result<(usize, Vec<Change>, Vec<Value>)> {
        config.execute_with_trace(message, &self.engine, &self.logic_cache)
    }

    /// Execute the internal validation function
    pub fn execute_validation(
        &self,
        message: &mut Message,
        config: &ValidationConfig,
    ) -> Result<(usize, Vec<Change>)> {
        config.execute(message, &self.engine, &self.logic_cache)
    }

    /// Execute the internal log function
    pub fn execute_log(
        &self,
        message: &mut Message,
        config: &LogConfig,
    ) -> Result<(usize, Vec<Change>)> {
        config.execute(message, &self.engine, &self.logic_cache)
    }

    /// Execute the internal filter function
    pub fn execute_filter(
        &self,
        message: &mut Message,
        config: &FilterConfig,
    ) -> Result<(usize, Vec<Change>)> {
        config.execute(message, &self.engine, &self.logic_cache)
    }

    /// Evaluate a workflow or task condition using the cached compiled logic.
    /// The supplied context covers `data`, `metadata`, `temp_data`, and `payload`.
    pub fn evaluate_condition(
        &self,
        condition_index: Option<usize>,
        context: Arc<Value>,
    ) -> Result<bool> {
        match condition_index {
            Some(index) if index < self.logic_cache.len() => {
                let compiled = &self.logic_cache[index];
                match eval_to_json(&self.engine, compiled, &context) {
                    Ok(value) => Ok(value == Value::Bool(true)),
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
