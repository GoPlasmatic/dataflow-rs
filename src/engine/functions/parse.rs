//! # Parse Function Module
//!
//! Parsing helpers that load payload data into the message's `data` context.
//! Supports JSON (native) and XML (via `serde_json::Value` bridge — XML is the
//! slow path, not worth a dedicated walker).
//!
//! Source paths:
//! - `"payload"` — entire payload
//! - `"payload.<path>"` — a nested field of the payload
//! - `"data.<path>"` — a nested field of the existing data context
//! - `"<path>"` — anything else is resolved against the full context

use crate::engine::error::{DataflowError, Result};
use crate::engine::executor::{ArenaContext, with_arena};
use crate::engine::functions::path_template::{DataRoot, ParamCtx, PathTemplate, ResolvedPath};
use crate::engine::functions::template::Template;
use crate::engine::message::{Change, Message};
use crate::engine::task_outcome::TaskOutcome;
use crate::engine::utils::{get_nested_value, get_nested_value_parts, set_nested_value_parts};
use datalogic_rs::Engine;
use datavalue::OwnedDataValue;
use log::debug;
use serde::Deserialize;
use serde_json::Value;
use std::borrow::Cow;
use std::sync::Arc;

/// Configuration for parse functions.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ParseConfig {
    /// Source location to read from — `"payload"`, `"payload.<path>"`,
    /// `"data.<path>"`, or a bare context path.
    ///
    /// JSONLogic, so the location can be computed:
    /// `{"cat": ["data.batches.", {"var": "temp_data.i"}]}`. It resolves to the
    /// *name* of a location, never to the value at one — `payload` is not in
    /// the JSONLogic evaluation context, so an expression could not read it
    /// even if it tried. Read it through [`Self::resolve_source`].
    pub source: Template,

    /// Target field name in `data` (stored at `data.{target}`).
    ///
    /// JSONLogic. A literal folds at compile time and keeps the precomputed
    /// split this always had.
    pub target: PathTemplate<DataRoot>,
}

impl ParseConfig {
    pub fn from_json(input: &Value) -> Result<Self> {
        let source = input.get("source").cloned().ok_or_else(|| {
            DataflowError::Validation("Missing 'source' in parse config".to_string())
        })?;

        let target = input.get("target").cloned().ok_or_else(|| {
            DataflowError::Validation("Missing 'target' in parse config".to_string())
        })?;

        Ok(ParseConfig {
            source: Template::from(source),
            target: PathTemplate::from(target),
        })
    }

    /// The source location for this message.
    ///
    /// # Errors
    ///
    /// [`DataflowError::LogicEvaluation`] if the expression fails to evaluate.
    pub(crate) fn resolve_source(&self, p: ParamCtx<'_>) -> Result<Cow<'_, str>> {
        self.source.resolve_str_in_arena(p)
    }

    /// The write destination for this message, as `(dotted, parts)`.
    ///
    /// # Errors
    ///
    /// As [`Self::resolve_source`].
    pub(crate) fn resolve_target_path(&self, p: ParamCtx<'_>) -> Result<Cow<'_, ResolvedPath>> {
        self.target.resolve_in_arena(p)
    }

    /// Extract the value at `source` as an owned `OwnedDataValue`.
    ///
    /// `payload` is reachable here and nowhere else: it is a separate field on
    /// `Message`, not part of the JSONLogic evaluation context, so this
    /// dispatch on the *resolved name* is the only way to read it.
    fn extract_source(&self, message: &Message, source: &str) -> OwnedDataValue {
        if source == "payload" {
            (*message.payload).clone()
        } else if let Some(path) = source.strip_prefix("payload.") {
            get_nested_value(&message.payload, path)
                .cloned()
                .unwrap_or(OwnedDataValue::Null)
        } else if let Some(path) = source.strip_prefix("data.") {
            get_nested_value(message.data(), path)
                .cloned()
                .unwrap_or(OwnedDataValue::Null)
        } else {
            get_nested_value(&message.context, source)
                .cloned()
                .unwrap_or(OwnedDataValue::Null)
        }
    }
}

/// Execute `parse_json`: read the source value and store it under `data.{target}`.
/// If the source is a JSON string, attempt to parse it; on failure, store the
/// string as-is (matches prior behaviour).
pub fn execute_parse_json(
    message: &mut Message,
    config: &ParseConfig,
    engine: &Engine,
) -> Result<(TaskOutcome, Vec<Change>)> {
    // Opens its own arena, as `MapConfig::execute` does — this entry point is
    // for callers outside a workflow sync stretch. Inside one,
    // `execute_parse_json_in_arena` reuses the stretch's context instead.
    with_arena(|arena| {
        let arena_ctx = ArenaContext::from_owned(&message.context, arena);
        parse_json_in(message, config, ParamCtx::from_arena(engine, &arena_ctx))
    })
}

fn parse_json_in(
    message: &mut Message,
    config: &ParseConfig,
    p: ParamCtx<'_>,
) -> Result<(TaskOutcome, Vec<Change>)> {
    let source = config.resolve_source(p)?;
    let target = config.resolve_target_path(p)?;
    let (target_path_arc, target_parts) = (&target.0, &*target.1);
    debug!("ParseJson: Extracting from '{source}' to '{target_path_arc}'");

    // Hot path: source == "payload" and not a JSON-string payload. The
    // payload Arc is already on the message; clone-into-context once, reuse
    // the Arc for the audit entry (refcount bump). This is the realistic
    // benchmark's exact shape.
    let payload_fast_path =
        source == "payload" && !matches!(*message.payload, OwnedDataValue::String(_));

    if message.capture_changes {
        let old_value = get_nested_value_parts(&message.context, target_parts)
            .cloned()
            .unwrap_or(OwnedDataValue::Null);

        // Resolve the source value once. For the payload fast-path we clone
        // out of the shared `Arc<OwnedDataValue>` payload; for the slow path
        // we extract from a sub-tree and re-parse JSON-string payloads.
        let source_data = resolve_parsed_source(config, message, &source, payload_fast_path);

        // Clone the source value once for the audit `new_value`; the original
        // is moved into the context below. (No `Arc` wrapping in the audit
        // entry — `Change` owns its values directly.)
        let new_value = source_data.clone();

        set_nested_value_parts(&mut message.context, target_parts, source_data);
        debug!("ParseJson: Successfully stored data to '{target_path_arc}'");
        return Ok((
            TaskOutcome::Success,
            vec![Change {
                path: Arc::clone(target_path_arc),
                old_value,
                new_value,
            }],
        ));
    }

    // Audit-off fast path: only the deep clone into the context survives.
    let source_data_for_context =
        resolve_parsed_source(config, message, &source, payload_fast_path);
    set_nested_value_parts(&mut message.context, target_parts, source_data_for_context);

    debug!("ParseJson: Successfully stored data to '{target_path_arc}'");

    Ok((TaskOutcome::Success, Vec::new()))
}

/// Resolve the value `parse_json` stores into `data.{target}`. The payload
/// fast-path clones straight out of the shared `Arc<OwnedDataValue>` payload;
/// otherwise the source is extracted from a sub-tree, re-parsing a
/// JSON-string source and falling back to the raw value on parse failure.
fn resolve_parsed_source(
    config: &ParseConfig,
    message: &Message,
    source: &str,
    payload_fast_path: bool,
) -> OwnedDataValue {
    if payload_fast_path {
        (*message.payload).clone()
    } else {
        let raw = config.extract_source(message, source);
        match &raw {
            OwnedDataValue::String(s) => {
                OwnedDataValue::from_json(s).unwrap_or_else(|_| raw.clone())
            }
            _ => raw,
        }
    }
}

/// Same as `execute_parse_json` but also refreshes the supplied
/// `ArenaContext` so subsequent sync tasks in the same workflow stretch see
/// the written `data.<target>` slot without rebuilding the whole arena form.
pub(crate) fn execute_parse_json_in_arena<'a>(
    message: &mut Message,
    config: &ParseConfig,
    engine: &datalogic_rs::Engine,
    arena_ctx: &mut ArenaContext<'a>,
) -> Result<(TaskOutcome, Vec<Change>)> {
    let p = ParamCtx::from_arena(engine, arena_ctx);
    let result = parse_json_in(message, config, p)?;
    // Refresh ONLY the affected depth-2 slot in the arena cache. For
    // source == "payload" target = "input", this is `data.input` — the
    // heavy slot — but it's re-arena'd exactly once per workflow stretch
    // here, not once per subsequent map mapping.
    let target = config.resolve_target_path(p)?;
    let target_parts = &*target.1;
    arena_ctx.refresh_for_path_parts(&message.context, target_parts);
    Ok(result)
}

/// Execute `parse_xml`: read the source string, parse XML into a
/// `serde_json::Value` (existing quick-xml path), convert to `OwnedDataValue`,
/// store under `data.{target}`.
pub fn execute_parse_xml(
    message: &mut Message,
    config: &ParseConfig,
    engine: &Engine,
) -> Result<(TaskOutcome, Vec<Change>)> {
    with_arena(|arena| {
        let arena_ctx = ArenaContext::from_owned(&message.context, arena);
        parse_xml_in(message, config, ParamCtx::from_arena(engine, &arena_ctx))
    })
}

pub(crate) fn parse_xml_in(
    message: &mut Message,
    config: &ParseConfig,
    p: ParamCtx<'_>,
) -> Result<(TaskOutcome, Vec<Change>)> {
    let source = config.resolve_source(p)?;
    debug!("ParseXml: Extracting from '{source}'");
    let source_data = config.extract_source(message, &source);

    let xml_string = match &source_data {
        OwnedDataValue::String(s) => s.clone(),
        _ => {
            return Err(DataflowError::Validation(format!(
                "ParseXml: Source '{source}' is not a string"
            )));
        }
    };

    let parsed_json = xml_to_json(&xml_string)?;
    let parsed_owned = OwnedDataValue::from(&parsed_json);

    let target = config.resolve_target_path(p)?;
    let (target_path_arc, target_parts) = (&target.0, &*target.1);
    let old_value = get_nested_value_parts(&message.context, target_parts)
        .cloned()
        .unwrap_or(OwnedDataValue::Null);

    set_nested_value_parts(&mut message.context, target_parts, parsed_owned.clone());

    debug!("ParseXml: Successfully parsed and stored XML to '{target_path_arc}'");

    Ok((
        TaskOutcome::Success,
        vec![Change {
            path: Arc::clone(target_path_arc),
            old_value,
            new_value: parsed_owned,
        }],
    ))
}

/// Convert an XML string to `serde_json::Value` using quick-xml's serde path.
fn xml_to_json(xml: &str) -> Result<Value> {
    use quick_xml::de::from_str;

    let parsed: Value = from_str(xml)
        .map_err(|e| DataflowError::Validation(format!("Failed to parse XML: {}", e)))?;

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::utils::set_nested_value;
    use serde_json::json;
    use std::sync::Arc;

    /// The engine parameters are resolved against — the same construction
    /// production uses, so a test cannot agree with an engine that never runs.
    fn test_engine() -> Engine {
        crate::engine::compiler::datalogic_engine_builder().build()
    }

    fn dv(v: serde_json::Value) -> OwnedDataValue {
        OwnedDataValue::from(&v)
    }

    #[test]
    fn test_parse_config_from_json() {
        let input = json!({"source": "payload", "target": "input_data"});
        let config = ParseConfig::from_json(&input).unwrap();
        assert_eq!(config.source.as_json(), &json!("payload"));
        assert_eq!(config.target.as_json(), &json!("input_data"));
    }

    #[test]
    fn test_parse_config_missing_source() {
        assert!(ParseConfig::from_json(&json!({"target": "input_data"})).is_err());
    }

    #[test]
    fn test_parse_config_missing_target() {
        assert!(ParseConfig::from_json(&json!({"source": "payload"})).is_err());
    }

    #[test]
    fn test_execute_parse_json_from_payload() {
        let payload = json!({"name": "John", "age": 30});
        let mut message = Message::from_value(&payload);

        let config = ParseConfig {
            source: Template::from(json!("payload")),
            target: PathTemplate::from("input"),
        };

        let result = execute_parse_json(&mut message, &config, &test_engine());
        assert!(result.is_ok());

        let (outcome, changes) = result.unwrap();
        assert_eq!(outcome, TaskOutcome::Success);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path.as_ref(), "data.input");

        assert_eq!(message.data()["input"]["name"], dv(json!("John")));
        assert_eq!(message.data()["input"]["age"], dv(json!(30)));
    }

    #[test]
    fn test_execute_parse_json_from_nested_payload() {
        let payload = json!({"body": {"user": {"name": "Alice"}}});
        let mut message = Message::from_value(&payload);

        let config = ParseConfig {
            source: Template::from(json!("payload.body.user")),
            target: PathTemplate::from("user_data"),
        };

        let result = execute_parse_json(&mut message, &config, &test_engine());
        assert!(result.is_ok());

        let (outcome, _) = result.unwrap();
        assert_eq!(outcome, TaskOutcome::Success);
        assert_eq!(message.data()["user_data"]["name"], dv(json!("Alice")));
    }

    #[test]
    fn test_execute_parse_json_from_data() {
        let mut message = Message::new(Arc::new(dv(json!({}))));
        set_nested_value(
            &mut message.context,
            "data",
            dv(json!({"existing": {"value": 42}})),
        );

        let config = ParseConfig {
            source: Template::from(json!("data.existing")),
            target: PathTemplate::from("copied"),
        };

        let result = execute_parse_json(&mut message, &config, &test_engine());
        assert!(result.is_ok());

        assert_eq!(message.data()["copied"]["value"], dv(json!(42)));
    }

    #[test]
    fn test_execute_parse_xml_simple() {
        let xml_payload = json!("<root><name>John</name><age>30</age></root>");
        let mut message = Message::from_value(&xml_payload);

        let config = ParseConfig {
            source: Template::from(json!("payload")),
            target: PathTemplate::from("parsed"),
        };

        let result = execute_parse_xml(&mut message, &config, &test_engine());
        assert!(result.is_ok());

        let (outcome, _) = result.unwrap();
        assert_eq!(outcome, TaskOutcome::Success);

        let parsed = &message.data()["parsed"];
        assert!(parsed.is_object());
    }

    #[test]
    fn test_execute_parse_xml_not_string() {
        let payload = json!({"not": "a string"});
        let mut message = Message::from_value(&payload);

        let config = ParseConfig {
            source: Template::from(json!("payload")),
            target: PathTemplate::from("parsed"),
        };

        assert!(execute_parse_xml(&mut message, &config, &test_engine()).is_err());
    }

    #[test]
    fn test_xml_to_json_simple() {
        let xml = "<root><name>Test</name></root>";
        let result = xml_to_json(xml);
        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.is_object());
    }

    #[test]
    fn test_xml_to_json_invalid() {
        let xml = "<root><unclosed>";
        assert!(xml_to_json(xml).is_err());
    }

    #[test]
    fn test_xml_to_json_with_attributes() {
        let xml = r#"<person id="123"><name>John</name></person>"#;
        assert!(xml_to_json(xml).is_ok());
    }

    #[test]
    fn test_xml_to_json_nested() {
        let xml = r#"<root><user><name>Alice</name><email>alice@example.com</email></user></root>"#;
        let result = xml_to_json(xml);
        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.is_object());
    }

    #[test]
    fn test_execute_parse_json_from_string_payload() {
        let payload = Value::String(r#"{"name":"John","age":30}"#.to_string());
        let mut message = Message::from_value(&payload);

        let config = ParseConfig {
            source: Template::from(json!("payload")),
            target: PathTemplate::from("input"),
        };

        let result = execute_parse_json(&mut message, &config, &test_engine());
        assert!(result.is_ok());

        let (outcome, _) = result.unwrap();
        assert_eq!(outcome, TaskOutcome::Success);

        assert_eq!(message.data()["input"]["name"], dv(json!("John")));
        assert_eq!(message.data()["input"]["age"], dv(json!(30)));
    }
}
