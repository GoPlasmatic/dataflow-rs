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
//! - Null results skip assignment
//! - Audit-trail change tracking

use crate::engine::error::{DataflowError, Result};
use crate::engine::executor::{ArenaContext, with_arena};
use crate::engine::message::{Change, Message};
use crate::engine::task_outcome::TaskOutcome;
use crate::engine::utils::{get_nested_value_parts, set_nested_value_parts};
use datalogic_rs::{Engine, Logic};
use datavalue::OwnedDataValue;
use log::{debug, error};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// Configuration for the map function containing a list of mappings.
#[derive(Debug, Clone, Deserialize)]
pub struct MapConfig {
    /// List of mappings to execute in order.
    pub mappings: Vec<MapMapping>,
}

/// A single mapping that transforms and assigns data.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct MapMapping {
    /// Target path where the result will be stored (e.g., "data.user.name").
    /// Supports dot notation for nested paths and `#` prefix for numeric field names.
    pub path: String,

    /// JSONLogic expression (kept as `serde_json::Value` since this is the
    /// shape the compiler accepts; not runtime data).
    pub logic: Value,

    /// Engine-internal: pre-compiled JSONLogic, populated by `LogicCompiler`.
    /// `None` is logged as an error during execute (the compiler should always
    /// populate it). Not part of the stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub compiled_logic: Option<Arc<Logic>>,

    /// Engine-internal: `Arc<str>` mirror of `path`, populated by
    /// `LogicCompiler`. Cloned (refcount bump) into `Change.path` per audit
    /// emission so the hot path avoids `Arc::from(&path)` allocations.
    /// Not part of the stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub path_arc: Arc<str>,

    /// Engine-internal: pre-split path segments (with the `#`-prefix escape
    /// already applied, matching `utils::strip_hash_prefix`). Populated by
    /// `LogicCompiler`. The hot path consumes this directly instead of running
    /// `path.split('.')` — saves ~3% on `CharSearcher::next_match` per the
    /// flamegraph. Not part of the stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub path_parts: Arc<[Arc<str>]>,
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
            let path = mapping
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| DataflowError::Validation("Missing 'path' in mapping".to_string()))?
                .to_string();

            let logic = mapping
                .get("logic")
                .ok_or_else(|| DataflowError::Validation("Missing 'logic' in mapping".to_string()))?
                .clone();

            parsed_mappings.push(MapMapping {
                path_arc: Arc::from(path.as_str()),
                path_parts: Arc::from(Vec::<Arc<str>>::new().into_boxed_slice()),
                path,
                logic,
                compiled_logic: None,
            });
        }

        Ok(MapConfig {
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
    /// `trace_snapshots` (when `Some`) collects a `serde_json::Value` snapshot
    /// of `message.context` *before* each mapping runs — the trace
    /// surface uses this for per-mapping debugging. `None` skips the snapshot
    /// work entirely (the production path).
    pub(crate) fn execute_in_arena(
        &self,
        message: &mut Message,
        arena_ctx: &mut ArenaContext<'_>,
        engine: &Arc<Engine>,
        mut trace_snapshots: Option<&mut Vec<Value>>,
    ) -> Result<(TaskOutcome, Vec<Change>)> {
        let mut changes = Vec::new();
        let mut errors_encountered = false;

        debug!("Map: Executing {} mappings", self.mappings.len());

        let arena = arena_ctx.arena();
        for mapping in &self.mappings {
            debug!("Processing mapping to path: {}", mapping.path);

            // Trace mode: snapshot the context as a serde_json::Value *before*
            // applying this mapping. Bridge cost is acceptable on the debug
            // surface; production callers pass `None` and skip it entirely.
            if let Some(buf) = trace_snapshots.as_deref_mut() {
                buf.push(Value::from(&message.context));
            }

            // Pre-compiled `Arc<Logic>` lives on the mapping; the workflow
            // compiler always populates it. `None` only happens for mappings
            // constructed directly without compilation (test surface) —
            // logged and skipped here.
            let compiled_logic = match &mapping.compiled_logic {
                Some(logic) => logic,
                None => {
                    error!("Map: Logic not compiled for mapping to {}", mapping.path);
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
                        mapping.path, e
                    );
                    errors_encountered = true;
                    continue;
                }
            };

            let transformed_value = result_av.to_owned();
            debug!(
                "Map: Evaluated logic for path {} resulted in: {:?}",
                mapping.path, transformed_value
            );

            if matches!(transformed_value, OwnedDataValue::Null) {
                debug!(
                    "Map: Skipping mapping for path {} as result is null",
                    mapping.path
                );
                continue;
            }

            // Compiler populates `path_parts`. For callers that build a
            // `MapConfig` directly (the test surface and a few in-tree
            // helpers) fall back to splitting on the fly — same semantics,
            // one extra allocation per mapping per call.
            let fallback_parts: Vec<Arc<str>>;
            let parts: &[Arc<str>] = if mapping.path_parts.is_empty() && !mapping.path.is_empty() {
                fallback_parts = mapping.path.split('.').map(Arc::from).collect();
                &fallback_parts
            } else {
                &mapping.path_parts
            };
            let path_arc: Arc<str> = if mapping.path_arc.is_empty() && !mapping.path.is_empty() {
                Arc::from(mapping.path.as_str())
            } else {
                Arc::clone(&mapping.path_arc)
            };

            if message.capture_changes {
                // Audit-on: capture old/new values directly into the `Change`.
                // `Change` owns `OwnedDataValue`s (not `Arc<…>`) — one fewer
                // heap allocation per recorded mutation.
                let old_value = get_nested_value_parts(&message.context, parts)
                    .cloned()
                    .unwrap_or(OwnedDataValue::Null);
                let new_value = transformed_value.clone();

                changes.push(Change {
                    path: path_arc,
                    old_value,
                    new_value,
                });
            }
            arena_ctx.apply_mutation_parts(&mut message.context, parts, |ctx| {
                apply_mapping_parts(ctx, parts, &mapping.path, transformed_value);
            });
            debug!("Successfully mapped to path: {}", mapping.path);
        }

        let outcome = if errors_encountered {
            TaskOutcome::Status(500)
        } else {
            TaskOutcome::Success
        };
        Ok((outcome, changes))
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
        let mut m = Message::new(Arc::new(dv(json!({}))));
        set_nested_value(&mut m.context, "data", dv(initial));
        m
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
        assert_eq!(config.mappings[0].path, "data.field1");
        assert_eq!(config.mappings[1].path, "data.field2");
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
        let engine = Arc::new(Engine::builder().with_templating(true).build());

        let mut message = fresh_message(json!({
            "SwiftMT": { "message_type": "103" }
        }));

        let mut config = MapConfig {
            mappings: vec![MapMapping {
                path: "metadata.SwiftMT.message_type".to_string(),
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
        let engine = Arc::new(Engine::builder().with_templating(true).build());

        let mut message = fresh_message(json!({ "existing_field": "should_remain" }));
        set_nested_value(
            &mut message.context,
            "metadata",
            dv(json!({"existing_meta": "should_remain"})),
        );

        let mut config = MapConfig {
            mappings: vec![
                MapMapping {
                    path: "data.new_field".to_string(),
                    logic: json!({"var": "data.non_existent_field"}),
                    ..Default::default()
                },
                MapMapping {
                    path: "metadata.new_meta".to_string(),
                    logic: json!({"var": "data.another_non_existent"}),
                    ..Default::default()
                },
                MapMapping {
                    path: "data.actual_field".to_string(),
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
        let engine = Arc::new(Engine::builder().with_templating(true).build());

        let mut message = fresh_message(json!({ "first": "Alice", "last": "Smith" }));

        let mut config = MapConfig {
            mappings: vec![
                MapMapping {
                    path: "data.full_name".to_string(),
                    logic: json!({"cat": [{"var": "data.first"}, " ", {"var": "data.last"}]}),
                    ..Default::default()
                },
                MapMapping {
                    path: "data.greeting".to_string(),
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

    #[test]
    fn test_map_multiple_fields_including_metadata() {
        let engine = Arc::new(Engine::builder().with_templating(true).build());

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
                    path: "data.SwiftMT.message_type".to_string(),
                    logic: json!("103"),
                    ..Default::default()
                },
                MapMapping {
                    path: "metadata.SwiftMT.message_type".to_string(),
                    logic: json!({"var": "data.SwiftMT.message_type"}),
                    ..Default::default()
                },
                MapMapping {
                    path: "temp_data.original_msg_type".to_string(),
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
