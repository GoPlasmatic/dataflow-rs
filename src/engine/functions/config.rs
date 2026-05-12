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
use serde::Deserialize;
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

/// Enum containing all possible function configurations
/// Uses internally tagged representation for clean deserialization
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
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
        /// `None` until then. Skipped by serde — round-tripping a workflow
        /// through JSON re-parses on the next `Engine::new()` call.
        #[serde(skip)]
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
