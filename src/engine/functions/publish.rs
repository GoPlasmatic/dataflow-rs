//! # Publish Function Module
//!
//! Serialises a slice of the message's `data` context to a JSON or XML string
//! and stores it back under `data.{target}`. JSON uses `OwnedDataValue`'s
//! native `to_json_string`; pretty-printed JSON and XML both bridge through
//! `serde_json::Value` since neither is on the hot path.

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

/// Configuration for publish functions.
#[derive(Debug, Clone, Deserialize)]
pub struct PublishConfig {
    /// Source field path inside `data` to serialize, as JSONLogic. Resolves to
    /// the *name* of a location, not the value at one.
    pub source: Template,

    /// Target field name inside `data` to receive the serialised string, as
    /// JSONLogic. A literal folds at compile time and keeps the precomputed
    /// split this always had.
    pub target: PathTemplate<DataRoot>,

    /// Whether to pretty-print the output (JSON only).
    #[serde(default)]
    pub pretty: bool,

    /// Root element name for XML output, as JSONLogic — so the element can
    /// follow the message type: `{"var": "data.doc_type"}`.
    #[serde(default = "default_root_element")]
    pub root_element: Template,
}

// Manual impl so `..Default::default()` construction gets the same
// `root_element` the serde default supplies ("root", not "").
impl Default for PublishConfig {
    fn default() -> Self {
        Self {
            source: Template::from(Value::String(String::new())),
            target: PathTemplate::default(),
            pretty: false,
            root_element: default_root_element(),
        }
    }
}

fn default_root_element() -> Template {
    Template::from(Value::from(DEFAULT_ROOT_ELEMENT))
}

/// The `root_element` default, as a plain name.
pub const DEFAULT_ROOT_ELEMENT: &str = "root";

impl PublishConfig {
    pub fn from_json(input: &Value) -> Result<Self> {
        let source = input.get("source").cloned().ok_or_else(|| {
            DataflowError::Validation("Missing 'source' in publish config".to_string())
        })?;

        let target = input.get("target").cloned().ok_or_else(|| {
            DataflowError::Validation("Missing 'target' in publish config".to_string())
        })?;

        let pretty = input
            .get("pretty")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let root_element = input
            .get("root_element")
            .cloned()
            .map(Template::from)
            .unwrap_or_else(default_root_element);

        Ok(PublishConfig {
            source: Template::from(source),
            target: PathTemplate::from(target),
            pretty,
            root_element,
        })
    }

    /// The write destination for this message, as `(dotted, parts)`.
    ///
    /// # Errors
    ///
    /// [`DataflowError::LogicEvaluation`] if the expression fails to evaluate.
    pub(crate) fn resolve_target_path(&self, p: ParamCtx<'_>) -> Result<Cow<'_, ResolvedPath>> {
        self.target.resolve_in_arena(p)
    }

    /// Resolve the source value as a borrow into the message context. The
    /// serializers below only read the value, so no deep clone of the source
    /// subtree is needed — the borrow ends before the context mutation.
    /// Returns `None` when the path doesn't resolve.
    fn resolve_source<'m>(&self, message: &'m Message, source: &str) -> Option<&'m OwnedDataValue> {
        // Direct field in `data` (also matches keys containing literal dots,
        // which the nested walk below would split).
        if let Some(value) = message.data().get(source) {
            return Some(value);
        }

        // Nested path inside `data`.
        if let Some(value) = get_nested_value(message.data(), source) {
            return Some(value);
        }

        // `data.<path>` shorthand pointing back into `data`.
        if let Some(path) = source.strip_prefix("data.") {
            return get_nested_value(message.data(), path);
        }

        None
    }
}

/// Shared tail behind [`execute_publish_json`] and [`execute_publish_xml`]:
/// resolve the target path, snapshot the old value, write the already-computed
/// `serialized` string, and build the `Change`. The two callers differ only in
/// how they produce `serialized` (JSON vs. XML) — everything from "where does
/// it land" onward is identical.
fn finish_publish(
    message: &mut Message,
    target: &ResolvedPath,
    serialized: String,
) -> (TaskOutcome, Vec<Change>) {
    let (target_path_arc, target_parts) = (&target.0, &*target.1);
    let old_value = get_nested_value_parts(&message.context, target_parts)
        .cloned()
        .unwrap_or(OwnedDataValue::Null);
    let new_value = OwnedDataValue::String(serialized);

    set_nested_value_parts(&mut message.context, target_parts, new_value.clone());

    (
        TaskOutcome::Success,
        vec![Change {
            path: Arc::clone(target_path_arc),
            old_value,
            new_value,
        }],
    )
}

/// Execute `publish_json`: serialise `data.{source}` to a JSON string and
/// store at `data.{target}`.
pub fn execute_publish_json(
    message: &mut Message,
    config: &PublishConfig,
    engine: &Engine,
) -> Result<(TaskOutcome, Vec<Change>)> {
    with_arena(|arena| {
        let arena_ctx = ArenaContext::from_owned(&message.context, arena);
        publish_json_in(message, config, ParamCtx::from_arena(engine, &arena_ctx))
    })
}

pub(crate) fn publish_json_in(
    message: &mut Message,
    config: &PublishConfig,
    p: ParamCtx<'_>,
) -> Result<(TaskOutcome, Vec<Change>)> {
    let source = config.source.resolve_str_in_arena(p)?;
    let target = config.resolve_target_path(p)?;
    debug!("PublishJson: Serializing 'data.{source}' to '{}'", target.0);

    // Borrowed resolve — a missing path and an explicit Null both reject,
    // matching the historical extract_source contract.
    let source_data = match config.resolve_source(message, &source) {
        Some(v) if !matches!(v, OwnedDataValue::Null) => v,
        _ => {
            return Err(DataflowError::Validation(format!(
                "PublishJson: Source 'data.{source}' not found or is null"
            )));
        }
    };

    // For compact JSON, use OwnedDataValue's native emitter (fastest path).
    // For pretty JSON, bridge to serde_json::Value — pretty publish is not a
    // hot path and the bridge cost there is irrelevant. Either way the
    // serializer reads through the borrow; the source subtree is never
    // deep-cloned.
    let json_string = if config.pretty {
        let bridge = Value::from(source_data);
        serde_json::to_string_pretty(&bridge)
            .map_err(|e| DataflowError::Validation(format!("Failed to serialize to JSON: {}", e)))?
    } else {
        source_data.to_json_string()
    };

    Ok(finish_publish(message, &target, json_string))
}

/// Execute `publish_xml`: serialise `data.{source}` to an XML string and
/// store at `data.{target}`. Bridges to `serde_json::Value` for the existing
/// recursive XML walker — XML is the slow path, no perf concern.
pub fn execute_publish_xml(
    message: &mut Message,
    config: &PublishConfig,
    engine: &Engine,
) -> Result<(TaskOutcome, Vec<Change>)> {
    with_arena(|arena| {
        let arena_ctx = ArenaContext::from_owned(&message.context, arena);
        publish_xml_in(message, config, ParamCtx::from_arena(engine, &arena_ctx))
    })
}

pub(crate) fn publish_xml_in(
    message: &mut Message,
    config: &PublishConfig,
    p: ParamCtx<'_>,
) -> Result<(TaskOutcome, Vec<Change>)> {
    let source = config.source.resolve_str_in_arena(p)?;
    let target = config.resolve_target_path(p)?;
    debug!("PublishXml: Serializing 'data.{source}' to '{}'", target.0);

    // Borrowed resolve — same contract as the JSON path: missing and
    // explicit-Null sources both reject, no source deep clone.
    let source_data = match config.resolve_source(message, &source) {
        Some(v) if !matches!(v, OwnedDataValue::Null) => v,
        _ => {
            return Err(DataflowError::Validation(format!(
                "PublishXml: Source 'data.{source}' not found or is null"
            )));
        }
    };

    let bridge = Value::from(source_data);
    let root_element = config.root_element.resolve_str_in_arena(p)?;
    let xml_string = json_to_xml(&bridge, &root_element)?;

    Ok(finish_publish(message, &target, xml_string))
}

/// Convert JSON Value to XML string. Recursive walker; same shape as before
/// the OwnedDataValue refactor — kept on `serde_json::Value` since XML is the
/// slow path.
fn json_to_xml(value: &Value, root_element: &str) -> Result<String> {
    let mut buffer = String::new();

    match value {
        Value::Object(_) => {
            buffer.push_str(&format!("<{}>", root_element));
            let content = serialize_value_to_xml_content(value)?;
            buffer.push_str(&content);
            buffer.push_str(&format!("</{}>", root_element));
        }
        Value::Array(arr) => {
            buffer.push_str(&format!("<{}>", root_element));
            for item in arr {
                buffer.push_str("<item>");
                let content = serialize_value_to_xml_content(item)?;
                buffer.push_str(&content);
                buffer.push_str("</item>");
            }
            buffer.push_str(&format!("</{}>", root_element));
        }
        _ => {
            buffer.push_str(&format!("<{}>", root_element));
            buffer.push_str(&value_to_xml_string(value));
            buffer.push_str(&format!("</{}>", root_element));
        }
    }

    Ok(buffer)
}

fn serialize_value_to_xml_content(value: &Value) -> Result<String> {
    let mut result = String::new();

    match value {
        Value::Object(map) => {
            for (key, val) in map {
                let safe_key = sanitize_xml_name(key);
                result.push_str(&format!("<{}>", safe_key));
                match val {
                    Value::Object(_) | Value::Array(_) => {
                        result.push_str(&serialize_value_to_xml_content(val)?);
                    }
                    _ => {
                        result.push_str(&value_to_xml_string(val));
                    }
                }
                result.push_str(&format!("</{}>", safe_key));
            }
        }
        Value::Array(arr) => {
            for item in arr {
                result.push_str("<item>");
                match item {
                    Value::Object(_) | Value::Array(_) => {
                        result.push_str(&serialize_value_to_xml_content(item)?);
                    }
                    _ => {
                        result.push_str(&value_to_xml_string(item));
                    }
                }
                result.push_str("</item>");
            }
        }
        _ => {
            result.push_str(&value_to_xml_string(value));
        }
    }

    Ok(result)
}

fn value_to_xml_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => escape_xml(s),
        _ => String::new(),
    }
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn sanitize_xml_name(name: &str) -> String {
    let mut result = String::new();

    for (i, c) in name.chars().enumerate() {
        if i == 0 {
            if c.is_ascii_alphabetic() || c == '_' {
                result.push(c);
            } else {
                result.push('_');
                if c.is_ascii_alphanumeric() {
                    result.push(c);
                }
            }
        } else if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
            result.push(c);
        } else {
            result.push('_');
        }
    }

    if result.is_empty() {
        result = "_element".to_string();
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn message_with_data(initial: serde_json::Value) -> Message {
        Message::builder().data(dv(initial)).build()
    }

    #[test]
    fn test_publish_config_from_json() {
        let input = json!({"source": "output", "target": "json_string"});
        let config = PublishConfig::from_json(&input).unwrap();
        assert_eq!(config.source.as_json(), &json!("output"));
        assert_eq!(config.target.as_json(), &json!("json_string"));
        assert!(!config.pretty);
        assert_eq!(config.root_element.as_json(), &json!("root"));
    }

    #[test]
    fn test_publish_config_with_options() {
        let input = json!({
            "source": "data",
            "target": "xml_output",
            "pretty": true,
            "root_element": "document"
        });

        let config = PublishConfig::from_json(&input).unwrap();
        assert_eq!(config.source.as_json(), &json!("data"));
        assert_eq!(config.target.as_json(), &json!("xml_output"));
        assert!(config.pretty);
        assert_eq!(config.root_element.as_json(), &json!("document"));
    }

    #[test]
    fn test_publish_config_missing_source() {
        assert!(PublishConfig::from_json(&json!({"target": "output"})).is_err());
    }

    #[test]
    fn test_publish_config_missing_target() {
        assert!(PublishConfig::from_json(&json!({"source": "input"})).is_err());
    }

    #[test]
    fn test_execute_publish_json() {
        let mut message = message_with_data(json!({"user": {"name": "John", "age": 30}}));

        let config = PublishConfig {
            source: Template::from(json!("user")),
            target: PathTemplate::from("user_json"),
            pretty: false,
            root_element: Template::from(json!("root")),
        };

        let result = execute_publish_json(&mut message, &config, &test_engine());
        assert!(result.is_ok());

        let (outcome, changes) = result.unwrap();
        assert_eq!(outcome, TaskOutcome::Success);
        assert_eq!(changes.len(), 1);

        let json_string = message.data()["user_json"].as_str().unwrap();
        assert!(json_string.contains("John"));
        assert!(json_string.contains("30"));
    }

    #[test]
    fn test_execute_publish_json_pretty() {
        let mut message = message_with_data(json!({"user": {"name": "Alice"}}));

        let config = PublishConfig {
            source: Template::from(json!("user")),
            target: PathTemplate::from("output"),
            pretty: true,
            root_element: Template::from(json!("root")),
        };

        let result = execute_publish_json(&mut message, &config, &test_engine());
        assert!(result.is_ok());

        let json_string = message.data()["output"].as_str().unwrap();
        assert!(json_string.contains('\n'));
    }

    #[test]
    fn test_execute_publish_json_not_found() {
        let mut message = Message::new(Arc::new(dv(json!({}))));

        let config = PublishConfig {
            source: Template::from(json!("nonexistent")),
            target: PathTemplate::from("output"),
            pretty: false,
            root_element: Template::from(json!("root")),
        };

        assert!(execute_publish_json(&mut message, &config, &test_engine()).is_err());
    }

    #[test]
    fn test_execute_publish_xml() {
        let mut message = message_with_data(json!({"user": {"name": "John", "age": 30}}));

        let config = PublishConfig {
            source: Template::from(json!("user")),
            target: PathTemplate::from("user_xml"),
            pretty: false,
            root_element: Template::from(json!("user")),
        };

        let result = execute_publish_xml(&mut message, &config, &test_engine());
        assert!(result.is_ok());

        let (outcome, _) = result.unwrap();
        assert_eq!(outcome, TaskOutcome::Success);

        let xml_string = message.data()["user_xml"].as_str().unwrap();
        assert!(xml_string.contains("<user>"));
        assert!(xml_string.contains("</user>"));
        assert!(xml_string.contains("<name>John</name>"));
    }

    #[test]
    fn test_execute_publish_xml_not_found() {
        let mut message = Message::new(Arc::new(dv(json!({}))));

        let config = PublishConfig {
            source: Template::from(json!("nonexistent")),
            target: PathTemplate::from("output"),
            pretty: false,
            root_element: Template::from(json!("root")),
        };

        assert!(execute_publish_xml(&mut message, &config, &test_engine()).is_err());
    }

    #[test]
    fn test_json_to_xml_simple() {
        let value = json!({"name": "Test", "value": 42});
        let xml = json_to_xml(&value, "root").unwrap();
        assert!(xml.contains("<root>"));
        assert!(xml.contains("</root>"));
        assert!(xml.contains("<name>Test</name>"));
        assert!(xml.contains("<value>42</value>"));
    }

    #[test]
    fn test_json_to_xml_nested() {
        let value = json!({"user": {"name": "Alice", "email": "alice@example.com"}});
        let xml = json_to_xml(&value, "data").unwrap();
        assert!(xml.contains("<data>"));
        assert!(xml.contains("<user>"));
        assert!(xml.contains("<name>Alice</name>"));
    }

    #[test]
    fn test_json_to_xml_array() {
        let value = json!([1, 2, 3]);
        let xml = json_to_xml(&value, "numbers").unwrap();
        assert!(xml.contains("<numbers>"));
        assert!(xml.contains("<item>1</item>"));
        assert!(xml.contains("<item>2</item>"));
        assert!(xml.contains("<item>3</item>"));
    }

    #[test]
    fn test_json_to_xml_special_chars() {
        let value = json!({"text": "<script>alert('xss')</script>"});
        let xml = json_to_xml(&value, "root").unwrap();
        assert!(xml.contains("&lt;script&gt;"));
        assert!(!xml.contains("<script>"));
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("hello"), "hello");
        assert_eq!(escape_xml("<tag>"), "&lt;tag&gt;");
        assert_eq!(escape_xml("a & b"), "a &amp; b");
        assert_eq!(escape_xml("\"quoted\""), "&quot;quoted&quot;");
    }

    #[test]
    fn test_sanitize_xml_name() {
        assert_eq!(sanitize_xml_name("valid"), "valid");
        assert_eq!(sanitize_xml_name("_valid"), "_valid");
        assert_eq!(sanitize_xml_name("123invalid"), "_123invalid");
        assert_eq!(sanitize_xml_name("has spaces"), "has_spaces");
        assert_eq!(sanitize_xml_name("has-dash"), "has-dash");
        assert_eq!(sanitize_xml_name(""), "_element");
    }

    #[test]
    fn test_execute_publish_json_nested_source() {
        let mut message = message_with_data(json!({
            "response": {"body": {"message": "success"}}
        }));

        let config = PublishConfig {
            source: Template::from(json!("response.body")),
            target: PathTemplate::from("output"),
            pretty: false,
            root_element: Template::from(json!("root")),
        };

        let result = execute_publish_json(&mut message, &config, &test_engine());
        assert!(result.is_ok());

        let json_string = message.data()["output"].as_str().unwrap();
        assert!(json_string.contains("success"));
    }
}
