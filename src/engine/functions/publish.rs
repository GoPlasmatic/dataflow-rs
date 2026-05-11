//! # Publish Function Module
//!
//! Serialises a slice of the message's `data` context to a JSON or XML string
//! and stores it back under `data.{target}`. JSON uses `OwnedDataValue`'s
//! native `to_json_string`; pretty-printed JSON and XML both bridge through
//! `serde_json::Value` since neither is on the hot path.

use crate::engine::error::{DataflowError, Result};
use crate::engine::message::{Change, Message};
use crate::engine::utils::{get_nested_value, set_nested_value};
use datavalue::OwnedDataValue;
use log::debug;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// Configuration for publish functions.
#[derive(Debug, Clone, Deserialize)]
pub struct PublishConfig {
    /// Source field path inside `data` to serialize.
    pub source: String,

    /// Target field name inside `data` to receive the serialised string.
    pub target: String,

    /// Whether to pretty-print the output (JSON only).
    #[serde(default)]
    pub pretty: bool,

    /// Root element name for XML output.
    #[serde(default = "default_root_element")]
    pub root_element: String,
}

fn default_root_element() -> String {
    "root".to_string()
}

impl PublishConfig {
    pub fn from_json(input: &Value) -> Result<Self> {
        let source = input
            .get("source")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                DataflowError::Validation("Missing 'source' in publish config".to_string())
            })?
            .to_string();

        let target = input
            .get("target")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                DataflowError::Validation("Missing 'target' in publish config".to_string())
            })?
            .to_string();

        let pretty = input
            .get("pretty")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let root_element = input
            .get("root_element")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(default_root_element);

        Ok(PublishConfig {
            source,
            target,
            pretty,
            root_element,
        })
    }

    /// Extract the source value as an owned `OwnedDataValue`.
    fn extract_source(&self, message: &Message) -> OwnedDataValue {
        // Direct field in `data`.
        if let Some(value) = message.data().get(&self.source) {
            return value.clone();
        }

        // Nested path inside `data`.
        if let Some(value) = get_nested_value(message.data(), &self.source) {
            return value.clone();
        }

        // `data.<path>` shorthand pointing back into `data`.
        if let Some(path) = self.source.strip_prefix("data.")
            && let Some(value) = get_nested_value(message.data(), path)
        {
            return value.clone();
        }

        OwnedDataValue::Null
    }
}

/// Execute `publish_json`: serialise `data.{source}` to a JSON string and
/// store at `data.{target}`.
pub fn execute_publish_json(
    message: &mut Message,
    config: &PublishConfig,
) -> Result<(usize, Vec<Change>)> {
    debug!(
        "PublishJson: Serializing 'data.{}' to 'data.{}'",
        config.source, config.target
    );

    let source_data = config.extract_source(message);

    if matches!(source_data, OwnedDataValue::Null) {
        return Err(DataflowError::Validation(format!(
            "PublishJson: Source 'data.{}' not found or is null",
            config.source
        )));
    }

    // For compact JSON, use OwnedDataValue's native emitter (fastest path).
    // For pretty JSON, bridge to serde_json::Value — pretty publish is not a
    // hot path and the bridge cost there is irrelevant.
    let json_string = if config.pretty {
        let bridge = Value::from(&source_data);
        serde_json::to_string_pretty(&bridge).map_err(|e| {
            DataflowError::Validation(format!("Failed to serialize to JSON: {}", e))
        })?
    } else {
        source_data.to_json_string()
    };

    let target_path = format!("data.{}", config.target);
    let old_value = get_nested_value(&message.context, &target_path)
        .cloned()
        .unwrap_or(OwnedDataValue::Null);
    let new_value = OwnedDataValue::String(json_string);

    set_nested_value(&mut message.context, &target_path, new_value.clone());

    Ok((
        200,
        vec![Change {
            path: Arc::from(target_path),
            old_value: Arc::new(old_value),
            new_value: Arc::new(new_value),
        }],
    ))
}

/// Execute `publish_xml`: serialise `data.{source}` to an XML string and
/// store at `data.{target}`. Bridges to `serde_json::Value` for the existing
/// recursive XML walker — XML is the slow path, no perf concern.
pub fn execute_publish_xml(
    message: &mut Message,
    config: &PublishConfig,
) -> Result<(usize, Vec<Change>)> {
    debug!(
        "PublishXml: Serializing 'data.{}' to 'data.{}'",
        config.source, config.target
    );

    let source_data = config.extract_source(message);

    if matches!(source_data, OwnedDataValue::Null) {
        return Err(DataflowError::Validation(format!(
            "PublishXml: Source 'data.{}' not found or is null",
            config.source
        )));
    }

    let bridge = Value::from(&source_data);
    let xml_string = json_to_xml(&bridge, &config.root_element)?;

    let target_path = format!("data.{}", config.target);
    let old_value = get_nested_value(&message.context, &target_path)
        .cloned()
        .unwrap_or(OwnedDataValue::Null);
    let new_value = OwnedDataValue::String(xml_string);

    set_nested_value(&mut message.context, &target_path, new_value.clone());

    Ok((
        200,
        vec![Change {
            path: Arc::from(target_path),
            old_value: Arc::new(old_value),
            new_value: Arc::new(new_value),
        }],
    ))
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

    fn dv(v: serde_json::Value) -> OwnedDataValue {
        OwnedDataValue::from(&v)
    }

    fn message_with_data(initial: serde_json::Value) -> Message {
        let mut m = Message::new(Arc::new(dv(json!({}))));
        set_nested_value(&mut m.context, "data", dv(initial));
        m
    }

    #[test]
    fn test_publish_config_from_json() {
        let input = json!({"source": "output", "target": "json_string"});
        let config = PublishConfig::from_json(&input).unwrap();
        assert_eq!(config.source, "output");
        assert_eq!(config.target, "json_string");
        assert!(!config.pretty);
        assert_eq!(config.root_element, "root");
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
        assert_eq!(config.source, "data");
        assert_eq!(config.target, "xml_output");
        assert!(config.pretty);
        assert_eq!(config.root_element, "document");
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
            source: "user".to_string(),
            target: "user_json".to_string(),
            pretty: false,
            root_element: "root".to_string(),
        };

        let result = execute_publish_json(&mut message, &config);
        assert!(result.is_ok());

        let (status, changes) = result.unwrap();
        assert_eq!(status, 200);
        assert_eq!(changes.len(), 1);

        let json_string = message.data()["user_json"].as_str().unwrap();
        assert!(json_string.contains("John"));
        assert!(json_string.contains("30"));
    }

    #[test]
    fn test_execute_publish_json_pretty() {
        let mut message = message_with_data(json!({"user": {"name": "Alice"}}));

        let config = PublishConfig {
            source: "user".to_string(),
            target: "output".to_string(),
            pretty: true,
            root_element: "root".to_string(),
        };

        let result = execute_publish_json(&mut message, &config);
        assert!(result.is_ok());

        let json_string = message.data()["output"].as_str().unwrap();
        assert!(json_string.contains('\n'));
    }

    #[test]
    fn test_execute_publish_json_not_found() {
        let mut message = Message::new(Arc::new(dv(json!({}))));

        let config = PublishConfig {
            source: "nonexistent".to_string(),
            target: "output".to_string(),
            pretty: false,
            root_element: "root".to_string(),
        };

        assert!(execute_publish_json(&mut message, &config).is_err());
    }

    #[test]
    fn test_execute_publish_xml() {
        let mut message = message_with_data(json!({"user": {"name": "John", "age": 30}}));

        let config = PublishConfig {
            source: "user".to_string(),
            target: "user_xml".to_string(),
            pretty: false,
            root_element: "user".to_string(),
        };

        let result = execute_publish_xml(&mut message, &config);
        assert!(result.is_ok());

        let (status, _) = result.unwrap();
        assert_eq!(status, 200);

        let xml_string = message.data()["user_xml"].as_str().unwrap();
        assert!(xml_string.contains("<user>"));
        assert!(xml_string.contains("</user>"));
        assert!(xml_string.contains("<name>John</name>"));
    }

    #[test]
    fn test_execute_publish_xml_not_found() {
        let mut message = Message::new(Arc::new(dv(json!({}))));

        let config = PublishConfig {
            source: "nonexistent".to_string(),
            target: "output".to_string(),
            pretty: false,
            root_element: "root".to_string(),
        };

        assert!(execute_publish_xml(&mut message, &config).is_err());
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
            source: "response.body".to_string(),
            target: "output".to_string(),
            pretty: false,
            root_element: "root".to_string(),
        };

        let result = execute_publish_json(&mut message, &config);
        assert!(result.is_ok());

        let json_string = message.data()["output"].as_str().unwrap();
        assert!(json_string.contains("success"));
    }
}
