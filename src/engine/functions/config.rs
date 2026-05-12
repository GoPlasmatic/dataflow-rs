use crate::engine::error::Result;
use crate::engine::executor::ArenaContext;
use crate::engine::functions::filter::FilterConfig;
use crate::engine::functions::integration::{EnrichConfig, HttpCallConfig, PublishKafkaConfig};
use crate::engine::functions::log::LogConfig;
use crate::engine::functions::map::MapConfig;
use crate::engine::functions::parse::{
    ParseConfig, execute_parse_json_in_arena, execute_parse_xml,
};
use crate::engine::functions::publish::{PublishConfig, execute_publish_json, execute_publish_xml};
use crate::engine::functions::validation::ValidationConfig;
use crate::engine::message::{Change, Message};
use crate::engine::task_outcome::TaskOutcome;
use datalogic_rs::Engine;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::any::Any;
use std::sync::Arc;

/// Pre-parsed typed input for a `FunctionConfig::Custom` task. Populated by
/// the engine at `Engine::new()` time by calling the registered
/// `AsyncFunctionHandler::parse_input` for the named function. Cached as
/// `Arc<dyn Any>` so the dispatch path can hand it to the handler with a
/// single `downcast_ref` (O(1)) and zero per-message deserialization cost.
///
/// The wrapper exists because `dyn Any` does not implement `Debug`, which
/// would otherwise prevent `#[derive(Debug)]` on `FunctionConfig`.
#[derive(Clone)]
pub struct CompiledCustomInput(pub Arc<dyn Any + Send + Sync>);

impl CompiledCustomInput {
    /// Borrow the inner value as `&(dyn Any + Send + Sync)` for handoff to
    /// `DynAsyncFunctionHandler::dyn_execute`.
    #[inline]
    pub fn as_any(&self) -> &(dyn Any + Send + Sync) {
        &*self.0
    }
}

impl std::fmt::Debug for CompiledCustomInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CompiledCustomInput(<opaque>)")
    }
}

/// Enum containing all possible function configurations.
///
/// Deserialization dispatches on the `name` field: each known built-in
/// (`map`, `validate`, `parse_json`, …) parses its `input` strictly into the
/// matching typed config and errors with a clear envelope (`config for
/// function 'map': missing field 'mappings'`). Unknown names fall through
/// to [`FunctionConfig::Custom`], which preserves the raw input for a
/// user-registered handler to consume at engine construction time.
#[derive(Debug, Clone)]
pub enum FunctionConfig {
    Map {
        name: MapName,
        input: MapConfig,
    },
    Validation {
        name: ValidationName,
        input: ValidationConfig,
    },
    ParseJson {
        name: ParseJsonName,
        input: ParseConfig,
    },
    ParseXml {
        name: ParseXmlName,
        input: ParseConfig,
    },
    PublishJson {
        name: PublishJsonName,
        input: PublishConfig,
    },
    PublishXml {
        name: PublishXmlName,
        input: PublishConfig,
    },
    Filter {
        name: FilterName,
        input: FilterConfig,
    },
    Log {
        name: LogName,
        input: LogConfig,
    },
    HttpCall {
        name: HttpCallName,
        input: HttpCallConfig,
    },
    Enrich {
        name: EnrichName,
        input: EnrichConfig,
    },
    PublishKafka {
        name: PublishKafkaName,
        input: PublishKafkaConfig,
    },
    /// For custom or unknown functions, store raw input and a slot for the
    /// pre-parsed typed value populated at engine construction time.
    Custom {
        name: String,
        input: Value,
        /// Pre-parsed `<RegisteredHandler as AsyncFunctionHandler>::Input`,
        /// boxed as `dyn Any`. Set by the engine after handler registration;
        /// `None` on initial deserialization. `FunctionConfig` is
        /// deserialize-only — round-tripping a workflow through JSON
        /// re-parses on the next `Engine::new()` call.
        compiled_input: Option<CompiledCustomInput>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MapName {
    Map,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ValidationName {
    Validation,
    Validate,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ParseJsonName {
    ParseJson,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ParseXmlName {
    ParseXml,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PublishJsonName {
    PublishJson,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PublishXmlName {
    PublishXml,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FilterName {
    Filter,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LogName {
    Log,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HttpCallName {
    HttpCall,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EnrichName {
    Enrich,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PublishKafkaName {
    PublishKafka,
}

/// Names of the built-in function variants — used in error messages and as
/// the discriminator for [`FunctionConfig`] deserialization. Kept in one
/// place so adding a new built-in updates the dispatch, the error suggestion
/// list, and the docs in lockstep.
pub(crate) const BUILTIN_FUNCTION_NAMES: &[&str] = &[
    "map",
    "validation",
    "validate",
    "parse_json",
    "parse_xml",
    "publish_json",
    "publish_xml",
    "filter",
    "log",
    "http_call",
    "enrich",
    "publish_kafka",
];

/// Parse a `serde_json::Value` into a typed config, wrapping any error in a
/// "config for function '<func>': …" envelope. Strips the trailing
/// `" at line 0 column 0"` that `serde_json::from_value` always appends
/// (since the source `Value` has no source-text location); the outer
/// deserializer re-attaches the real source location when this error
/// bubbles up to e.g. `Workflow::from_json`.
fn parse_function_input<T, E>(func: &str, input: Value) -> std::result::Result<T, E>
where
    T: DeserializeOwned,
    E: serde::de::Error,
{
    serde_json::from_value::<T>(input).map_err(|err| {
        let raw = err.to_string();
        let trimmed = raw
            .rsplit_once(" at line ")
            .map(|(head, _)| head)
            .unwrap_or(&raw);
        E::custom(format!("config for function '{func}': {trimmed}"))
    })
}

impl<'de> Deserialize<'de> for FunctionConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Tag-only intermediate. Format-agnostic: works for any deserializer
        // that produces `String`/`serde_json::Value`. The strict typed parse
        // happens in the dispatch below.
        #[derive(Deserialize)]
        struct Raw {
            name: String,
            input: Value,
        }

        let Raw { name, input } = Raw::deserialize(deserializer)?;

        Ok(match name.as_str() {
            "map" => FunctionConfig::Map {
                name: MapName::Map,
                input: parse_function_input("map", input)?,
            },
            "validate" => FunctionConfig::Validation {
                name: ValidationName::Validate,
                input: parse_function_input("validate", input)?,
            },
            "validation" => FunctionConfig::Validation {
                name: ValidationName::Validation,
                input: parse_function_input("validation", input)?,
            },
            "parse_json" => FunctionConfig::ParseJson {
                name: ParseJsonName::ParseJson,
                input: parse_function_input("parse_json", input)?,
            },
            "parse_xml" => FunctionConfig::ParseXml {
                name: ParseXmlName::ParseXml,
                input: parse_function_input("parse_xml", input)?,
            },
            "publish_json" => FunctionConfig::PublishJson {
                name: PublishJsonName::PublishJson,
                input: parse_function_input("publish_json", input)?,
            },
            "publish_xml" => FunctionConfig::PublishXml {
                name: PublishXmlName::PublishXml,
                input: parse_function_input("publish_xml", input)?,
            },
            "filter" => FunctionConfig::Filter {
                name: FilterName::Filter,
                input: parse_function_input("filter", input)?,
            },
            "log" => FunctionConfig::Log {
                name: LogName::Log,
                input: parse_function_input("log", input)?,
            },
            "http_call" => FunctionConfig::HttpCall {
                name: HttpCallName::HttpCall,
                input: parse_function_input("http_call", input)?,
            },
            "enrich" => FunctionConfig::Enrich {
                name: EnrichName::Enrich,
                input: parse_function_input("enrich", input)?,
            },
            "publish_kafka" => FunctionConfig::PublishKafka {
                name: PublishKafkaName::PublishKafka,
                input: parse_function_input("publish_kafka", input)?,
            },
            _ => FunctionConfig::Custom {
                name,
                input,
                compiled_input: None,
            },
        })
    }
}

impl FunctionConfig {
    /// Get the function name for this configuration
    pub fn function_name(&self) -> &str {
        match self {
            FunctionConfig::Map { .. } => "map",
            FunctionConfig::Validation { .. } => "validate",
            FunctionConfig::ParseJson { .. } => "parse_json",
            FunctionConfig::ParseXml { .. } => "parse_xml",
            FunctionConfig::PublishJson { .. } => "publish_json",
            FunctionConfig::PublishXml { .. } => "publish_xml",
            FunctionConfig::Filter { .. } => "filter",
            FunctionConfig::Log { .. } => "log",
            FunctionConfig::HttpCall { .. } => "http_call",
            FunctionConfig::Enrich { .. } => "enrich",
            FunctionConfig::PublishKafka { .. } => "publish_kafka",
            FunctionConfig::Custom { name, .. } => name,
        }
    }

    /// Whether this is a synchronous built-in. Synchronous built-ins can share
    /// a single `ArenaContext` lifetime across consecutive tasks within a
    /// workflow without crossing any `.await` point.
    ///
    /// Must match the variants handled in [`Self::try_execute_in_arena`]; the
    /// debug assertion below ties the two together so they can't drift.
    pub fn is_sync_builtin(&self) -> bool {
        matches!(
            self,
            FunctionConfig::Map { .. }
                | FunctionConfig::Validation { .. }
                | FunctionConfig::ParseJson { .. }
                | FunctionConfig::ParseXml { .. }
                | FunctionConfig::PublishJson { .. }
                | FunctionConfig::PublishXml { .. }
                | FunctionConfig::Filter { .. }
                | FunctionConfig::Log { .. }
        )
    }

    /// If this config is a sync built-in, execute it against the supplied
    /// arena context and return `Some(result)`. Otherwise return `None` —
    /// the workflow executor uses that as the signal to break the sync
    /// stretch and dispatch the task on the async path instead.
    ///
    /// `map_snapshot_buf` is only consulted by the `Map` variant — when
    /// `Some`, the map function pushes a `serde_json::Value` snapshot of the
    /// context before each mapping (for the trace surface). All other
    /// variants ignore it. Pass `None` from the production path.
    ///
    /// This is the single source of truth for the sync-stretch dispatch:
    /// adding a new sync built-in only requires adding an arm here (and the
    /// matching variant to `is_sync_builtin` above).
    pub(crate) fn try_execute_in_arena(
        &self,
        message: &mut Message,
        arena_ctx: &mut ArenaContext<'_>,
        engine: &Arc<Engine>,
        map_snapshot_buf: Option<&mut Vec<Value>>,
    ) -> Option<Result<(TaskOutcome, Vec<Change>)>> {
        match self {
            FunctionConfig::Map { input, .. } => {
                Some(input.execute_in_arena(message, arena_ctx, engine, map_snapshot_buf))
            }
            FunctionConfig::Validation { input, .. } => {
                Some(input.execute_in_arena(message, arena_ctx, engine))
            }
            FunctionConfig::ParseJson { input, .. } => {
                Some(execute_parse_json_in_arena(message, input, arena_ctx))
            }
            FunctionConfig::ParseXml { input, .. } => {
                // Refresh the arena only on success; on error the arena cache
                // is still in sync with the unchanged context.
                Some(match execute_parse_xml(message, input) {
                    Ok(r) => {
                        arena_ctx.refresh_for_path(&message.context, "data");
                        Ok(r)
                    }
                    Err(e) => Err(e),
                })
            }
            FunctionConfig::PublishJson { input, .. } => {
                // publish writes to `data.<target>` but goes through
                // `set_nested_value` on the owned context — refresh the
                // arena slot afterwards so the next task in the stretch
                // observes the new value.
                Some(match execute_publish_json(message, input) {
                    Ok(r) => {
                        arena_ctx.refresh_for_path(&message.context, "data");
                        Ok(r)
                    }
                    Err(e) => Err(e),
                })
            }
            FunctionConfig::PublishXml { input, .. } => {
                Some(match execute_publish_xml(message, input) {
                    Ok(r) => {
                        arena_ctx.refresh_for_path(&message.context, "data");
                        Ok(r)
                    }
                    Err(e) => Err(e),
                })
            }
            FunctionConfig::Filter { input, .. } => {
                Some(input.execute_in_arena(message, arena_ctx, engine))
            }
            FunctionConfig::Log { input, .. } => {
                Some(input.execute_in_arena(message, arena_ctx, engine))
            }
            FunctionConfig::HttpCall { .. }
            | FunctionConfig::Enrich { .. }
            | FunctionConfig::PublishKafka { .. }
            | FunctionConfig::Custom { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: serde_json::Value) -> std::result::Result<FunctionConfig, serde_json::Error> {
        serde_json::from_value(value)
    }

    #[test]
    fn map_with_valid_config_deserializes_to_map_variant() {
        let cfg = parse(json!({
            "name": "map",
            "input": {
                "mappings": [
                    { "path": "data.x", "logic": { "var": "data.y" } }
                ]
            }
        }))
        .expect("valid map config should deserialize");
        assert!(matches!(cfg, FunctionConfig::Map { .. }));
    }

    #[test]
    fn map_with_missing_mappings_gives_clear_error() {
        let err = parse(json!({
            "name": "map",
            "input": {}
        }))
        .expect_err("map with empty input should fail");
        let msg = err.to_string();
        assert!(
            msg.starts_with("config for function 'map':"),
            "error should be prefixed with function envelope, got: {msg}"
        );
        assert!(
            msg.contains("mappings"),
            "error should mention the missing field, got: {msg}"
        );
    }

    #[test]
    fn map_with_wrong_input_shape_gives_clear_error() {
        let err = parse(json!({
            "name": "map",
            "input": { "mappings": "not an array" }
        }))
        .expect_err("map with bad mappings type should fail");
        let msg = err.to_string();
        assert!(
            msg.starts_with("config for function 'map':"),
            "error should be prefixed with function envelope, got: {msg}"
        );
    }

    #[test]
    fn validation_accepts_both_spellings() {
        for name in ["validate", "validation"] {
            let cfg = parse(json!({
                "name": name,
                "input": { "rules": [] }
            }))
            .unwrap_or_else(|e| panic!("'{name}' should deserialize: {e}"));
            assert!(matches!(cfg, FunctionConfig::Validation { .. }));
        }
    }

    #[test]
    fn unknown_name_falls_through_to_custom() {
        let cfg = parse(json!({
            "name": "my_custom_handler",
            "input": { "anything": "goes" }
        }))
        .expect("unknown name should produce Custom");
        match cfg {
            FunctionConfig::Custom {
                name,
                compiled_input,
                ..
            } => {
                assert_eq!(name, "my_custom_handler");
                assert!(compiled_input.is_none());
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn missing_name_field_errors() {
        let err = parse(json!({ "input": {} })).expect_err("missing name should fail");
        assert!(err.to_string().contains("name"));
    }

    #[test]
    fn missing_input_field_errors() {
        let err = parse(json!({ "name": "map" })).expect_err("missing input should fail");
        assert!(err.to_string().contains("input"));
    }

    #[test]
    fn http_call_with_missing_connector_gives_clear_error() {
        let err = parse(json!({
            "name": "http_call",
            "input": { "method": "GET" }
        }))
        .expect_err("http_call needs connector");
        let msg = err.to_string();
        assert!(
            msg.starts_with("config for function 'http_call':"),
            "error should be prefixed with function envelope, got: {msg}"
        );
        assert!(msg.contains("connector"));
    }

    #[test]
    fn builtin_names_never_fall_through_to_custom() {
        // Every name in BUILTIN_FUNCTION_NAMES must be handled by the
        // dispatch — either parsing successfully or failing with the
        // envelope. None should silently land in Custom.
        for name in BUILTIN_FUNCTION_NAMES {
            let cfg = parse(json!({
                "name": name,
                "input": {}
            }));
            match cfg {
                Ok(c) => assert!(
                    !matches!(c, FunctionConfig::Custom { .. }),
                    "name '{name}' silently fell through to Custom"
                ),
                Err(e) => assert!(
                    e.to_string()
                        .starts_with(&format!("config for function '{name}':")),
                    "name '{name}' failed without envelope: {e}"
                ),
            }
        }
    }
}
