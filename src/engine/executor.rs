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
//!
//! `ArenaContext` (below) extends this further for **mutating** tasks (map):
//! the message context is `to_arena`'d once per task call into a depth‑2
//! cache, and subsequent writes only re‑arena the dirtied subtree — typically
//! `data.MT103` while the heavy `data.input` stays cached.

use crate::engine::error::Result;
use crate::engine::functions::{FilterConfig, LogConfig, MapConfig, ValidationConfig};
use crate::engine::message::{Change, Message};
use bumpalo::Bump;
use datalogic_rs::{Engine, Logic};
use datavalue::{DataValue, OwnedDataValue};
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

/// Depth‑2 arena cache for a `Message.context` (always an
/// `OwnedDataValue::Object`).
///
/// Built once at the top of a mutating task call, then mutated in place as
/// the task writes back into `message.context`. Writes at path `a.b.X`
/// invalidate only the `(a, b)` arena slot — `data.input` stays cached
/// across the entire map task even while `data.MT103.*` is being written.
///
/// **Lifetime model.** All arena allocations come out of the borrowed `Bump`.
/// `top_keys` / `top_values` / `depth2` are owned `Vec`s so we can mutate
/// them freely; the `DataValue<'a>` slice handed to `engine.evaluate` is a
/// fresh `arena.alloc_slice_copy` per call, so it stays valid for that eval
/// regardless of subsequent mutations.
pub(crate) struct ArenaContext<'a> {
    arena: &'a Bump,
    /// Top-level slot keys, arena-allocated `&'a str`.
    top_keys: Vec<&'a str>,
    /// Top-level slot values. When a slot's owned value is an `Object`, the
    /// corresponding `top_values[i]` is `DataValue::Object(&'a [...])` whose
    /// slice was minted from `depth2[i]` via `alloc_slice_copy`. When not an
    /// Object, `depth2[i] = None` and `top_values[i]` is the full arena form.
    top_values: Vec<DataValue<'a>>,
    /// Depth‑2 cache, parallel to `top_keys`. `None` for non‑Object top slots.
    depth2: Vec<Option<Depth2Cache<'a>>>,
}

struct Depth2Cache<'a> {
    keys: Vec<&'a str>,
    values: Vec<DataValue<'a>>,
}

impl<'a> ArenaContext<'a> {
    /// Build from an `OwnedDataValue` context (which should be the canonical
    /// `Object { data, metadata, temp_data }` shape). Deep-walks the owned
    /// tree exactly once; subsequent reads / mutations are O(touched slot).
    pub fn from_owned(ctx: &OwnedDataValue, arena: &'a Bump) -> Self {
        let mut top_keys: Vec<&'a str> = Vec::with_capacity(4);
        let mut top_values: Vec<DataValue<'a>> = Vec::with_capacity(4);
        let mut depth2: Vec<Option<Depth2Cache<'a>>> = Vec::with_capacity(4);

        if let OwnedDataValue::Object(pairs) = ctx {
            for (k, v) in pairs {
                top_keys.push(arena.alloc_str(k));
                match v {
                    OwnedDataValue::Object(children) => {
                        let mut d2_keys: Vec<&'a str> = Vec::with_capacity(children.len());
                        let mut d2_vals: Vec<DataValue<'a>> = Vec::with_capacity(children.len());
                        for (ck, cv) in children {
                            d2_keys.push(arena.alloc_str(ck));
                            d2_vals.push(cv.to_arena(arena));
                        }
                        let slice = build_object_slice(arena, &d2_keys, &d2_vals);
                        top_values.push(DataValue::Object(slice));
                        depth2.push(Some(Depth2Cache {
                            keys: d2_keys,
                            values: d2_vals,
                        }));
                    }
                    _ => {
                        top_values.push(v.to_arena(arena));
                        depth2.push(None);
                    }
                }
            }
        }

        Self {
            arena,
            top_keys,
            top_values,
            depth2,
        }
    }

    /// Build an arena `DataValue::Object` for the current state. The returned
    /// slice is freshly allocated in the arena and stays valid for the caller
    /// to pass into `engine.evaluate`; later mutations on `self` allocate a
    /// new slice on the next call.
    pub fn as_data_value(&self) -> DataValue<'a> {
        let slice = build_object_slice(self.arena, &self.top_keys, &self.top_values);
        DataValue::Object(slice)
    }

    /// Borrow the underlying arena — needed by callers that want to allocate
    /// or evaluate into the same `Bump` (e.g. `engine.evaluate(...)`).
    #[inline]
    pub fn arena(&self) -> &'a Bump {
        self.arena
    }

    /// Apply an owned write at `path` (pre-split into `parts`) to *both* the
    /// underlying `OwnedDataValue` context (via the supplied closure that
    /// performs the in-place mutation) and the arena cache. Skips the runtime
    /// `str::split` that shows up in profiles as `CharSearcher::next_match`.
    pub fn apply_mutation_parts(
        &mut self,
        owned_ctx: &mut OwnedDataValue,
        parts: &[Arc<str>],
        apply: impl FnOnce(&mut OwnedDataValue),
    ) {
        apply(owned_ctx);
        self.refresh_after_write_parts(owned_ctx, parts);
    }

    /// Refresh the arena slot(s) for `path` from the current `owned_ctx`,
    /// without applying any new write. Used when a sync task mutated
    /// `message.context` directly (e.g. `parse_json` going through legacy
    /// helpers) and we need the arena to catch up.
    pub fn refresh_for_path(&mut self, owned_ctx: &OwnedDataValue, path: &str) {
        self.refresh_after_write(owned_ctx, path);
    }

    /// Pre-split variant of `refresh_after_write` — same algorithm, no
    /// per-call `str::split` walk. `parts` retains the original `#` prefix;
    /// the hash strip is applied here at lookup so the cache key matches
    /// what `set_nested_value_parts` actually wrote.
    fn refresh_after_write_parts(&mut self, owned_ctx: &OwnedDataValue, parts: &[Arc<str>]) {
        let top_raw: &str = match parts.first() {
            Some(p) if !p.is_empty() => p,
            _ => {
                self.rebuild_all_from(owned_ctx);
                return;
            }
        };
        let top = top_raw.strip_prefix('#').unwrap_or(top_raw);
        fn strip<'p>(p: &'p Arc<str>) -> &'p str {
            let s: &'p str = p;
            s.strip_prefix('#').unwrap_or(s)
        }
        let depth2_key: Option<&str> = parts.get(1).map(strip);
        let depth3_key: Option<&str> = parts.get(2).map(strip);
        self.refresh_after_write_inner(owned_ctx, top, depth2_key, depth3_key);
    }

    /// Refresh the arena cache after `owned_ctx` was mutated at `path`.
    fn refresh_after_write(&mut self, owned_ctx: &OwnedDataValue, path: &str) {
        let mut parts = path.split('.');
        let top_raw = match parts.next() {
            Some(p) if !p.is_empty() => p,
            _ => {
                self.rebuild_all_from(owned_ctx);
                return;
            }
        };
        let top = top_raw.strip_prefix('#').unwrap_or(top_raw);
        let depth2_key = parts.next().map(|p| p.strip_prefix('#').unwrap_or(p));
        let depth3_key = parts.next().map(|p| p.strip_prefix('#').unwrap_or(p));
        self.refresh_after_write_inner(owned_ctx, top, depth2_key, depth3_key);
    }

    /// Shared body: walk the cache for `top` and optional `depth2_key`,
    /// rebuilding only the dirtied slot. `depth3_key` is ignored (the
    /// depth-3 sub-cache was tried but regressed on the realistic workload —
    /// per-write d3 cache thrashing exceeded the savings).
    fn refresh_after_write_inner(
        &mut self,
        owned_ctx: &OwnedDataValue,
        top: &str,
        depth2_key: Option<&str>,
        _depth3_key: Option<&str>,
    ) {

        let OwnedDataValue::Object(owned_pairs) = owned_ctx else {
            self.rebuild_all_from(owned_ctx);
            return;
        };

        let owned_top_val = owned_pairs.iter().find(|(k, _)| k == top).map(|(_, v)| v);

        let top_idx = self.top_keys.iter().position(|k| *k == top);

        match (owned_top_val, top_idx) {
            (None, Some(idx)) => {
                // Top slot was removed from owned ctx — remove from cache.
                self.top_keys.remove(idx);
                self.top_values.remove(idx);
                self.depth2.remove(idx);
            }
            (Some(new_val), idx_opt) => {
                let idx = match idx_opt {
                    Some(i) => i,
                    None => {
                        self.top_keys.push(self.arena.alloc_str(top));
                        self.top_values.push(DataValue::Null);
                        self.depth2.push(None);
                        self.top_keys.len() - 1
                    }
                };

                match (new_val, depth2_key, &mut self.depth2[idx]) {
                    // Depth-2 write into an existing Object top slot that already
                    // has a depth-2 cache → refresh only the child.
                    (
                        OwnedDataValue::Object(new_children),
                        Some(child_key),
                        Some(d2),
                    ) => {
                        if let Some(new_child) = new_children
                            .iter()
                            .find(|(k, _)| k == child_key)
                            .map(|(_, v)| v)
                        {
                            // Replace or insert the single child slot.
                            let child_arena = new_child.to_arena(self.arena);
                            if let Some(pos) =
                                d2.keys.iter().position(|k| *k == child_key)
                            {
                                d2.values[pos] = child_arena;
                            } else {
                                d2.keys.push(self.arena.alloc_str(child_key));
                                d2.values.push(child_arena);
                            }
                            // Also reflect deletions of *other* depth-2 keys
                            // (rare but possible if the write replaced the
                            // whole top object). Cheap O(n) scan.
                            if d2.keys.len() != new_children.len() {
                                // Owned children diverged from our cache —
                                // rebuild the depth-2 cache from owned.
                                self.rebuild_top_slot(owned_top_val.unwrap(), idx);
                                return;
                            }
                        } else {
                            // child_key not found in new owned object — child
                            // was removed. Drop from cache.
                            if let Some(pos) =
                                d2.keys.iter().position(|k| *k == child_key)
                            {
                                d2.keys.remove(pos);
                                d2.values.remove(pos);
                            }
                        }
                        let slice =
                            build_object_slice(self.arena, &d2.keys, &d2.values);
                        self.top_values[idx] = DataValue::Object(slice);
                    }
                    // Top-level (depth-1) write or shape change → rebuild
                    // the whole top slot (cheap relative to a full ctx walk).
                    _ => {
                        self.rebuild_top_slot(new_val, idx);
                    }
                }
            }
            (None, None) => { /* no-op */ }
        }
    }

    fn rebuild_top_slot(&mut self, owned: &OwnedDataValue, idx: usize) {
        match owned {
            OwnedDataValue::Object(children) => {
                let mut d2_keys: Vec<&'a str> = Vec::with_capacity(children.len());
                let mut d2_vals: Vec<DataValue<'a>> = Vec::with_capacity(children.len());
                for (ck, cv) in children {
                    d2_keys.push(self.arena.alloc_str(ck));
                    d2_vals.push(cv.to_arena(self.arena));
                }
                let slice = build_object_slice(self.arena, &d2_keys, &d2_vals);
                self.top_values[idx] = DataValue::Object(slice);
                self.depth2[idx] = Some(Depth2Cache {
                    keys: d2_keys,
                    values: d2_vals,
                });
            }
            _ => {
                self.top_values[idx] = owned.to_arena(self.arena);
                self.depth2[idx] = None;
            }
        }
    }

    /// Last-resort: ditch all cached state and rebuild from scratch. Should be
    /// rare on normal flows — only triggered if the context shape changes in
    /// a way the depth-2 cache can't track.
    fn rebuild_all_from(&mut self, ctx: &OwnedDataValue) {
        let rebuilt = ArenaContext::from_owned(ctx, self.arena);
        self.top_keys = rebuilt.top_keys;
        self.top_values = rebuilt.top_values;
        self.depth2 = rebuilt.depth2;
    }
}

/// Allocate a fresh `(key, value)` slice in the arena. Each
/// `engine.evaluate` call gets its own slice; subsequent mutations to the
/// underlying Vecs are independent.
fn build_object_slice<'a>(
    arena: &'a Bump,
    keys: &[&'a str],
    values: &[DataValue<'a>],
) -> &'a [(&'a str, DataValue<'a>)] {
    debug_assert_eq!(keys.len(), values.len());
    arena.alloc_slice_fill_iter(keys.iter().zip(values.iter()).map(|(k, v)| (*k, *v)))
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

    /// Same as `evaluate_condition` but evaluates against an arena-resident
    /// `DataValue` and an existing `Bump`. Used inside a `with_arena` block
    /// (the workflow sync-stretch path) to avoid re-entering the
    /// thread-local arena `RefCell::borrow_mut`.
    pub fn evaluate_condition_in_arena(
        &self,
        condition_index: Option<usize>,
        ctx: DataValue<'_>,
        arena: &Bump,
    ) -> Result<bool> {
        match condition_index {
            Some(index) if index < self.logic_cache.len() => {
                let compiled = &self.logic_cache[index];
                match self.engine.evaluate(compiled, ctx, arena) {
                    Ok(value) => Ok(matches!(value, DataValue::Bool(true))),
                    Err(e) => {
                        error!("Failed to evaluate condition: {:?}", e);
                        Ok(false)
                    }
                }
            }
            _ => Ok(true),
        }
    }
}
