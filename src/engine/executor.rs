//! # Evaluation Primitives
//!
//! Free functions and types that support JSONLogic evaluation in the engine.
//! Built-in function execution lives on each config type (`MapConfig::execute`,
//! `ValidationConfig::execute`, …) — this module just provides the shared
//! evaluation machinery they all build on.
//!
//! The bump arena is held in a thread-local cell on each Tokio worker. Per
//! call, the arena is rewound via `Bump::reset` (constant-time, retains chunks)
//! before the eval. Chunks accumulate to fit the workload's high-water mark
//! and persist across calls — no per-task malloc/free churn. Profiling
//! showed per-task `Bump::with_capacity` malloc was the dominant cost when
//! arena sizing was tuned for realistic workloads; thread-local reuse
//! amortizes that to zero in steady state.
//!
//! `ArenaContext` (below) extends this further for **mutating** tasks (map):
//! the message context is `to_arena`'d once per task call into a depth‑2
//! cache, and subsequent writes only re‑arena the dirtied subtree — typically
//! `data.MT103` while the heavy `data.input` stays cached.

use crate::engine::error::Result;
use bumpalo::Bump;
use datalogic_rs::{Engine, Logic};
use datavalue::{DataValue, OwnedDataValue};
use log::error;
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
    with_eval_arena(|arena| {
        let r = engine.evaluate(compiled, context, arena)?;
        Ok(r.to_owned())
    })
}

/// As [`eval_to_owned`] but projects the arena result directly to
/// `serde_json::Value` — one walk out, with no `OwnedDataValue` intermediate
/// and no `serde_json::from_value` rebuild.
#[inline]
pub(crate) fn eval_to_json(
    engine: &Engine,
    compiled: &Logic,
    context: &OwnedDataValue,
) -> std::result::Result<serde_json::Value, datalogic_rs::Error> {
    with_eval_arena(|arena| Ok(engine.evaluate(compiled, context, arena)?.to_serde_value()))
}

/// As [`eval_to_owned`] but coerced to a *plain* string: a `DataValue::String`
/// yields its contents, everything else its compact JSON form (`Display` on
/// `DataValue` is compact JSON).
///
/// This deliberately disagrees with datalogic-rs's `String: FromDataValue` — and
/// therefore with `Session::eval_str` — which keeps the JSON quoting, so a string
/// result there comes back as `"\"abc\""`. The divergence is pinned by a test.
#[inline]
pub(crate) fn eval_to_plain_string(
    engine: &Engine,
    compiled: &Logic,
    context: &OwnedDataValue,
) -> std::result::Result<String, datalogic_rs::Error> {
    with_eval_arena(|arena| {
        Ok(match engine.evaluate(compiled, context, arena)? {
            DataValue::String(s) => s.to_string(),
            other => other.to_string(),
        })
    })
}

/// Run `f` against the worker thread's rewound arena, falling back to a fresh
/// `Bump` if the thread-local is already borrowed.
///
/// The fallback exists for re-entrancy. The engine's own paths never nest:
/// `next_async_boundary` keeps every non-sync-builtin task out of the
/// `run_sync_stretch` arena block, and the only in-crate `TaskContext::new` call
/// site sits at an `.await` outside every `with_arena` scope. But
/// `TaskContext::new` is `pub` precisely so tests and benches can drive a handler
/// directly, and one of those *can* be written inside a `with_arena` closure.
/// Paying one allocation there is better than panicking out of
/// `process_message`.
///
/// Note [`with_arena`] deliberately does **not** do this: a nested batch scope
/// would be an engine bug, and the panic is the right signal for it.
#[inline]
fn with_eval_arena<R>(f: impl FnOnce(&Bump) -> R) -> R {
    EVAL_ARENA.with(|cell| match cell.try_borrow_mut() {
        Ok(mut arena) => {
            arena.reset();
            f(&arena)
        }
        Err(_) => {
            let arena = Bump::new();
            f(&arena)
        }
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
    /// performs the in-place mutation) and the arena cache, using the
    /// already-arena-resident eval result `value_av` for the cache side.
    ///
    /// The cache update splices `value_av` into the depth-2 slot directly
    /// (rebuilding only the spine of the written path — shallow pair copies,
    /// no string data copied, no descent into unchanged siblings). The old
    /// per-write behaviour — `to_arena` of the *entire* owned depth-2
    /// subtree — made k mappings into the same subtree O(k²); it remains as
    /// [`Self::refresh_after_write_parts`], the correctness backstop for any
    /// shape the splice doesn't cover (array segments, missing depth-2 slot,
    /// non-object hops, root writes).
    ///
    /// The owned-context write stays the source of truth; `value_av` must be
    /// the arena form of the value the closure writes at `parts`.
    pub fn apply_mutation_parts_write_through(
        &mut self,
        owned_ctx: &mut OwnedDataValue,
        parts: &[Arc<str>],
        value_av: DataValue<'a>,
        apply: impl FnOnce(&mut OwnedDataValue),
    ) {
        apply(owned_ctx);
        if !self.try_splice_write_parts(parts, value_av) {
            self.refresh_after_write_parts(owned_ctx, parts);
        }
        // Differential check (unit-test builds only): after every write-through
        // the cache must equal a from-scratch rebuild of the owned context.
        // This gives every unit test that drives a map task free differential
        // coverage of the splice against the owned source of truth.
        #[cfg(test)]
        self.assert_matches_owned(owned_ctx);
    }

    /// Attempt the plain-case arena splice for a write of `value_av` at
    /// `parts`. Covered shape: `parts[0]` resolves to a cached top slot with
    /// a depth-2 cache; for writes deeper than depth 2, `parts[1]` names an
    /// *existing* depth-2 child and every deeper segment is a plain
    /// (non-numeric) object key. Returns `false` when the shape needs the
    /// owned-rebuild fallback instead.
    fn try_splice_write_parts(&mut self, parts: &[Arc<str>], value_av: DataValue<'a>) -> bool {
        if parts.len() < 2 {
            return false;
        }
        // A raw segment that parses as usize takes array-index semantics in
        // `set_nested_value_parts` — fall back rather than mirror that here.
        // (`#`-escaped keys never parse: `#20` is the object key "20".)
        if parts[2..].iter().any(|p| p.parse::<usize>().is_ok()) {
            return false;
        }

        let top = strip_hash(&parts[0]);
        let Some(top_idx) = self.top_keys.iter().position(|k| *k == top) else {
            // Write created a brand-new top slot — rare; let the fallback
            // build it from owned.
            return false;
        };
        let arena = self.arena;
        let Some(d2) = self.depth2[top_idx].as_mut() else {
            // Top slot isn't an Object (or has no depth-2 cache) — the owned
            // write may have no-op'd or reshaped it; fall back.
            return false;
        };

        let d2_key = strip_hash(&parts[1]);
        let d2_pos = d2.keys.iter().position(|k| *k == d2_key);

        if parts.len() == 2 {
            // Whole depth-2 slot replace/insert: the eval result IS the new
            // child value — no owned round-trip needed.
            match d2_pos {
                Some(pos) => d2.values[pos] = value_av,
                None => {
                    d2.keys.push(arena.alloc_str(d2_key));
                    d2.values.push(value_av);
                }
            }
        } else {
            let Some(pos) = d2_pos else {
                // First write into a not-yet-cached depth-2 subtree (e.g. the
                // first mapping into `data.MT103`) — the owned write creates
                // it; let the fallback arena the fresh subtree once.
                return false;
            };
            let Some(new_subtree) =
                splice_object_write(arena, d2.values[pos], &parts[2..], value_av)
            else {
                // Non-object hop (scalar/array mid-path) — the owned write
                // no-ops or takes semantics the splice doesn't mirror.
                return false;
            };
            d2.values[pos] = new_subtree;
        }

        let slice = build_object_slice(arena, &d2.keys, &d2.values);
        self.top_values[top_idx] = DataValue::Object(slice);
        true
    }

    /// Test-only differential check: the live cache must be value- and
    /// order-identical to a fresh depth-2 rebuild of `owned_ctx`.
    #[cfg(test)]
    fn assert_matches_owned(&self, owned_ctx: &OwnedDataValue) {
        let rebuilt = ArenaContext::from_owned(owned_ctx, self.arena);
        assert_eq!(
            self.as_data_value().to_owned(),
            rebuilt.as_data_value().to_owned(),
            "arena cache diverged from owned context"
        );
    }

    /// Refresh the arena slot(s) for `path` from the current `owned_ctx`,
    /// without applying any new write. Used when a sync task mutated
    /// `message.context` directly (e.g. `parse_json` going through legacy
    /// helpers) and we need the arena to catch up.
    pub fn refresh_for_path(&mut self, owned_ctx: &OwnedDataValue, path: &str) {
        self.refresh_after_write(owned_ctx, path);
    }

    /// Pre-split variant of [`Self::refresh_for_path`] — callers holding
    /// compiler-populated path parts (parse/publish target paths) skip the
    /// per-call `str::split`.
    pub fn refresh_for_path_parts(&mut self, owned_ctx: &OwnedDataValue, parts: &[Arc<str>]) {
        self.refresh_after_write_parts(owned_ctx, parts);
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
                    (OwnedDataValue::Object(new_children), Some(child_key), Some(d2)) => {
                        if let Some(new_child) = new_children
                            .iter()
                            .find(|(k, _)| k == child_key)
                            .map(|(_, v)| v)
                        {
                            // Replace or insert the single child slot.
                            let child_arena = new_child.to_arena(self.arena);
                            if let Some(pos) = d2.keys.iter().position(|k| *k == child_key) {
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
                            if let Some(pos) = d2.keys.iter().position(|k| *k == child_key) {
                                d2.keys.remove(pos);
                                d2.values.remove(pos);
                            }
                        }
                        let slice = build_object_slice(self.arena, &d2.keys, &d2.values);
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

/// Strip exactly one leading `#` from an object-key path segment — same
/// semantics as `utils::strip_hash_prefix` (`"#20"` → `"20"`, `"##"` → `"#"`).
#[inline]
fn strip_hash(part: &str) -> &str {
    part.strip_prefix('#').unwrap_or(part)
}

/// Rebuild only the spine of `current` for a write of `value` at `parts`
/// (all plain object keys — the caller has excluded numeric segments).
/// Each level allocates a fresh `(key, value)` slice with the one descended
/// child replaced: shallow pair copies, no string data copied, no descent
/// into unchanged siblings. A missing key builds the remaining chain as
/// nested single-pair Objects, mirroring `set_nested_value_parts`' create
/// semantics for non-numeric segments. Returns `None` when `current` is not
/// an Object — those shapes (array hop, scalar overwrite mid-path) take the
/// owned-rebuild fallback, which mirrors whatever the owned write did.
fn splice_object_write<'a>(
    arena: &'a Bump,
    current: DataValue<'a>,
    parts: &[Arc<str>],
    value: DataValue<'a>,
) -> Option<DataValue<'a>> {
    let DataValue::Object(pairs) = current else {
        return None;
    };
    let key = strip_hash(&parts[0]);
    let pos = pairs.iter().position(|(k, _)| *k == key);

    let new_slice = if parts.len() == 1 {
        // Terminal level: replace the slot, or append a new pair.
        match pos {
            Some(p) => alloc_pairs_with_replaced(arena, pairs, p, value),
            None => alloc_pairs_with_appended(arena, pairs, arena.alloc_str(key), value),
        }
    } else {
        match pos {
            Some(p) => {
                let child = splice_object_write(arena, pairs[p].1, &parts[1..], value)?;
                alloc_pairs_with_replaced(arena, pairs, p, child)
            }
            None => {
                let chain = build_object_chain(arena, &parts[1..], value);
                alloc_pairs_with_appended(arena, pairs, arena.alloc_str(key), chain)
            }
        }
    };
    Some(DataValue::Object(new_slice))
}

/// Wrap `value` in nested single-pair Objects, innermost-last: for parts
/// `[a, b]` produces `{a: {b: value}}`. Mirrors the container-creation path
/// of `set_nested_value_parts` when every segment is a non-numeric key.
fn build_object_chain<'a>(
    arena: &'a Bump,
    parts: &[Arc<str>],
    value: DataValue<'a>,
) -> DataValue<'a> {
    let mut acc = value;
    for part in parts.iter().rev() {
        let key: &'a str = arena.alloc_str(strip_hash(part));
        let slice = arena.alloc_slice_fill_iter(std::iter::once((key, acc)));
        acc = DataValue::Object(slice);
    }
    acc
}

/// Fresh slice with `pairs[replace_idx]`'s value swapped for `new_value`.
fn alloc_pairs_with_replaced<'a>(
    arena: &'a Bump,
    pairs: &'a [(&'a str, DataValue<'a>)],
    replace_idx: usize,
    new_value: DataValue<'a>,
) -> &'a [(&'a str, DataValue<'a>)] {
    arena.alloc_slice_fill_with(pairs.len(), |i| {
        if i == replace_idx {
            (pairs[i].0, new_value)
        } else {
            pairs[i]
        }
    })
}

/// Fresh slice of `pairs` plus one appended `(key, value)` pair.
fn alloc_pairs_with_appended<'a>(
    arena: &'a Bump,
    pairs: &'a [(&'a str, DataValue<'a>)],
    key: &'a str,
    value: DataValue<'a>,
) -> &'a [(&'a str, DataValue<'a>)] {
    arena.alloc_slice_fill_with(pairs.len() + 1, |i| {
        if i < pairs.len() {
            pairs[i]
        } else {
            (key, value)
        }
    })
}

/// Evaluate a workflow or task condition using a cached compiled logic
/// expression. Returns `true` when `condition_index` is `None` (no condition
/// is treated as "always run"). Evaluation errors are logged and downgraded
/// to `false` — a condition that fails to evaluate skips its task/workflow
/// rather than aborting the whole message.
pub fn evaluate_condition(
    engine: &Engine,
    compiled_condition: Option<&Arc<Logic>>,
    context: &OwnedDataValue,
) -> Result<bool> {
    match compiled_condition {
        Some(compiled) => match eval_to_owned(engine, compiled, context) {
            Ok(value) => Ok(matches!(value, OwnedDataValue::Bool(true))),
            Err(e) => {
                error!("Failed to evaluate condition: {:?}", e);
                Ok(false)
            }
        },
        None => Ok(true),
    }
}

/// Same as `evaluate_condition` but evaluates against an arena-resident
/// `DataValue` and an existing `Bump`. Used inside a `with_arena` block
/// (the workflow sync-stretch path) to avoid re-entering the thread-local
/// arena `RefCell::borrow_mut`.
pub fn evaluate_condition_in_arena(
    engine: &Engine,
    compiled_condition: Option<&Arc<Logic>>,
    ctx: DataValue<'_>,
    arena: &Bump,
) -> Result<bool> {
    match compiled_condition {
        Some(compiled) => match engine.evaluate(compiled, ctx, arena) {
            Ok(value) => Ok(matches!(value, DataValue::Bool(true))),
            Err(e) => {
                error!("Failed to evaluate condition: {:?}", e);
                Ok(false)
            }
        },
        None => Ok(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::utils::{set_nested_value, set_nested_value_parts};
    use serde_json::json;

    fn dv(v: serde_json::Value) -> OwnedDataValue {
        OwnedDataValue::from(&v)
    }

    fn parts_of(path: &str) -> Vec<Arc<str>> {
        path.split('.').map(Arc::from).collect()
    }

    /// Drive a corpus of writes through the write-through path. Every call
    /// re-validates the cache against a from-scratch rebuild via the
    /// `#[cfg(test)]` assertion inside `apply_mutation_parts_write_through`,
    /// so this test fails on any splice/fallback divergence — splice hits
    /// (deep object paths, `#`-escaped keys, chain creation, depth-2
    /// replace/insert) and fallback shapes (array segments, scalar mid-path
    /// no-ops, new top slots) alike.
    #[test]
    fn write_through_differential_corpus() {
        let mut owned = dv(json!({
            "data": {
                "input": {"big": {"nested": [1, 2, 3]}, "flag": true},
                "MT103": {"20": "REF", "72": "existing"},
                "items": [{"x": 1}, {"x": 2}],
                "scalar": "leaf"
            },
            "metadata": {"processed_at": "t0"},
            "temp_data": {}
        }));

        with_arena(|arena| {
            let mut ctx = ArenaContext::from_owned(&owned, arena);

            let corpus: &[(&str, serde_json::Value)] = &[
                // Depth-3 replace of an existing key inside a cached subtree.
                ("data.MT103.20", json!("NEWREF")),
                // Depth-3 append of a new key.
                ("data.MT103.23B", json!("CRED")),
                // Depth-4 with a missing intermediate — chain creation.
                ("data.MT103.32A.amount", json!(1500.25)),
                // Depth-4 into the now-existing intermediate.
                ("data.MT103.32A.currency", json!("USD")),
                // Deeper chain creation (three missing levels).
                ("data.MT103.a.b.c", json!({"deep": [1, {"k": "v"}]})),
                // Overwrite an entire existing sub-object.
                ("data.MT103.32A", json!({"replaced": true})),
                // Depth-2 whole-slot replace with an object.
                ("data.MT103", json!({"fresh": {"start": 1}})),
                // Depth-2 whole-slot replace with a scalar (drops d2 cache on
                // next rebuild; subsequent write through it must no-op).
                ("data.scalar2", json!("plain")),
                // New depth-2 key inserted into an existing top slot.
                ("data.MT202", json!({"20": "REF2"})),
                // Write into a not-yet-cached depth-2 subtree (fallback).
                ("data.MT205.20", json!("REF5")),
                // `#`-escaped keys: object keys "20" and "#".
                ("data.MT103.#20", json!("HASH20")),
                ("data.MT103.##", json!("HASHKEY")),
                // Array index segments — fallback shapes.
                ("data.items.0.x", json!(10)),
                ("data.items.3", json!({"x": 4})),
                // Scalar mid-path — owned write no-ops; cache must stay in sync.
                ("data.scalar.sub.key", json!("ignored")),
                // Metadata / temp_data writes.
                ("metadata.progress.workflow_id", json!("wf1")),
                ("temp_data.uetr", json!("uuid-here")),
                // Brand-new top-level slot (fallback creates it).
                ("other_top.x.y", json!(42)),
            ];

            for (path, val) in corpus {
                let parts = parts_of(path);
                let owned_val = dv(val.clone());
                let value_av = owned_val.to_arena(arena);
                ctx.apply_mutation_parts_write_through(&mut owned, &parts, value_av, |c| {
                    set_nested_value_parts(c, &parts, owned_val.clone());
                });
            }

            // Spot-check a few end states against the owned source of truth.
            assert_eq!(owned["data"]["MT103"]["20"], dv(json!("HASH20")));
            assert_eq!(owned["data"]["MT103"]["fresh"]["start"], dv(json!(1)));
            assert_eq!(owned["data"]["items"][3]["x"], dv(json!(4)));
            assert_eq!(owned["data"]["scalar"], dv(json!("leaf")));
            assert_eq!(
                owned["metadata"]["progress"]["workflow_id"],
                dv(json!("wf1"))
            );
        });
    }

    /// Interleave reads (as_data_value) with write-throughs to make sure a
    /// previously handed-out slice never aliases a mutated spine.
    #[test]
    fn write_through_reads_see_latest_state() {
        let mut owned = dv(json!({
            "data": {"doc": {"a": 1}},
            "metadata": {},
            "temp_data": {}
        }));

        with_arena(|arena| {
            let mut ctx = ArenaContext::from_owned(&owned, arena);

            for i in 0..10u64 {
                let key = format!("data.doc.f{i}");
                let parts = parts_of(&key);
                let owned_val = OwnedDataValue::from(i);
                let value_av = owned_val.to_arena(arena);
                ctx.apply_mutation_parts_write_through(&mut owned, &parts, value_av, |c| {
                    set_nested_value_parts(c, &parts, owned_val.clone());
                });
                // A fresh projection after every write must equal owned.
                assert_eq!(ctx.as_data_value().to_owned(), owned);
            }
        });
    }

    /// The refresh-only path (used by parse_json and the per-task progress
    /// refresh) must remain consistent when mixed with write-throughs.
    #[test]
    fn refresh_and_write_through_interleave() {
        let mut owned = dv(json!({
            "data": {},
            "metadata": {},
            "temp_data": {}
        }));

        with_arena(|arena| {
            let mut ctx = ArenaContext::from_owned(&owned, arena);

            // Simulate parse_json: direct owned write + refresh_for_path.
            set_nested_value(&mut owned, "data.input", dv(json!({"payload": {"v": 7}})));
            ctx.refresh_for_path(&owned, "data.input");
            assert_eq!(ctx.as_data_value().to_owned(), owned);

            // Then map write-throughs landing next to it.
            let parts = parts_of("data.out.v");
            let owned_val = dv(json!(7));
            let value_av = owned_val.to_arena(arena);
            ctx.apply_mutation_parts_write_through(&mut owned, &parts, value_av, |c| {
                set_nested_value_parts(c, &parts, owned_val.clone());
            });

            // Simulate the progress write + narrow refresh.
            set_nested_value(
                &mut owned,
                "metadata.progress",
                dv(json!({"task_id": "t1"})),
            );
            ctx.refresh_for_path(&owned, "metadata.progress");
            assert_eq!(ctx.as_data_value().to_owned(), owned);
        });
    }
}
