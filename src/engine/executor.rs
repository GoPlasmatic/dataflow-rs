//! # Internal Function Execution Module
//!
//! Executes built-in functions (map, validation, filter, log) using pre-compiled
//! `Arc<Logic>` produced by the workflow compiler. Each evaluation goes through
//! the datalogic v5 `Engine::evaluate` API directly against an `&OwnedDataValue`
//! context — no `serde_json::Value` intermediate on the hot path.
//!
//! The bump arena is held in a thread-local cell on each Tokio worker. Per
//! call, the arena is rewound via `Bump::reset` (constant-time, retains chunks)
//! before the eval, then evaluated against. Chunks accumulate to fit the
//! workload's high-water mark and persist across calls — no per-task
//! malloc/free churn. Profiling showed per-task `Bump::with_capacity` malloc
//! was the dominant cost when arena sizing was tuned for realistic workloads;
//! thread-local reuse amortizes that to zero in steady state.

use crate::engine::error::Result;
use crate::engine::functions::{FilterConfig, LogConfig, MapConfig, ValidationConfig};
use crate::engine::message::{Change, Message};
use bumpalo::Bump;
use datalogic_rs::{Engine, Logic};
use datavalue::OwnedDataValue;
use log::error;
use serde_json::Value;
use std::cell::RefCell;
use std::sync::Arc;

/// Initial bump arena capacity per worker thread. Sized to cover a realistic
/// ISO-20022-shaped payload's `to_arena` deep-clone in one shot, so the first
/// few calls on each thread don't trigger `Bump::new_chunk`. After that the
/// chunks persist across calls and the capacity is irrelevant.
const ARENA_INITIAL_CAPACITY: usize = 128 * 1024;

thread_local! {
    /// Per-worker-thread bump arena. `Engine` and `Arc<Logic>` are `Send + Sync`
    /// and shared across threads; `Bump` is `!Send` so it lives here for
    /// zero-contention scratch space. Chunks accumulate over the thread's
    /// lifetime and `reset()` rewinds the pointer without freeing chunks back
    /// to the OS — steady-state allocator pressure is zero.
    static EVAL_ARENA: RefCell<Bump> = RefCell::new(Bump::with_capacity(ARENA_INITIAL_CAPACITY));
}

/// Evaluate `compiled` against `context` using the worker thread's bump
/// arena, returning the result as an owned `OwnedDataValue`. The arena is
/// rewound before the call so peak memory is bounded by the single largest
/// evaluation; chunks persist across calls so steady-state allocation is zero.
///
/// Use this for one-shot evals where the context isn't reused across
/// multiple JSONLogic calls (e.g. a single condition check). For batches of
/// read-only evals against the same context (validation, log) use
/// [`with_arena`] and convert the context once via
/// [`datavalue::OwnedDataValue::to_arena`].
#[inline]
pub(crate) fn eval_to_owned(
    engine: &Engine,
    compiled: &Logic,
    context: &OwnedDataValue,
) -> std::result::Result<OwnedDataValue, datalogic_rs::Error> {
    EVAL_ARENA.with(|cell| {
        let mut arena = cell.borrow_mut();
        arena.reset();
        let r = engine.evaluate(compiled, context, &arena)?;
        Ok(r.to_owned())
    })
}

/// Run `f` with the worker thread's bump arena rewound. The closure receives
/// the `Bump` and can amortize work across multiple `engine.evaluate` calls
/// by converting the input context to `DataValue` once and reusing it. Use
/// this for batches of read-only evals against the same context (validation,
/// log) — it skips the per-eval `to_arena` deep-clone that dominates
/// realistic profile.
#[inline]
pub(crate) fn with_arena<R>(f: impl FnOnce(&Bump) -> R) -> R {
    EVAL_ARENA.with(|cell| {
        let mut arena = cell.borrow_mut();
        arena.reset();
        f(&arena)
    })
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
        config.execute(message, &self.engine, &self.logic_cache)
    }

    /// Execute the internal map function with trace support.
    pub fn execute_map_with_trace(
        &self,
        message: &mut Message,
        config: &MapConfig,
    ) -> Result<(usize, Vec<Change>, Vec<Value>)> {
        config.execute_with_trace(message, &self.engine, &self.logic_cache)
    }

    /// Execute the internal validation function.
    pub fn execute_validation(
        &self,
        message: &mut Message,
        config: &ValidationConfig,
    ) -> Result<(usize, Vec<Change>)> {
        config.execute(message, &self.engine, &self.logic_cache)
    }

    /// Execute the internal log function.
    pub fn execute_log(
        &self,
        message: &mut Message,
        config: &LogConfig,
    ) -> Result<(usize, Vec<Change>)> {
        config.execute(message, &self.engine, &self.logic_cache)
    }

    /// Execute the internal filter function.
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
        context: &OwnedDataValue,
    ) -> Result<bool> {
        match condition_index {
            Some(index) if index < self.logic_cache.len() => {
                let compiled = &self.logic_cache[index];
                match eval_to_owned(&self.engine, compiled, context) {
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
