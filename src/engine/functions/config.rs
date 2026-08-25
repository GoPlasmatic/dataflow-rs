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

/// Every function name that [`FunctionConfig`]'s deserializer resolves to a
/// typed built-in variant instead of [`FunctionConfig::Custom`].
///
/// Used in error messages and as the discriminator for [`FunctionConfig`]
/// deserialization. Kept in one place so adding a new built-in updates the
/// dispatch, the error suggestion list, and the docs in lockstep.
///
/// Public so a service layer that gates workflow authoring on a closed
/// function set can derive that set rather than copy it. Membership here is
/// **not** the same fact as "this engine can run it" — see
/// [`builtin_function_kind`], and prefer it for that question.
///
/// # Stability
///
/// Names are only added in a minor release and only removed in a major one, so
/// a caller may treat a name that appears here as durable. **Ordering is not
/// meaningful** and may change without notice; treat this as a set. Note that
/// `validation` and `validate` are both present and both resolve to
/// [`FunctionConfig::Validation`].
pub const BUILTIN_FUNCTION_NAMES: &[&str] = &[
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

/// How a built-in function reaches an implementation.
///
/// This is the programmatic form of the distinction
/// `docs/src/built-in-functions/integrations.md` draws in prose: some built-ins
/// this crate executes itself, and for others it only supplies a config schema.
///
/// Deliberately **not** `#[non_exhaustive]`. A caller matching on this is
/// usually deciding whether to accept a workflow definition, and if a third
/// kind is ever added that decision needs revisiting — a compile error at every
/// match site is the correct signal, not a silent fall-through to a `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinKind {
    /// Executed by this crate. Needs no registration; an engine can always run it.
    SelfContained,
    /// Deserializes into a typed built-in variant — so `Engine::new` accepts it
    /// without complaint — but dispatches to a handler registered under the same
    /// name, and fails with [`crate::DataflowError::FunctionNotFound`] on the
    /// first message if none is registered.
    ///
    /// `http_call`, `enrich` and `publish_kafka` are these. A validator that
    /// treats them like [`BuiltinKind::SelfContained`] will green-light a
    /// workflow that builds cleanly and then fails every request.
    RequiresHandler,
}

/// Classify `name` as a built-in function.
///
/// `None` means it is not a built-in: it lands in [`FunctionConfig::Custom`] and
/// needs `Engine::builder().register(name, handler)`.
///
/// Matching is exact, the same as the deserializer dispatch — `"HTTP_CALL"` and
/// `"htttp_call"` are both `None`.
///
/// ```
/// use dataflow_rs::{BuiltinKind, builtin_function_kind};
///
/// assert_eq!(builtin_function_kind("map"), Some(BuiltinKind::SelfContained));
/// assert_eq!(builtin_function_kind("enrich"), Some(BuiltinKind::RequiresHandler));
/// assert_eq!(builtin_function_kind("my_handler"), None);
/// ```
pub fn builtin_function_kind(name: &str) -> Option<BuiltinKind> {
    match name {
        "map" | "validation" | "validate" | "parse_json" | "parse_xml" | "publish_json"
        | "publish_xml" | "filter" | "log" => Some(BuiltinKind::SelfContained),
        "http_call" | "enrich" | "publish_kafka" => Some(BuiltinKind::RequiresHandler),
        _ => None,
    }
}

/// Whether `name` deserializes to a typed built-in variant at all.
///
/// Equivalent to `builtin_function_kind(name).is_some()`. This answers "is this
/// a name the crate special-cases", **not** "can this engine run it" — a
/// [`BuiltinKind::RequiresHandler`] name returns `true` here whether or not a
/// handler is registered.
#[inline]
pub fn is_builtin_function(name: &str) -> bool {
    builtin_function_kind(name).is_some()
}

/// One function an engine will actually dispatch.
///
/// Yielded by [`crate::Engine::dispatchable_functions`] and
/// [`crate::EngineBuilder::dispatchable_functions`]. Together with
/// [`BuiltinKind`] this is the whole authoring-side vocabulary: `kind` says how
/// the name reaches an implementation, and `aliases` says which other spellings
/// resolve to the same one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchableFunction<'a> {
    /// The canonical name. Alternative spellings are listed in
    /// [`Self::aliases`] rather than yielded as separate entries.
    pub name: &'a str,
    /// `Some(..)` for a built-in; `None` for a name backed only by a registered
    /// custom handler.
    ///
    /// The `Option` deliberately mirrors [`builtin_function_kind`], where
    /// `None` already means "not a built-in". [`BuiltinKind`] is not
    /// `#[non_exhaustive]` on purpose, so widening it with a third variant
    /// would break every downstream `match`; this carries the same fact
    /// additively.
    pub kind: Option<BuiltinKind>,
    /// Other accepted spellings of this same function.
    ///
    /// `validate` carries `["validation"]`; every other name is empty today.
    /// An alias never appears as its own entry, but
    /// [`crate::Engine::can_dispatch`] does accept it — a task named
    /// `validation` really does execute.
    pub aliases: &'static [&'static str],
}

/// Aliases of `validate`. Both spellings deserialize to
/// [`FunctionConfig::Validation`]; `function_name()` reports `"validate"`, which
/// makes that the canonical one.
const VALIDATE_ALIASES: &[&str] = &["validation"];

/// The empty alias list, so [`builtin_aliases`] can return a `'static` slice for
/// every name without allocating.
const NO_ALIASES: &[&str] = &[];

/// Map a built-in spelling to the canonical one for its function.
///
/// Every name is its own canonical form except `validation`, which is an alias
/// of `validate`. Deliberately a match rather than a table: the alias relation
/// is the *only* new fact here, and a table of canonical names would be a
/// second copy of [`BUILTIN_FUNCTION_NAMES`] to keep in sync.
pub(crate) fn canonical_builtin_name(name: &str) -> &str {
    match name {
        "validation" => "validate",
        other => other,
    }
}

/// The alternative spellings of `canonical`, which must already be a canonical
/// name. Paired with [`canonical_builtin_name`]; the two are pinned to each
/// other and to [`BUILTIN_FUNCTION_NAMES`] by `aliases_and_canonical_names_agree`.
pub(crate) fn builtin_aliases(canonical: &str) -> &'static [&'static str] {
    match canonical {
        "validate" => VALIDATE_ALIASES,
        _ => NO_ALIASES,
    }
}

/// Whether a registry containing `registry`'s keys will dispatch `name`.
///
/// The single definition of "this engine can run it": a
/// [`BuiltinKind::SelfContained`] built-in always can, and every other name —
/// [`BuiltinKind::RequiresHandler`] built-ins and custom names alike — can only
/// if a handler is registered under it. `TaskExecutor::has_function` and the
/// two public `can_dispatch` methods all route through here so the predicate
/// the engine dispatches on and the predicate hosts query cannot drift.
///
/// Generic over the map's value type so this module needs no dependency on
/// `BoxedFunctionHandler`.
pub(crate) fn can_dispatch_in<V>(
    registry: &std::collections::HashMap<String, V>,
    name: &str,
) -> bool {
    match builtin_function_kind(name) {
        Some(BuiltinKind::SelfContained) => true,
        // RequiresHandler and Custom alike: only if a handler was registered.
        _ => registry.contains_key(name),
    }
}

/// Every function a registry with these keys will dispatch.
///
/// Built-ins are yielded only when they are their own canonical name, which
/// performs the alias grouping with no list to maintain. `RequiresHandler`
/// built-ins appear only when backed by a registration; custom keys appear with
/// `kind: None`.
///
/// A key that names a [`BuiltinKind::SelfContained`] built-in is skipped on the
/// registry side — it is already yielded as a built-in, and the registration
/// itself is inert (the deserializer routes `map` to [`FunctionConfig::Map`],
/// which this crate executes without consulting the registry).
pub(crate) fn dispatchable_functions_in<V>(
    registry: &std::collections::HashMap<String, V>,
) -> impl Iterator<Item = DispatchableFunction<'_>> {
    let builtins = BUILTIN_FUNCTION_NAMES
        .iter()
        .copied()
        // Skip aliases: `validation` is reported under `validate`.
        .filter(|name| canonical_builtin_name(name) == *name)
        .filter_map(move |name| match builtin_function_kind(name) {
            // Always runnable, registered or not.
            kind @ Some(BuiltinKind::SelfContained) => Some(DispatchableFunction {
                name,
                kind,
                aliases: builtin_aliases(name),
            }),
            // Config schema only — present iff a handler backs it.
            kind @ Some(BuiltinKind::RequiresHandler) if registry.contains_key(name) => {
                Some(DispatchableFunction {
                    name,
                    kind,
                    aliases: builtin_aliases(name),
                })
            }
            _ => None,
        });

    let customs = registry
        .keys()
        .map(String::as_str)
        // Built-in names are handled above; a registration under one is either
        // already counted (RequiresHandler) or inert (SelfContained).
        .filter(|name| builtin_function_kind(name).is_none())
        .map(|name| DispatchableFunction {
            name,
            kind: None,
            aliases: NO_ALIASES,
        });

    builtins.chain(customs)
}

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

/// Refresh the arena's `"data"` slot on `Ok`, then return `result` unchanged.
/// Shared by the three built-ins (`parse_xml`, `publish_json`, `publish_xml`)
/// that write through `set_nested_value` on the owned context rather than the
/// arena — the arena cache would otherwise miss that write for the rest of the
/// sync stretch. On `Err` the context didn't change, so the cache is already
/// in sync and is left alone.
fn refresh_data_on_success(
    message: &Message,
    arena_ctx: &mut ArenaContext<'_>,
    result: Result<(TaskOutcome, Vec<Change>)>,
) -> Result<(TaskOutcome, Vec<Change>)> {
    if result.is_ok() {
        arena_ctx.refresh_for_path(&message.context, "data");
    }
    result
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
    /// The connector this task references, if any.
    ///
    /// The three integration variants return their typed `connector` field
    /// verbatim — including an empty string. Whether an empty connector name is
    /// acceptable is a validation question for the host, not this accessor's.
    ///
    /// [`FunctionConfig::Custom`] returns `input["connector"]` when that key
    /// holds a string. That is the convention for service-registered integration
    /// handlers, mirroring the three built-in schemas; a `Custom` input whose
    /// `connector` key means something else is a false positive, and the
    /// convention is the only contract available.
    ///
    /// Usable without a [`crate::Task`]: `FunctionConfig` deserializes from a
    /// bare `{"name": .., "input": ..}` object, so a caller holding only a task's
    /// `function` value does not need to satisfy `Task`'s required `id` and
    /// `name`.
    ///
    /// The match is exhaustive on purpose — a future connector-bearing config
    /// cannot be silently omitted.
    pub fn connector(&self) -> Option<&str> {
        match self {
            FunctionConfig::HttpCall { input, .. } => Some(&input.connector),
            FunctionConfig::Enrich { input, .. } => Some(&input.connector),
            FunctionConfig::PublishKafka { input, .. } => Some(&input.connector),
            FunctionConfig::Custom { input, .. } => input.get("connector").and_then(Value::as_str),
            FunctionConfig::Map { .. }
            | FunctionConfig::Validation { .. }
            | FunctionConfig::ParseJson { .. }
            | FunctionConfig::ParseXml { .. }
            | FunctionConfig::PublishJson { .. }
            | FunctionConfig::PublishXml { .. }
            | FunctionConfig::Filter { .. }
            | FunctionConfig::Log { .. } => None,
        }
    }

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
    /// `mapping_snapshots` is only consulted by the `Map` variant — when
    /// `Some`, the map function pushes a `serde_json::Value` snapshot of the
    /// context before each mapping (for the trace surface). All other
    /// variants ignore it. Pass `None` from the production path.
    ///
    /// This is the single source of truth for the sync-stretch dispatch:
    /// adding a new sync built-in only requires adding an arm here (and the
    /// matching variant to `is_sync_builtin` above).
    pub(crate) fn try_execute_in_arena<'arena>(
        &'arena self,
        message: &mut Message,
        arena_ctx: &mut ArenaContext<'arena>,
        engine: &Arc<Engine>,
        mapping_snapshots: Option<&mut Vec<Value>>,
    ) -> Option<Result<(TaskOutcome, Vec<Change>)>> {
        match self {
            FunctionConfig::Map { input, .. } => {
                Some(input.execute_in_arena(message, arena_ctx, engine, mapping_snapshots))
            }
            FunctionConfig::Validation { input, .. } => {
                Some(input.execute_in_arena(message, arena_ctx, engine))
            }
            FunctionConfig::ParseJson { input, .. } => {
                Some(execute_parse_json_in_arena(message, input, arena_ctx))
            }
            FunctionConfig::ParseXml { input, .. } => {
                // parse_xml/publish_json/publish_xml all write through
                // `set_nested_value` on the owned context rather than the
                // arena, so the arena's "data" slot needs a manual refresh —
                // but only on success; on error the context didn't change
                // either, so the arena cache is still in sync.
                let result = execute_parse_xml(message, input);
                Some(refresh_data_on_success(message, arena_ctx, result))
            }
            FunctionConfig::PublishJson { input, .. } => {
                let result = execute_publish_json(message, input);
                Some(refresh_data_on_success(message, arena_ctx, result))
            }
            FunctionConfig::PublishXml { input, .. } => {
                let result = execute_publish_xml(message, input);
                Some(refresh_data_on_success(message, arena_ctx, result))
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

            // The const and the classifier cannot drift: anything listed as a
            // built-in must classify as one.
            assert!(
                builtin_function_kind(name).is_some(),
                "name '{name}' is in BUILTIN_FUNCTION_NAMES but classifies as None"
            );
        }
    }

    /// Parse an `http_call` task and hand back its typed config.
    fn parse_http_call(
        input: serde_json::Value,
    ) -> std::result::Result<HttpCallConfig, serde_json::Error> {
        match parse(json!({ "name": "http_call", "input": input }))? {
            FunctionConfig::HttpCall { input, .. } => Ok(input),
            other => panic!("expected HttpCall, got {other:?}"),
        }
    }

    #[test]
    fn http_call_response_path_is_read_under_its_own_name() {
        let cfg = parse_http_call(json!({ "connector": "c", "response_path": "data.x" }))
            .expect("response_path should parse");
        assert_eq!(cfg.response_path.as_deref(), Some("data.x"));
    }

    #[test]
    fn http_call_response_path_accepts_the_output_alias() {
        // This is the case that previously yielded None, silently: the request
        // was made and the response thrown away.
        let cfg = parse_http_call(json!({ "connector": "c", "output": "data.x" }))
            .expect("output should be accepted as an alias");
        assert_eq!(cfg.response_path.as_deref(), Some("data.x"));
    }

    #[test]
    fn http_call_response_path_is_optional() {
        let cfg = parse_http_call(json!({ "connector": "c" })).expect("no destination is valid");
        assert_eq!(cfg.response_path, None);
    }

    #[test]
    fn http_call_rejects_both_destination_keys_in_either_order() {
        // Asserting both orderings matters: a single-order test would also pass
        // on an implementation that had an order-dependent precedence rule.
        for input in [
            json!({ "connector": "c", "response_path": "a", "output": "b" }),
            json!({ "connector": "c", "output": "b", "response_path": "a" }),
        ] {
            let err = parse_http_call(input.clone())
                .expect_err("supplying both destination keys must fail");
            let msg = err.to_string();
            assert!(
                msg.starts_with("config for function 'http_call':"),
                "error should carry the function envelope, got: {msg}"
            );
            assert!(
                msg.contains("duplicate field"),
                "error should name the conflict, got: {msg}"
            );
        }
    }

    #[test]
    fn http_call_rejects_a_misspelled_destination_field() {
        // The recorded decision: `HttpCallConfig` is `deny_unknown_fields`, so a
        // near-miss spelling is a parse error naming the field rather than a
        // silently discarded response. This is the defect this closes.
        for bad in ["outputs", "Output", "respose_path", "response-path"] {
            let mut input = serde_json::Map::new();
            input.insert("connector".to_string(), json!("c"));
            input.insert(bad.to_string(), json!("data.x"));

            let err = parse_http_call(serde_json::Value::Object(input))
                .expect_err("a misspelled field must be rejected, not silently discarded");
            let msg = err.to_string();
            assert!(
                msg.starts_with("config for function 'http_call':"),
                "error should carry the function envelope, got: {msg}"
            );
            assert!(
                msg.contains("unknown field"),
                "error should say the field is unknown, got: {msg}"
            );
            assert!(
                msg.contains(bad),
                "error should name the offending field '{bad}', got: {msg}"
            );
        }
    }

    #[test]
    fn enrich_does_not_accept_the_output_alias() {
        // The asymmetry is deliberate: only `HttpCallConfig::response_path`
        // takes the alias. `EnrichConfig`'s destination is `merge_path`, and
        // `deny_unknown_fields` makes the mistake loud instead of silent.
        let err = parse(json!({
            "name": "enrich",
            "input": { "connector": "c", "output": "data.x" }
        }))
        .expect_err("enrich has no `output` field");
        let msg = err.to_string();
        assert!(
            msg.starts_with("config for function 'enrich':"),
            "error should carry the function envelope, got: {msg}"
        );

        // And the real spelling still works.
        let ok = parse(json!({
            "name": "enrich",
            "input": { "connector": "c", "merge_path": "data.x" }
        }))
        .expect("merge_path is enrich's destination field");
        assert!(matches!(ok, FunctionConfig::Enrich { .. }));
    }

    #[test]
    fn publish_kafka_rejects_unknown_fields() {
        let err = parse(json!({
            "name": "publish_kafka",
            "input": { "connector": "c", "topic": "t", "tpoic": "typo" }
        }))
        .expect_err("publish_kafka should reject an unknown field");
        assert!(err.to_string().contains("unknown field"), "got: {err}");
    }

    #[test]
    fn connector_is_returned_for_the_three_typed_integrations() {
        let cases = [
            (
                json!({ "name": "http_call", "input": { "connector": "user_service" } }),
                "user_service",
            ),
            (
                json!({ "name": "enrich",
                        "input": { "connector": "ref_data", "merge_path": "data.out" } }),
                "ref_data",
            ),
            (
                json!({ "name": "publish_kafka",
                        "input": { "connector": "events", "topic": "t" } }),
                "events",
            ),
        ];
        for (input, expected) in cases {
            let cfg = parse(input.clone()).expect("should parse");
            assert_eq!(cfg.connector(), Some(expected), "for {input}");
        }
    }

    #[test]
    fn connector_is_none_for_every_non_connector_builtin() {
        // Table-driven over BUILTIN_FUNCTION_NAMES minus the three integration
        // names, so this cannot go stale when a built-in is added.
        let minimal_input = |name: &str| -> serde_json::Value {
            match name {
                "map" => json!({ "mappings": [] }),
                "validation" | "validate" => json!({ "rules": [] }),
                "parse_json" | "parse_xml" | "publish_json" | "publish_xml" => {
                    json!({ "source": "data.in", "target": "out" })
                }
                "filter" => json!({ "condition": true }),
                "log" => json!({ "message": "hi" }),
                _ => json!({}),
            }
        };

        for name in BUILTIN_FUNCTION_NAMES {
            if matches!(*name, "http_call" | "enrich" | "publish_kafka") {
                continue;
            }
            let cfg = parse(json!({ "name": name, "input": minimal_input(name) }))
                .unwrap_or_else(|e| panic!("'{name}' should parse: {e}"));
            assert_eq!(cfg.connector(), None, "'{name}' names no connector");
        }
    }

    #[test]
    fn connector_reads_the_custom_convention() {
        let cfg = parse(json!({
            "name": "pg_query",
            "input": { "connector": "pg_main", "database": "orders" }
        }))
        .unwrap();
        assert_eq!(cfg.connector(), Some("pg_main"));
    }

    #[test]
    fn connector_is_none_for_a_custom_input_without_a_string_connector() {
        // `Custom` accepts arbitrary input, so every one of these is reachable.
        for input in [
            json!({}),                            // key absent
            json!({ "connector": 7 }),            // number
            json!({ "connector": true }),         // bool
            json!({ "connector": null }),         // null
            json!({ "connector": ["a"] }),        // array
            json!({ "connector": { "n": "a" } }), // object
            json!([]),                            // input is not an object
            json!(7),                             // input is a scalar
        ] {
            let cfg = parse(json!({ "name": "my_handler", "input": input.clone() }))
                .unwrap_or_else(|e| panic!("custom should parse {input}: {e}"));
            assert_eq!(cfg.connector(), None, "for input {input}");
        }
    }

    #[test]
    fn connector_returns_an_empty_name_verbatim() {
        // The recorded decision: the accessor reports what was authored and
        // never disagrees with itself across the typed and Custom arms. Whether
        // an empty connector is acceptable is the host's validation question.
        let typed = parse(json!({ "name": "http_call", "input": { "connector": "" } })).unwrap();
        assert_eq!(typed.connector(), Some(""));

        let custom = parse(json!({ "name": "x", "input": { "connector": "" } })).unwrap();
        assert_eq!(custom.connector(), Some(""));
    }

    #[test]
    fn connector_returns_a_non_ascii_name_byte_for_byte() {
        // A pin against a future "normalize or trim it here" change.
        let cfg =
            parse(json!({ "name": "http_call", "input": { "connector": "連携先" } })).unwrap();
        assert_eq!(cfg.connector(), Some("連携先"));
    }

    #[test]
    fn builtin_function_kind_is_none_for_non_builtins() {
        // Classification is exact-match, same as the deserializer dispatch.
        for name in [
            "",
            "__not_a_builtin__",
            "HTTP_CALL",    // case differs
            "htttp_call",   // typo
            "map ",         // trailing space
            "publish_kafk", // truncated
        ] {
            assert_eq!(
                builtin_function_kind(name),
                None,
                "'{name}' must not classify as a built-in"
            );
            assert!(!is_builtin_function(name));
        }
    }

    #[test]
    fn builtin_kinds_partition_matches_real_dispatch_behaviour() {
        // Tie the classifier to executed code rather than to a second
        // hand-maintained list: `is_sync_builtin` decides whether the workflow
        // executor runs a task itself in the arena, and `try_execute_in_arena`
        // returns `None` for exactly the handler-backed variants. So a
        // SelfContained name must be a sync built-in and a RequiresHandler name
        // must not be.
        let minimal_input = |name: &str| -> serde_json::Value {
            match name {
                "map" => json!({ "mappings": [] }),
                "validation" | "validate" => json!({ "rules": [] }),
                "parse_json" | "parse_xml" | "publish_json" | "publish_xml" => {
                    json!({ "source": "data.in", "target": "out" })
                }
                "filter" => json!({ "condition": true }),
                "log" => json!({ "message": "hi" }),
                "http_call" => json!({ "connector": "c" }),
                "enrich" => json!({ "connector": "c", "merge_path": "data.out" }),
                "publish_kafka" => json!({ "connector": "c", "topic": "t" }),
                // A new built-in with required fields will fail loudly below
                // rather than silently skewing the partition.
                _ => json!({}),
            }
        };

        for name in BUILTIN_FUNCTION_NAMES {
            let kind = builtin_function_kind(name)
                .unwrap_or_else(|| panic!("'{name}' must classify as a built-in"));
            let cfg = parse(json!({ "name": name, "input": minimal_input(name) }))
                .unwrap_or_else(|e| panic!("'{name}' should parse with minimal input: {e}"));

            assert_eq!(
                cfg.is_sync_builtin(),
                matches!(kind, BuiltinKind::SelfContained),
                "'{name}' classifies as {kind:?} but is_sync_builtin() is {}",
                cfg.is_sync_builtin()
            );
        }
    }

    #[test]
    fn requires_handler_kind_covers_exactly_the_config_only_integrations() {
        // The three names documented as shipping config-only.
        for name in ["http_call", "enrich", "publish_kafka"] {
            assert_eq!(
                builtin_function_kind(name),
                Some(BuiltinKind::RequiresHandler),
                "'{name}' ships as config only and needs a registered handler"
            );
        }

        // Both accepted spellings of validation are self-contained. The
        // deserializer takes either; `function_name()` only ever returns
        // "validate", so checking one spelling would miss a regression.
        for name in [
            "map",
            "validation",
            "validate",
            "parse_json",
            "parse_xml",
            "publish_json",
            "publish_xml",
            "filter",
            "log",
        ] {
            assert_eq!(
                builtin_function_kind(name),
                Some(BuiltinKind::SelfContained),
                "'{name}' is executed by this crate"
            );
        }
    }
}

#[cfg(test)]
mod dispatch_vocabulary_tests {
    use super::*;
    use std::collections::HashMap;

    /// A registry whose values are irrelevant — every function here keys off
    /// name membership only.
    fn registry(names: &[&str]) -> HashMap<String, ()> {
        names.iter().map(|n| ((*n).to_string(), ())).collect()
    }

    fn names(registry: &HashMap<String, ()>) -> Vec<&str> {
        let mut out: Vec<&str> = dispatchable_functions_in(registry)
            .map(|f| f.name)
            .collect();
        out.sort_unstable();
        out
    }

    /// Acceptance criterion: the enumeration accounts for every name in
    /// `BUILTIN_FUNCTION_NAMES` exactly once, aliases grouped.
    ///
    /// This is the drift net for `canonical_builtin_name` / `builtin_aliases`.
    /// Adding a built-in without teaching them about it fails here rather than
    /// silently dropping the name from every host's vocabulary.
    #[test]
    fn aliases_and_canonical_names_agree() {
        // Every canonical name is its own canonical form, and every alias
        // resolves to a name that is.
        for name in BUILTIN_FUNCTION_NAMES {
            let canonical = canonical_builtin_name(name);
            assert_eq!(
                canonical_builtin_name(canonical),
                canonical,
                "'{name}' resolves to '{canonical}', which must itself be canonical"
            );
            assert!(
                BUILTIN_FUNCTION_NAMES.contains(&canonical),
                "'{canonical}' is a canonical name and must be an accepted spelling"
            );
            // An alias shares its canonical name's kind — both spellings
            // deserialize to the same variant.
            assert_eq!(
                builtin_function_kind(name),
                builtin_function_kind(canonical),
                "'{name}' and '{canonical}' are one function and must classify alike"
            );
        }

        // Each name is either a canonical entry or an alias of exactly one —
        // never both, never neither.
        for name in BUILTIN_FUNCTION_NAMES {
            let is_canonical = canonical_builtin_name(name) == *name;
            let alias_of: Vec<&str> = BUILTIN_FUNCTION_NAMES
                .iter()
                .copied()
                .filter(|c| builtin_aliases(c).contains(name))
                .collect();
            assert_eq!(
                is_canonical,
                alias_of.is_empty(),
                "'{name}' must be canonical XOR an alias, got canonical={is_canonical} \
                 listed-as-alias-of={alias_of:?}"
            );
            assert!(
                alias_of.len() <= 1,
                "'{name}' is listed as an alias of more than one function: {alias_of:?}"
            );
        }

        // And the two directions agree: every alias listed is a real spelling.
        for canonical in BUILTIN_FUNCTION_NAMES {
            for alias in builtin_aliases(canonical) {
                assert_eq!(
                    canonical_builtin_name(alias),
                    *canonical,
                    "'{alias}' is listed under '{canonical}' but does not resolve to it"
                );
            }
        }
    }

    #[test]
    fn validate_is_canonical_and_validation_is_its_alias() {
        assert_eq!(canonical_builtin_name("validation"), "validate");
        assert_eq!(canonical_builtin_name("validate"), "validate");
        assert_eq!(builtin_aliases("validate"), &["validation"]);
        assert!(builtin_aliases("validation").is_empty());
        assert!(builtin_aliases("map").is_empty());
    }

    #[test]
    fn an_empty_registry_dispatches_every_self_contained_builtin() {
        assert_eq!(
            names(&registry(&[])),
            vec![
                "filter",
                "log",
                "map",
                "parse_json",
                "parse_xml",
                "publish_json",
                "publish_xml",
                "validate",
            ],
            "self-contained built-ins need no registration; `validation` is \
             folded into `validate`, and the three config-only integrations are absent"
        );
    }

    #[test]
    fn requires_handler_builtins_appear_only_when_registered() {
        let empty = registry(&[]);
        assert!(!names(&empty).contains(&"enrich"));
        assert!(!can_dispatch_in(&empty, "enrich"));

        let backed = registry(&["enrich"]);
        assert!(names(&backed).contains(&"enrich"));
        assert!(can_dispatch_in(&backed, "enrich"));

        // …and it keeps its built-in classification rather than reading as custom.
        let entry = dispatchable_functions_in(&backed)
            .find(|f| f.name == "enrich")
            .expect("registered enrich is enumerated");
        assert_eq!(entry.kind, Some(BuiltinKind::RequiresHandler));
    }

    #[test]
    fn custom_names_are_enumerated_with_no_kind() {
        let reg = registry(&["shout"]);
        let entry = dispatchable_functions_in(&reg)
            .find(|f| f.name == "shout")
            .expect("a registered custom name is enumerated");
        assert_eq!(entry.kind, None, "None is how a custom handler reports");
        assert!(entry.aliases.is_empty());
        assert!(can_dispatch_in(&reg, "shout"));
        assert!(!can_dispatch_in(&registry(&[]), "shout"));
    }

    #[test]
    fn registering_a_self_contained_name_is_inert_and_never_duplicates_it() {
        // Registering under `map` does nothing: the deserializer routes `map`
        // to FunctionConfig::Map, which the crate executes itself. The name is
        // reported once either way.
        let shadowed = registry(&["map"]);
        assert_eq!(
            names(&shadowed),
            names(&registry(&[])),
            "a shadowing registration changes nothing about the vocabulary"
        );
        assert_eq!(
            dispatchable_functions_in(&shadowed)
                .filter(|f| f.name == "map")
                .count(),
            1,
            "`map` is yielded exactly once, not once per source"
        );
    }

    #[test]
    fn aliases_dispatch_but_are_not_enumerated() {
        let reg = registry(&[]);
        assert!(
            can_dispatch_in(&reg, "validation"),
            "a task named `validation` really does execute"
        );
        assert!(
            !names(&reg).contains(&"validation"),
            "but the enumeration reports it under `validate`"
        );
    }

    #[test]
    fn can_dispatch_rejects_names_the_crate_does_not_know() {
        let reg = registry(&["shout"]);
        assert!(!can_dispatch_in(&reg, "SHOUT"), "matching is exact");
        assert!(!can_dispatch_in(&reg, "htttp_call"));
        assert!(!can_dispatch_in(&reg, ""));
    }

    /// The predicate and the enumeration must describe the same set — with the
    /// one documented exception that `can_dispatch` also accepts aliases.
    #[test]
    fn every_enumerated_name_is_dispatchable() {
        let reg = registry(&["enrich", "shout"]);
        for f in dispatchable_functions_in(&reg) {
            assert!(
                can_dispatch_in(&reg, f.name),
                "'{}' is enumerated, so it must dispatch",
                f.name
            );
            for alias in f.aliases {
                assert!(
                    can_dispatch_in(&reg, alias),
                    "alias '{alias}' of '{}' must dispatch too",
                    f.name
                );
            }
        }
    }
}
