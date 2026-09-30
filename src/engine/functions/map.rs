//! # Map Function Module
//!
//! Data transformation via JSONLogic expressions. Each mapping evaluates a
//! compiled JSONLogic rule against the message's context (`OwnedDataValue`)
//! and assigns the result to a path. The result type is `OwnedDataValue` —
//! no `serde_json::Value` intermediate.
//!
//! ## Features
//!
//! - JSONLogic-driven transformations
//! - Dot-path target paths with auto-creation
//! - Root-field merge semantics for `data` / `metadata` / `temp_data`
//! - Null results skip assignment by default ("set or keep");
//!   `on_null: "unset"` removes the path instead
//! - `unset: true` removes a path outright
//! - `mode: "append"` / `"extend"` add to an array in place
//! - Audit-trail change tracking

use crate::engine::error::{DataflowError, Result};
use crate::engine::executor::{ArenaContext, with_arena};
use crate::engine::functions::path_template::{ContextRoot, ParamCtx, PathTemplate, ResolvedPath};
use crate::engine::message::{Change, Message};
use crate::engine::task_outcome::TaskOutcome;
use crate::engine::utils::{
    describe_kind, get_nested_value_parts, get_nested_value_parts_mut, set_nested_value_parts,
    strip_hash_prefix,
};
use datalogic_rs::{Engine, Logic};
use datavalue::{DataValue, OwnedDataValue};
use log::{debug, error};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::sync::Arc;

/// Configuration for the map function containing a list of mappings.
#[derive(Debug, Clone, Deserialize)]
pub struct MapConfig {
    /// List of mappings to execute in order.
    pub mappings: Vec<MapMapping>,
}

/// A single mapping that transforms and assigns data — or, with
/// [`unset`](Self::unset), removes it.
///
/// Three rules hold between the keys, and a definition breaking one fails to
/// parse (so [`Workflow::validate_authored`](crate::Workflow::validate_authored)
/// reports it as [`IssueCode::InvalidMapping`](crate::IssueCode::InvalidMapping)
/// and `Engine::build` refuses it):
///
/// - a mapping has `logic` or `"unset": true`, never both;
/// - `on_null` and `mode` need `logic`, since they say what a *result* does;
/// - a removal, an append or an extend may not name a context root (`data`,
///   `metadata`, `temp_data`). A literal path is checked here; a computed one
///   that resolves to a root fails that mapping at run time instead.
#[derive(Debug, Clone, Default)]
pub struct MapMapping {
    /// Target path where the result will be stored (e.g., `"data.user.name"`).
    /// Supports dot notation for nested paths and `#` prefix for numeric field
    /// names.
    ///
    /// JSONLogic, so a mapping can compute where it writes:
    /// `{"cat": ["data.accounts.", {"var": "data.id"}, ".balance"]}`. The
    /// static spelling is a literal string, which folds at compile time and
    /// keeps the precomputed split this hot loop has always used — a dynamic
    /// path is the only one that pays to split per write.
    pub path: PathTemplate<ContextRoot>,

    /// JSONLogic expression (kept as `serde_json::Value` since this is the
    /// shape the compiler accepts; not runtime data).
    ///
    /// `Value::Null` on an [`unset`](Self::unset) mapping, which has none.
    pub logic: Value,

    /// Remove the key at `path` instead of writing to it. Takes no `logic`.
    ///
    /// A no-op when the key is already absent. A removal is recorded on the
    /// audit trail as a [`Change`] with [`removed`](Change::removed) set.
    ///
    /// This exists because `null` cannot mean "clear": a null result is
    /// skipped (see [`OnNull::Skip`]), which is what lets
    /// `{"if": [cond, value, null]}` mean "set or keep".
    pub unset: bool,

    /// What a `null` result from `logic` does. Defaults to
    /// [`OnNull::Skip`], the historical behaviour.
    pub on_null: OnNull,

    /// How a non-null result is written. Defaults to [`MapMode::Set`], the
    /// historical behaviour; [`MapMode::Append`] and [`MapMode::Extend`] add to
    /// the array at `path` in place.
    pub mode: MapMode,

    /// Engine-internal: pre-compiled JSONLogic, populated by `LogicCompiler`.
    /// `None` is logged as an error during execute (the compiler should always
    /// populate it, except on an `unset` mapping, which has no logic and never
    /// reads this). Not part of the stable API.
    #[doc(hidden)]
    pub compiled_logic: Option<Arc<Logic>>,
}

/// What a [`MapMapping`] does when its `logic` evaluates to `null`.
///
/// `#[non_exhaustive]`: a later minor may add a spelling — writing an explicit
/// `null`, say, which no mapping can do today.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum OnNull {
    /// Leave the path as it was. A `var` that misses and an `if` with no
    /// matching branch both produce `null`, so skipping is what keeps a
    /// missing source from wiping its target — and what makes
    /// `{"if": [cond, value, null]}` mean "set or keep".
    #[default]
    Skip,
    /// Remove the key at `path`, exactly as [`MapMapping::unset`] would:
    /// `{"if": [cond, value, null]}` then means "set or clear".
    Unset,
}

/// How a [`MapMapping`] writes a non-null result to its `path`.
///
/// `#[non_exhaustive]`: a later minor may add a spelling.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum MapMode {
    /// Replace the value at `path` (merging, when `path` is a context root).
    #[default]
    Set,
    /// Push the result onto the array at `path`, in place.
    ///
    /// A missing or `null` target becomes a one-element array; any other
    /// non-array target fails the mapping rather than being wrapped. Only the
    /// result is converted and copied, so appending costs the size of the
    /// entry, not the size of the array: the `merge` idiom it replaces rebuilt
    /// the whole array on every append, which made a log accumulated over a
    /// loop quadratic.
    Append,
    /// As [`Self::Append`], pushing each element of an array result. A
    /// non-array result fails the mapping.
    Extend,
}

/// A mapping as authored, before the rules between its keys are applied.
///
/// Shared by [`MapMapping`]'s `Deserialize` and the authoring walk, so the
/// rules have one definition and `validate_authored` cannot disagree with the
/// parser about them.
#[derive(Deserialize)]
pub(crate) struct AuthoredMapping {
    path: PathTemplate<ContextRoot>,
    /// `Some(Value::Null)` for an explicit `"logic": null`; `None` only when
    /// the key is absent. A plain `Option<Value>` would fold the two together.
    #[serde(default, deserialize_with = "present")]
    logic: Option<Value>,
    #[serde(default)]
    unset: bool,
    /// `Option` so `on_null` without `logic` is caught even when it spells the
    /// default.
    #[serde(default)]
    on_null: Option<OnNull>,
    /// `Option` for the same reason as `on_null`.
    #[serde(default)]
    mode: Option<MapMode>,
}

/// Deserialize a field that is present, keeping an explicit `null` as
/// `Some(Value::Null)`.
fn present<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

/// A rule between a mapping's keys that an authored mapping breaks.
pub(crate) struct MappingProblem {
    /// The mapping key the problem is reported against.
    pub(crate) field: &'static str,
    pub(crate) message: &'static str,
}

impl AuthoredMapping {
    /// The first rule this mapping breaks, if any.
    pub(crate) fn problem(&self) -> Option<MappingProblem> {
        let problem = |field, message| Some(MappingProblem { field, message });
        match (&self.logic, self.unset) {
            (None, false) => {
                return problem(
                    "logic",
                    "a mapping needs `logic`, or `\"unset\": true` to remove the path",
                );
            }
            (Some(_), true) => {
                return problem(
                    "unset",
                    "`unset` removes the path and takes no `logic` — to remove it only \
                     when the result is null, keep `logic` and use `\"on_null\": \"unset\"`",
                );
            }
            _ => {}
        }
        if self.logic.is_none() && self.on_null.is_some() {
            return problem(
                "on_null",
                "`on_null` says what a null `logic` result does, and this mapping has no `logic`",
            );
        }
        if self.logic.is_none() && self.mode.is_some() {
            return problem(
                "mode",
                "`mode` says how a `logic` result is written, and this mapping has no `logic`",
            );
        }
        let literal_root = self
            .path
            .as_json()
            .as_str()
            .is_some_and(|path| is_context_root(&path.split('.').collect::<Vec<_>>()));
        if literal_root && (self.unset || self.on_null == Some(OnNull::Unset)) {
            return problem(
                "path",
                "a context root (`data`, `metadata`, `temp_data`) cannot be removed — \
                 remove the keys under it instead",
            );
        }
        if literal_root && matches!(self.mode, Some(MapMode::Append | MapMode::Extend)) {
            return problem(
                "path",
                "a context root (`data`, `metadata`, `temp_data`) is an object and cannot be \
                 appended to — name an array under it instead",
            );
        }
        None
    }

    fn into_mapping(self) -> MapMapping {
        MapMapping {
            path: self.path,
            logic: self.logic.unwrap_or(Value::Null),
            unset: self.unset,
            on_null: self.on_null.unwrap_or_default(),
            mode: self.mode.unwrap_or_default(),
            compiled_logic: None,
        }
    }
}

impl<'de> Deserialize<'de> for MapMapping {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let authored = AuthoredMapping::deserialize(d)?;
        if let Some(problem) = authored.problem() {
            return Err(D::Error::custom(format_args!(
                "mapping `{}`: {}",
                problem.field, problem.message
            )));
        }
        Ok(authored.into_mapping())
    }
}

/// Whether `parts` names a context root — the one kind of path a mapping may
/// write (it merges) but never remove.
fn is_context_root<P: AsRef<str>>(parts: &[P]) -> bool {
    matches!(parts, [only] if matches!(strip_hash_prefix(only.as_ref()), "data" | "metadata" | "temp_data"))
}

impl MapMapping {
    /// How to name this mapping's destination in a log line or error, without a
    /// message in hand.
    ///
    /// A constant path is its dotted form. A computed one has no single answer
    /// before evaluation, so it is named by the expression that produces it —
    /// which is what an author would search their workflow for.
    pub(crate) fn describe_path(&self) -> String {
        self.path
            .constant_path()
            .map(str::to_string)
            .unwrap_or_else(|| self.path.as_json().to_string())
    }
}

impl MapConfig {
    /// Parses a `MapConfig` from a JSON value.
    pub fn from_json(input: &Value) -> Result<Self> {
        let mappings = input.get("mappings").ok_or_else(|| {
            DataflowError::Validation("Missing 'mappings' array in input".to_string())
        })?;

        let mappings_arr = mappings
            .as_array()
            .ok_or_else(|| DataflowError::Validation("'mappings' must be an array".to_string()))?;

        let mut parsed_mappings = Vec::new();

        for mapping in mappings_arr {
            if mapping.get("path").is_none() {
                return Err(DataflowError::Validation(
                    "Missing 'path' in mapping".to_string(),
                ));
            }
            // The same parse the workflow deserializer runs, so the rules
            // between `logic`, `unset` and `on_null` hold here too.
            let parsed = MapMapping::deserialize(mapping)
                .map_err(|e| DataflowError::Validation(e.to_string()))?;
            parsed_mappings.push(parsed);
        }

        Ok(Self {
            mappings: parsed_mappings,
        })
    }

    /// Executes all map transformations using pre-compiled logic.
    ///
    /// # Arguments
    /// * `message` - The message to transform (modified in place)
    /// * `engine` - Datalogic v5 engine for evaluation
    pub fn execute(
        &self,
        message: &mut Message,
        engine: &Arc<Engine>,
    ) -> Result<(TaskOutcome, Vec<Change>)> {
        // Default path: open the arena, build a fresh ArenaContext from the
        // current `message.context`, run mappings. Used when no outer
        // workflow-level arena session is available.
        with_arena(|arena| {
            let mut arena_ctx = ArenaContext::from_owned(&message.context, arena);
            self.execute_in_arena(message, &mut arena_ctx, engine, None)
        })
    }

    /// Mappings-loop run against an externally-provided `ArenaContext`.
    /// Used by the workflow-level sync-stretch executor so the
    /// `OwnedDataValue → arena` conversion done by an earlier task in the
    /// same workflow stretch is reused.
    ///
    /// `mapping_snapshots` (when `Some`) collects a `serde_json::Value` snapshot
    /// of `message.context` *before* each mapping runs — the trace
    /// surface uses this for per-mapping debugging. `None` skips the snapshot
    /// work entirely (the production path).
    /// The `'arena` lifetime ties `&self` to the arena context: the eval
    /// result borrows both the compiled logic and the arena
    /// (`Engine::evaluate` unifies them), and the write-through splice needs
    /// that result at exactly the cache's lifetime.
    pub(crate) fn execute_in_arena<'arena>(
        &'arena self,
        message: &mut Message,
        arena_ctx: &mut ArenaContext<'arena>,
        engine: &Arc<Engine>,
        mut mapping_snapshots: Option<&mut Vec<Value>>,
    ) -> Result<(TaskOutcome, Vec<Change>)> {
        // Audit-on runs push one Change per mapping that writes or removes —
        // size for the common all-mappings-write case up front.
        let mut changes = if message.capture_changes {
            Vec::with_capacity(self.mappings.len())
        } else {
            Vec::new()
        };
        let mut errors_encountered = false;

        debug!("Map: Executing {} mappings", self.mappings.len());

        let arena = arena_ctx.arena();
        for mapping in &self.mappings {
            debug!("Processing mapping to path: {}", mapping.describe_path());

            // Trace mode: snapshot the context as a serde_json::Value *before*
            // applying this mapping. Bridge cost is acceptable on the debug
            // surface; production callers pass `None` and skip it entirely.
            if let Some(buf) = mapping_snapshots.as_deref_mut() {
                buf.push(Value::from(&message.context));
            }

            if mapping.unset {
                // Nothing to evaluate: resolve the destination against the
                // current context and take the key out.
                let ctx_av = arena_ctx.as_data_value();
                let Some(resolved) = resolve_destination(mapping, engine, ctx_av, arena) else {
                    errors_encountered = true;
                    continue;
                };
                if !remove_at(message, arena_ctx, &resolved, &mut changes) {
                    errors_encountered = true;
                }
                continue;
            }

            // Pre-compiled `Arc<Logic>` lives on the mapping; the workflow
            // compiler always populates it. `None` only happens for mappings
            // constructed directly without compilation (test surface) —
            // logged and skipped here.
            let compiled_logic = match &mapping.compiled_logic {
                Some(logic) => logic,
                None => {
                    error!(
                        "Map: Logic not compiled for mapping to {}",
                        mapping.describe_path()
                    );
                    errors_encountered = true;
                    continue;
                }
            };

            let ctx_av = arena_ctx.as_data_value();
            let result_av = match engine.evaluate(compiled_logic, ctx_av, arena) {
                Ok(av) => av,
                Err(e) => {
                    error!(
                        "Map: Error evaluating logic for path {}: {:?}",
                        mapping.describe_path(),
                        e
                    );
                    errors_encountered = true;
                    continue;
                }
            };

            let transformed_value = result_av.to_owned();
            debug!(
                "Map: Evaluated logic for path {} resulted in: {:?}",
                mapping.describe_path(),
                transformed_value
            );

            if matches!(transformed_value, OwnedDataValue::Null) {
                match mapping.on_null {
                    OnNull::Skip => {
                        debug!(
                            "Map: Skipping mapping for path {} as result is null",
                            mapping.describe_path()
                        );
                    }
                    OnNull::Unset => {
                        let Some(resolved) = resolve_destination(mapping, engine, ctx_av, arena)
                        else {
                            errors_encountered = true;
                            continue;
                        };
                        if !remove_at(message, arena_ctx, &resolved, &mut changes) {
                            errors_encountered = true;
                        }
                    }
                }
                continue;
            }

            // Where this mapping writes. A constant path — the static
            // spelling, and the overwhelmingly common case — hands back the
            // pair precomputed at engine construction as two refcount bumps;
            // only a computed path splits here. Resolved *after* the logic so
            // a dynamic destination sees the same context the value did.
            let Some(resolved) = resolve_destination(mapping, engine, ctx_av, arena) else {
                errors_encountered = true;
                continue;
            };
            if mapping.mode != MapMode::Set {
                if !append_at(
                    message,
                    arena_ctx,
                    mapping.mode,
                    &resolved,
                    result_av,
                    transformed_value,
                    &mut changes,
                ) {
                    errors_encountered = true;
                }
                continue;
            }
            let (path_arc, parts) = (&resolved.0, &*resolved.1);

            if message.capture_changes {
                // Audit-on: capture old/new values directly into the `Change`.
                // `Change` owns `OwnedDataValue`s (not `Arc<…>`) — one fewer
                // heap allocation per recorded mutation.
                let old_value = get_nested_value_parts(&message.context, parts)
                    .cloned()
                    .unwrap_or(OwnedDataValue::Null);
                let new_value = transformed_value.clone();

                changes.push(Change {
                    path: Arc::clone(path_arc),
                    old_value,
                    new_value,
                    removed: false,
                });
            }
            // Write-through: the owned context write is the source of truth;
            // `result_av` (already arena-resident) is spliced into the cache
            // directly, avoiding the owned→arena re-walk of the whole target
            // subtree that made k same-subtree mappings O(k²).
            arena_ctx.apply_mutation_parts_write_through(
                &mut message.context,
                parts,
                *result_av,
                |ctx| {
                    apply_mapping_parts(ctx, parts, path_arc, transformed_value);
                },
            );
            debug!("Successfully mapped to path: {path_arc}");
        }

        let outcome = if errors_encountered {
            TaskOutcome::Status(500)
        } else {
            TaskOutcome::Success
        };
        Ok((outcome, changes))
    }
}

/// Resolve where `mapping` writes, logging a failure. `None` fails the mapping.
///
/// Borrows: a constant path hands back the pair precomputed at engine
/// construction.
fn resolve_destination<'a>(
    mapping: &'a MapMapping,
    engine: &'a Engine,
    ctx_av: datavalue::DataValue<'a>,
    arena: &'a datalogic_rs::bumpalo::Bump,
) -> Option<std::borrow::Cow<'a, ResolvedPath>> {
    match mapping
        .path
        .resolve_in_arena(ParamCtx::new(engine, ctx_av, arena))
    {
        Ok(pair) => Some(pair),
        Err(e) => {
            error!(
                "Map: Error resolving destination path {}: {:?}",
                mapping.describe_path(),
                e
            );
            None
        }
    }
}

/// Remove the key at `resolved` from the context and the arena cache,
/// recording the removal when audit capture is on. An absent key is a no-op
/// and records nothing.
///
/// `false` when the path names a context root. A literal root is refused at
/// parse time, so only a computed path that resolves to one reaches here.
fn remove_at(
    message: &mut Message,
    arena_ctx: &mut ArenaContext<'_>,
    resolved: &ResolvedPath,
    changes: &mut Vec<Change>,
) -> bool {
    let (path_arc, parts) = (&resolved.0, &*resolved.1);
    if is_context_root(parts) {
        error!("Map: Refusing to remove context root {path_arc}");
        return false;
    }
    match arena_ctx.apply_removal_parts(&mut message.context, parts) {
        Some(old_value) => {
            if message.capture_changes {
                changes.push(Change {
                    path: Arc::clone(path_arc),
                    old_value,
                    new_value: OwnedDataValue::Null,
                    removed: true,
                });
            }
            debug!("Map: Removed {path_arc}");
        }
        None => debug!("Map: Nothing to remove at {path_arc}"),
    }
    true
}

/// Add a non-null result to the array at `resolved`, in place: one element
/// for [`MapMode::Append`], each element of an array result for
/// [`MapMode::Extend`]. A missing or `null` target becomes a new array.
///
/// Only the result is copied. The owned array grows by the result's owned form
/// (already converted for the null check), and the arena cache by a shallow
/// copy of the old slice plus the result's arena form, so the cost is the size
/// of the entry, not of the array. Records one [`Change`] per element, at its
/// index (`data.log.7`), so a traced run does not copy the array either.
///
/// `false`, after logging, when the path names a context root, the target is
/// some other kind of value, or `extend` got a non-array result: no silent
/// wrapping.
fn append_at<'a>(
    message: &mut Message,
    arena_ctx: &mut ArenaContext<'a>,
    mode: MapMode,
    resolved: &ResolvedPath,
    result_av: &'a DataValue<'a>,
    result: OwnedDataValue,
    changes: &mut Vec<Change>,
) -> bool {
    let (path_arc, parts) = (&resolved.0, &*resolved.1);
    if is_context_root(parts) {
        error!("Map: Refusing to append to context root {path_arc}");
        return false;
    }
    let (items_av, items): (&'a [DataValue<'a>], Vec<OwnedDataValue>) = match (mode, result) {
        (MapMode::Extend, OwnedDataValue::Array(items)) => match result_av {
            DataValue::Array(items_av) => (items_av, items),
            _ => unreachable!("the owned result was converted from this arena value"),
        },
        (MapMode::Extend, other) => {
            error!(
                "Map: `extend` needs an array result for {path_arc}, got {}",
                describe_kind(&other)
            );
            return false;
        }
        (_, value) => (std::slice::from_ref(result_av), vec![value]),
    };
    let start = match get_nested_value_parts(&message.context, parts) {
        None | Some(OwnedDataValue::Null) => 0,
        Some(OwnedDataValue::Array(existing)) => existing.len(),
        Some(other) => {
            error!(
                "Map: Cannot append to {path_arc}: it holds {}, not an array",
                describe_kind(other)
            );
            return false;
        }
    };
    if message.capture_changes {
        for (i, item) in items.iter().enumerate() {
            changes.push(Change {
                path: Arc::from(format!("{path_arc}.{}", start + i)),
                old_value: OwnedDataValue::Null,
                new_value: item.clone(),
                removed: false,
            });
        }
    }

    let old_av: Option<&[DataValue<'a>]> = if start == 0 {
        Some(&[])
    } else {
        match arena_ctx.cached_at(parts) {
            Some(DataValue::Array(old)) if old.len() == start => Some(old),
            _ => None,
        }
    };
    match old_av {
        Some(old) => {
            let arena = arena_ctx.arena();
            let new_av = DataValue::Array(arena.alloc_slice_fill_with(
                old.len() + items_av.len(),
                |i| {
                    if i < old.len() {
                        old[i]
                    } else {
                        items_av[i - old.len()]
                    }
                },
            ));
            arena_ctx.apply_mutation_parts_write_through(
                &mut message.context,
                parts,
                new_av,
                |ctx| append_owned(ctx, parts, items),
            );
        }
        // The cache does not hold the array where the owned context does;
        // re-convert that subtree rather than guess.
        None => {
            append_owned(&mut message.context, parts, items);
            arena_ctx.refresh_for_path_parts(&message.context, parts);
        }
    }
    debug!("Map: Appended to {path_arc}");
    true
}

/// The owned half of [`append_at`]: extend the array at `parts`, or create it
/// when the target is missing or `null` (the caller has refused every other
/// kind).
fn append_owned(context: &mut OwnedDataValue, parts: &[Arc<str>], items: Vec<OwnedDataValue>) {
    match get_nested_value_parts_mut(context, parts) {
        Some(OwnedDataValue::Array(existing)) => existing.extend(items),
        _ => set_nested_value_parts(context, parts, OwnedDataValue::Array(items)),
    }
}

/// Pre-split variant of `apply_mapping`. Consumes `parts` for the
/// `set_nested_value` walk; `full_path` is only needed for the root-merge
/// detection (which checks the exact, un-split string).
fn apply_mapping_parts(
    context: &mut OwnedDataValue,
    parts: &[Arc<str>],
    full_path: &str,
    new_value: OwnedDataValue,
) {
    if parts.len() == 1 && matches!(full_path, "data" | "metadata" | "temp_data") {
        merge_root_field(context, full_path, new_value);
    } else {
        set_nested_value_parts(context, parts, new_value);
    }
}

/// Merge `new_value` into the existing root-field slot named `path` on the
/// context object. If both sides are objects, merge keys (new wins for
/// collisions). Otherwise, overwrite.
fn merge_root_field(context: &mut OwnedDataValue, path: &str, new_value: OwnedDataValue) {
    let OwnedDataValue::Object(ctx_pairs) = context else {
        // The canonical context is always an Object; if somehow not, replace.
        *context = wrap_root(path, new_value);
        return;
    };

    let slot_idx = ctx_pairs.iter().position(|(k, _)| k == path);
    match slot_idx {
        Some(idx) => {
            let slot = &mut ctx_pairs[idx].1;
            match (slot, new_value) {
                (OwnedDataValue::Object(existing), OwnedDataValue::Object(new_pairs)) => {
                    for (k, v) in new_pairs {
                        if let Some(s) = existing.iter_mut().find(|(ek, _)| ek == &k) {
                            s.1 = v;
                        } else {
                            existing.push((k, v));
                        }
                    }
                }
                (slot, new) => *slot = new,
            }
        }
        None => {
            ctx_pairs.push((path.to_string(), new_value));
        }
    }
}

/// Fallback wrap when the top-level context isn't an Object (shouldn't happen
/// in normal flow but kept for defence in depth).
fn wrap_root(path: &str, value: OwnedDataValue) -> OwnedDataValue {
    OwnedDataValue::Object(vec![(path.to_string(), value)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::message::Message;
    use crate::engine::utils::set_nested_value;
    use serde_json::json;

    fn dv(v: serde_json::Value) -> OwnedDataValue {
        OwnedDataValue::from(&v)
    }

    fn fresh_message(initial: serde_json::Value) -> Message {
        // Build a message whose context's `data` field starts as `initial`.
        Message::builder().data(dv(initial)).build()
    }

    #[test]
    fn test_map_config_from_json() {
        let input = json!({
            "mappings": [
                { "path": "data.field1", "logic": {"var": "data.source"} },
                { "path": "data.field2", "logic": "static_value" }
            ]
        });

        let config = MapConfig::from_json(&input).unwrap();
        assert_eq!(config.mappings.len(), 2);
        assert_eq!(config.mappings[0].path.as_json(), &json!("data.field1"));
        assert_eq!(config.mappings[1].path.as_json(), &json!("data.field2"));
    }

    #[test]
    fn test_map_config_missing_mappings() {
        assert!(MapConfig::from_json(&json!({})).is_err());
    }

    #[test]
    fn test_map_config_invalid_mappings() {
        assert!(MapConfig::from_json(&json!({"mappings": "not_an_array"})).is_err());
    }

    #[test]
    fn test_map_config_missing_path() {
        let input = json!({"mappings": [{"logic": {"var": "data.source"}}]});
        assert!(MapConfig::from_json(&input).is_err());
    }

    #[test]
    fn test_map_config_missing_logic() {
        let input = json!({"mappings": [{"path": "data.field1"}]});
        assert!(MapConfig::from_json(&input).is_err());
    }

    /// Helper that compiles each mapping's `logic` and stamps the resulting
    /// `Arc<Logic>` into the `compiled_logic` slot — mirroring what
    /// `LogicCompiler` does at engine construction.
    fn compile_mappings(engine: &Arc<Engine>, config: &mut MapConfig) {
        for mapping in &mut config.mappings {
            mapping.compiled_logic = Some(engine.compile_arc(&mapping.logic).unwrap());
        }
    }

    #[test]
    fn test_map_metadata_assignment() {
        let engine = Arc::new(crate::engine::compiler::datalogic_engine_builder().build());

        let mut message = fresh_message(json!({
            "SwiftMT": { "message_type": "103" }
        }));

        let mut config = MapConfig {
            mappings: vec![MapMapping {
                path: PathTemplate::from("metadata.SwiftMT.message_type"),
                logic: json!({"var": "data.SwiftMT.message_type"}),
                ..Default::default()
            }],
        };
        compile_mappings(&engine, &mut config);

        let result = config.execute(&mut message, &engine);
        assert!(result.is_ok());

        let (outcome, changes) = result.unwrap();
        assert_eq!(outcome, TaskOutcome::Success);
        assert_eq!(changes.len(), 1);

        assert_eq!(
            message.context["metadata"]
                .get("SwiftMT")
                .and_then(|v| v.get("message_type")),
            Some(&dv(json!("103")))
        );
    }

    #[test]
    fn test_map_null_values_skip_assignment() {
        let engine = Arc::new(crate::engine::compiler::datalogic_engine_builder().build());

        let mut message = fresh_message(json!({ "existing_field": "should_remain" }));
        set_nested_value(
            &mut message.context,
            "metadata",
            dv(json!({"existing_meta": "should_remain"})),
        );

        let mut config = MapConfig {
            mappings: vec![
                MapMapping {
                    path: PathTemplate::from("data.new_field"),
                    logic: json!({"var": "data.non_existent_field"}),
                    ..Default::default()
                },
                MapMapping {
                    path: PathTemplate::from("metadata.new_meta"),
                    logic: json!({"var": "data.another_non_existent"}),
                    ..Default::default()
                },
                MapMapping {
                    path: PathTemplate::from("data.actual_field"),
                    logic: json!("actual_value"),
                    ..Default::default()
                },
            ],
        };
        compile_mappings(&engine, &mut config);

        let result = config.execute(&mut message, &engine);
        assert!(result.is_ok());

        let (outcome, changes) = result.unwrap();
        assert_eq!(outcome, TaskOutcome::Success);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path.as_ref(), "data.actual_field");

        assert_eq!(message.context["data"].get("new_field"), None);
        assert_eq!(message.context["metadata"].get("new_meta"), None);

        assert_eq!(
            message.context["data"].get("existing_field"),
            Some(&dv(json!("should_remain")))
        );
        assert_eq!(
            message.context["metadata"].get("existing_meta"),
            Some(&dv(json!("should_remain")))
        );

        assert_eq!(
            message.context["data"].get("actual_field"),
            Some(&dv(json!("actual_value")))
        );
    }

    #[test]
    fn test_map_execute_with_trace_captures_context_snapshots() {
        let engine = Arc::new(crate::engine::compiler::datalogic_engine_builder().build());

        let mut message = fresh_message(json!({ "first": "Alice", "last": "Smith" }));

        let mut config = MapConfig {
            mappings: vec![
                MapMapping {
                    path: PathTemplate::from("data.full_name"),
                    logic: json!({"cat": [{"var": "data.first"}, " ", {"var": "data.last"}]}),
                    ..Default::default()
                },
                MapMapping {
                    path: PathTemplate::from("data.greeting"),
                    logic: json!({"cat": ["Hello, ", {"var": "data.full_name"}]}),
                    ..Default::default()
                },
            ],
        };
        compile_mappings(&engine, &mut config);

        let mut context_snapshots: Vec<Value> = Vec::new();
        let result = with_arena(|arena| {
            let mut arena_ctx = ArenaContext::from_owned(&message.context, arena);
            config.execute_in_arena(
                &mut message,
                &mut arena_ctx,
                &engine,
                Some(&mut context_snapshots),
            )
        });
        assert!(result.is_ok());

        let (outcome, changes) = result.unwrap();
        assert_eq!(outcome, TaskOutcome::Success);
        assert_eq!(changes.len(), 2);
        assert_eq!(context_snapshots.len(), 2);

        // Snapshots are `serde_json::Value` for the trace surface.
        assert!(context_snapshots[0]["data"].get("full_name").is_none());
        assert_eq!(
            context_snapshots[1]["data"].get("full_name"),
            Some(&json!("Alice Smith"))
        );
    }

    fn parse(mapping: serde_json::Value) -> std::result::Result<MapMapping, String> {
        MapMapping::deserialize(&mapping).map_err(|e| e.to_string())
    }

    #[test]
    fn the_rules_between_mapping_keys() {
        let unset = parse(json!({"path": "temp_data.x", "unset": true})).unwrap();
        assert!(unset.unset);
        assert_eq!(unset.logic, Value::Null);
        assert_eq!(unset.on_null, OnNull::Skip);

        let on_null =
            parse(json!({"path": "temp_data.x", "logic": 1, "on_null": "unset"})).unwrap();
        assert_eq!(on_null.on_null, OnNull::Unset);

        // Back-compat: an explicit null is still accepted, and still skipped.
        assert!(parse(json!({"path": "data.x", "logic": null})).is_ok());

        for (bad, names) in [
            (json!({"path": "data.x"}), "`logic`"),
            (
                json!({"path": "data.x", "logic": 1, "unset": true}),
                "`unset`",
            ),
            (
                json!({"path": "data.x", "logic": null, "unset": true}),
                "`unset`",
            ),
            (
                json!({"path": "data.x", "unset": true, "on_null": "skip"}),
                "`on_null`",
            ),
            (json!({"path": "data", "unset": true}), "`path`"),
            (json!({"path": "#temp_data", "unset": true}), "`path`"),
            (
                json!({"path": "metadata", "logic": 1, "on_null": "unset"}),
                "`path`",
            ),
            (
                json!({"path": "data.x", "unset": true, "mode": "append"}),
                "`mode`",
            ),
            (
                json!({"path": "data", "logic": 1, "mode": "append"}),
                "`path`",
            ),
            (
                json!({"path": "temp_data", "logic": [1], "mode": "extend"}),
                "`path`",
            ),
        ] {
            let err = parse(bad.clone()).expect_err(&format!("{bad} should be refused"));
            assert!(err.contains(&format!("mapping {names}")), "{bad}: {err}");
        }

        let append = parse(json!({"path": "data.log", "logic": 1, "mode": "append"})).unwrap();
        assert_eq!(append.mode, MapMode::Append);
        assert_eq!(
            parse(json!({"path": "data.x", "logic": 1})).unwrap().mode,
            MapMode::Set
        );

        // A root is only refused for a removal or an append; writing one
        // still merges.
        assert!(parse(json!({"path": "data", "logic": {"a": 1}})).is_ok());
        assert!(parse(json!({"path": "data", "logic": {"a": 1}, "mode": "set"})).is_ok());
        assert!(parse(json!({"path": "data", "logic": 1, "on_null": "skip"})).is_ok());
    }

    #[test]
    fn from_json_applies_the_same_rules() {
        assert!(
            MapConfig::from_json(&json!({"mappings": [{"path": "data.x", "unset": true}]}))
                .unwrap()
                .mappings[0]
                .unset
        );
        assert!(
            MapConfig::from_json(
                &json!({"mappings": [{"path": "data.x", "logic": 1, "unset": true}]})
            )
            .is_err()
        );
    }

    /// Every removal shape through the arena path, so the unit-test-only
    /// differential check in `apply_removal_parts` compares the cache against
    /// a rebuild after each one.
    #[test]
    fn removals_keep_the_arena_cache_in_step() {
        let engine = Arc::new(crate::engine::compiler::datalogic_engine_builder().build());
        let mut message = fresh_message(json!({
            "a": {"b": {"c": 1, "d": 2}},
            "items": [1, 2, 3],
            "top": 1,
            "7": "hashed"
        }));
        set_nested_value(&mut message.context, "extra_root", dv(json!({"k": 1})));

        let mut config = MapConfig {
            mappings: [
                "data.a.b.c",
                "data.items.1",
                "data.top",
                "data.#7",
                "extra_root",
                "data.not.there",
            ]
            .into_iter()
            .map(|path| MapMapping {
                path: PathTemplate::from(path),
                unset: true,
                ..Default::default()
            })
            .chain([MapMapping {
                path: PathTemplate::from("data.a.b.d"),
                logic: json!({"var": "data.nope"}),
                on_null: OnNull::Unset,
                ..Default::default()
            }])
            .collect(),
        };
        compile_mappings(&engine, &mut config);

        let (outcome, changes) = config.execute(&mut message, &engine).unwrap();
        assert_eq!(outcome, TaskOutcome::Success);
        assert_eq!(
            Value::from(&message.context["data"]),
            json!({"a": {"b": {}}, "items": [1, 3]})
        );
        assert!(message.context.get("extra_root").is_none());
        assert_eq!(changes.len(), 6, "the absent path records nothing");
        assert!(changes.iter().all(|c| c.removed));
    }

    /// Every append shape through one arena session, with a later mapping
    /// reading each result, so a stale cache would show. The write-through
    /// path runs the unit-test differential check after each write; the
    /// fallback path is checked against a rebuild at the end.
    #[test]
    fn appends_keep_the_arena_cache_in_step() {
        let engine = Arc::new(crate::engine::compiler::datalogic_engine_builder().build());
        let mut message = fresh_message(json!({
            "nul": null,
            "a": {"b": {"list": [1]}},
            "nested": [[1]],
            "top": 5
        }));
        let m = |path: &str, logic: Value, mode: MapMode| MapMapping {
            path: PathTemplate::from(path),
            logic,
            mode,
            ..Default::default()
        };
        let mut config = MapConfig {
            mappings: vec![
                m("data.log", json!({"id": 0}), MapMode::Append),
                m("data.log", json!({"id": 1}), MapMode::Append),
                m("data.log", json!([2, 3]), MapMode::Extend),
                m("data.seen", json!({"var": "data.log"}), MapMode::Set),
                // Append pushes an array result as one element.
                m("data.log", json!([4]), MapMode::Append),
                m("data.nul", json!("x"), MapMode::Append),
                m("data.a.b.list", json!(2), MapMode::Append),
                m(
                    "data.a.b.seen",
                    json!({"var": "data.a.b.list"}),
                    MapMode::Set,
                ),
                m("data.nested.0", json!(2), MapMode::Append),
                m(
                    "data.nested_seen",
                    json!({"var": "data.nested"}),
                    MapMode::Set,
                ),
                m("data.empty", json!([]), MapMode::Extend),
                m(
                    "data.skipped",
                    json!({"var": "data.missing"}),
                    MapMode::Append,
                ),
            ],
        };
        compile_mappings(&engine, &mut config);

        with_arena(|arena| {
            let mut arena_ctx = ArenaContext::from_owned(&message.context, arena);
            let (outcome, changes) = config
                .execute_in_arena(&mut message, &mut arena_ctx, &engine, None)
                .unwrap();
            assert_eq!(outcome, TaskOutcome::Success);
            assert_eq!(
                arena_ctx.as_data_value().to_owned(),
                ArenaContext::from_owned(&message.context, arena)
                    .as_data_value()
                    .to_owned(),
                "the cache matches a rebuild"
            );
            let paths: Vec<&str> = changes
                .iter()
                .filter(|c| c.path.starts_with("data.log"))
                .map(|c| c.path.as_ref())
                .collect();
            assert_eq!(
                paths,
                [
                    "data.log.0",
                    "data.log.1",
                    "data.log.2",
                    "data.log.3",
                    "data.log.4"
                ]
            );
            assert!(changes.iter().all(|c| c.old_value == OwnedDataValue::Null));
        });

        let log = json!([{"id": 0}, {"id": 1}, 2, 3, [4]]);
        assert_eq!(
            Value::from(&message.context["data"]),
            json!({
                "nul": ["x"],
                "a": {"b": {"list": [1, 2], "seen": [1, 2]}},
                "nested": [[1, 2]],
                "top": 5,
                "log": log,
                "seen": [{"id": 0}, {"id": 1}, 2, 3],
                "nested_seen": [[1, 2]],
                "empty": []
            })
        );
    }

    #[test]
    fn an_append_to_a_non_array_or_an_extend_of_a_non_array_fails_the_mapping() {
        let engine = Arc::new(crate::engine::compiler::datalogic_engine_builder().build());
        for (mapping, data) in [
            (
                json!({"path": "data.top", "logic": 1, "mode": "append"}),
                json!({"top": 5}),
            ),
            (
                json!({"path": "data.obj", "logic": [1], "mode": "extend"}),
                json!({"obj": {"k": 1}}),
            ),
            (
                json!({"path": "data.log", "logic": 1, "mode": "extend"}),
                json!({"log": []}),
            ),
            (
                json!({"path": {"cat": ["da", "ta"]}, "logic": 1, "mode": "append"}),
                json!({}),
            ),
        ] {
            let mut message = fresh_message(data.clone());
            let mut config = MapConfig::from_json(&json!({"mappings": [mapping]})).unwrap();
            compile_mappings(&engine, &mut config);
            for m in &mut config.mappings {
                m.path
                    .compile(
                        &crate::engine::functions::TemplateCompiler::new(engine.clone()),
                        "path",
                    )
                    .unwrap();
            }
            let (outcome, changes) = config.execute(&mut message, &engine).unwrap();
            assert_eq!(outcome, TaskOutcome::Status(500), "{mapping}");
            assert!(changes.is_empty(), "{mapping}");
            assert_eq!(Value::from(&message.context["data"]), data, "{mapping}");
        }
    }

    #[test]
    fn test_map_multiple_fields_including_metadata() {
        let engine = Arc::new(crate::engine::compiler::datalogic_engine_builder().build());

        let mut message = fresh_message(json!({
            "ISO20022_MX": {
                "document": {
                    "TxInf": {
                        "OrgnlGrpInf": { "OrgnlMsgNmId": "pacs.008.001.08" }
                    }
                }
            },
            "SwiftMT": { "message_type": "103" }
        }));

        let mut config = MapConfig {
            mappings: vec![
                MapMapping {
                    path: PathTemplate::from("data.SwiftMT.message_type"),
                    logic: json!("103"),
                    ..Default::default()
                },
                MapMapping {
                    path: PathTemplate::from("metadata.SwiftMT.message_type"),
                    logic: json!({"var": "data.SwiftMT.message_type"}),
                    ..Default::default()
                },
                MapMapping {
                    path: PathTemplate::from("temp_data.original_msg_type"),
                    logic: json!({"var": "data.ISO20022_MX.document.TxInf.OrgnlGrpInf.OrgnlMsgNmId"}),
                    ..Default::default()
                },
            ],
        };
        compile_mappings(&engine, &mut config);

        let result = config.execute(&mut message, &engine);
        assert!(result.is_ok());

        let (outcome, changes) = result.unwrap();
        assert_eq!(outcome, TaskOutcome::Success);
        assert_eq!(changes.len(), 3);

        assert_eq!(
            message.context["data"]
                .get("SwiftMT")
                .and_then(|v| v.get("message_type")),
            Some(&dv(json!("103")))
        );
        assert_eq!(
            message.context["metadata"]
                .get("SwiftMT")
                .and_then(|v| v.get("message_type")),
            Some(&dv(json!("103")))
        );
        assert_eq!(
            message.context["temp_data"].get("original_msg_type"),
            Some(&dv(json!("pacs.008.001.08")))
        );
    }
}
