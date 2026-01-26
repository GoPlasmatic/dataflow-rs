//! # Publish Function Module
//!
//! This module provides publishing capabilities for converting structured context data
//! into serialized string formats (JSON or XML). It's typically used at the end of a
//! workflow to prepare output data for transmission or storage.
//!
//! ## Features
//!
//! - Serialize data field to JSON string
//! - Serialize data field to XML string
//! - Support for nested source paths
//! - Automatic change tracking for audit trails
//!
//! ## Example Usage
//!
//! ```json
//! {
//!     "name": "publish_json",
//!     "input": {
//!         "source": "output",
//!         "target": "json_string"
//!     }
//! }
//! ```

use crate::engine::error::{DataflowError, Result};
use crate::engine::message::{Change, Message};
use crate::engine::utils::get_nested_value;
use log::debug;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// Configuration for publish functions.
///
/// Specifies where to read the source data from and where to store
/// the serialized result in the data context.
#[derive(Debug, Clone, Deserialize)]
pub struct PublishConfig {
    /// Source field name in data to serialize.
    /// - "field_name" - Read from data.field_name
    /// - "nested.field" - Read from data.nested.field
    pub source: String,

    /// Target field name in data where the serialized string will be stored.
    /// The result is stored at `data.{target}`.
    pub target: String,

    /// Whether to pretty-print the output (for JSON only).
    /// Defaults to false.
    #[serde(default)]
    pub pretty: bool,

    /// Root element name for XML output.
    /// Defaults to "root".
    #[serde(default = "default_root_element")]
    pub root_element: String,
}

fn default_root_element() -> String {
    "root".to_string()
}

impl PublishConfig {
    /// Parses a `PublishConfig` from a JSON value.
    ///
    /// # Arguments
    /// * `input` - JSON object containing "source" and "target" fields
    ///
    /// # Errors
    /// Returns `DataflowError::Validation` if required fields are missing
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

    /// Extract source data from the data context.
    ///
    /// # Arguments
    /// * `message` - The message to extract data from
    ///
    /// # Returns
    /// The extracted value, or Value::Null if not found
    fn extract_source(&self, message: &Message) -> Value {
        // Check if source is a direct field in data
        if let Some(value) = message.data().get(&self.source) {
            return value.clone();
        }

        // Try nested path in data
        if let Some(value) = get_nested_value(message.data(), &self.source) {
            return value.clone();
        }

        // Try full context path (for data.field syntax)
        if let Some(path) = self.source.strip_prefix("data.")
            && let Some(value) = get_nested_value(message.data(), path)
        {
            return value.clone();
        }

        Value::Null
    }
}

/// Execute publish_json operation.
///
/// Serializes data from the source path to a JSON string and stores it
/// in the target data field.
///
/// # Arguments
/// * `message` - The message to process (modified in place)
/// * `config` - Publish configuration specifying source and target
///
/// # Returns
/// * `Ok((200, changes))` - Success with list of changes for audit trail
/// * `Err` - If configuration is invalid or serialization fails
pub fn execute_publish_json(
    message: &mut Message,
    config: &PublishConfig,
) -> Result<(usize, Vec<Change>)> {
    debug!(
        "PublishJson: Serializing 'data.{}' to 'data.{}'",
        config.source, config.target
    );

    // Extract source data
    let source_data = config.extract_source(message);

    if source_data.is_null() {
        return Err(DataflowError::Validation(format!(
            "PublishJson: Source 'data.{}' not found or is null",
            config.source
        )));
    }

    // Serialize to JSON string
    let json_string = if config.pretty {
        serde_json::to_string_pretty(&source_data)
    } else {
        serde_json::to_string(&source_data)
    }
    .map_err(|e| DataflowError::Validation(format!("Failed to serialize to JSON: {}", e)))?;

    // Get old value for change tracking
    let old_value = message
        .data()
        .get(&config.target)
        .cloned()
        .unwrap_or(Value::Null);

    let new_value = Value::String(json_string);

    // Store to target in data
    if let Some(data_obj) = message.data_mut().as_object_mut() {
        data_obj.insert(config.target.clone(), new_value.clone());
    }

    // Invalidate context cache
    message.invalidate_context_cache();

    debug!(
        "PublishJson: Successfully serialized to 'data.{}'",
        config.target
    );

    Ok((
        200,
        vec![Change {
            path: Arc::from(format!("data.{}", config.target)),
            old_value: Arc::new(old_value),
            new_value: Arc::new(new_value),
        }],
    ))
}

/// Execute publish_xml operation.
///
/// Serializes data from the source path to an XML string and stores it
/// in the target data field.
///
/// # Arguments
/// * `message` - The message to process (modified in place)
/// * `config` - Publish configuration specifying source and target
///
/// # Returns
/// * `Ok((200, changes))` - Success with list of changes for audit trail
/// * `Err` - If configuration is invalid or serialization fails
pub fn execute_publish_xml(
    message: &mut Message,
    config: &PublishConfig,
) -> Result<(usize, Vec<Change>)> {
    debug!(
        "PublishXml: Serializing 'data.{}' to 'data.{}'",
        config.source, config.target
    );

    // Extract source data
    let source_data = config.extract_source(message);

    if source_data.is_null() {
        return Err(DataflowError::Validation(format!(
            "PublishXml: Source 'data.{}' not found or is null",
            config.source
        )));
    }

    // Serialize to XML string
    let xml_string = json_to_xml(&source_data, &config.root_element)?;

    // Get old value for change tracking
    let old_value = message
        .data()
        .get(&config.target)
        .cloned()
        .unwrap_or(Value::Null);

    let new_value = Value::String(xml_string);

    // Store to target in data
    if let Some(data_obj) = message.data_mut().as_object_mut() {
        data_obj.insert(config.target.clone(), new_value.clone());
    }

    // Invalidate context cache
    message.invalidate_context_cache();

    debug!(
        "PublishXml: Successfully serialized to 'data.{}'",
        config.target
    );

    Ok((
        200,
        vec![Change {
            path: Arc::from(format!("data.{}", config.target)),
            old_value: Arc::new(old_value),
            new_value: Arc::new(new_value),
        }],
    ))
}

/// Convert JSON Value to XML string.
///
/// Uses a recursive approach to convert JSON to XML.
fn json_to_xml(value: &Value, root_element: &str) -> Result<String> {
    let mut buffer = String::new();

    // For objects, serialize directly with root element
    match value {
        Value::Object(_) => {
            // Create XML with custom root element
            buffer.push_str(&format!("<{}>", root_element));

            // Serialize the object contents
            let content = serialize_value_to_xml_content(value)?;
            buffer.push_str(&content);

            buffer.push_str(&format!("</{}>", root_element));
        }
        Value::Array(arr) => {
            // For arrays, wrap each item
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
            // For primitives, wrap in root element
            buffer.push_str(&format!("<{}>", root_element));
            buffer.push_str(&value_to_xml_string(value));
            buffer.push_str(&format!("</{}>", root_element));
        }
    }

    Ok(buffer)
}

/// Serialize a JSON value's content to XML (without root wrapper).
fn serialize_value_to_xml_content(value: &Value) -> Result<String> {
    let mut result = String::new();

    match value {
        Value::Object(map) => {
            for (key, val) in map {
                // Sanitize key for XML element name
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

/// Convert a primitive JSON value to an XML-safe string.
fn value_to_xml_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => escape_xml(s),
        _ => String::new(),
    }
}

/// Escape special XML characters.
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Sanitize a string to be a valid XML element name.
fn sanitize_xml_name(name: &str) -> String {
    let mut result = String::new();

    for (i, c) in name.chars().enumerate() {
        if i == 0 {
            // First character must be letter or underscore
            if c.is_ascii_alphabetic() || c == '_' {
                result.push(c);
            } else {
                result.push('_');
                if c.is_ascii_alphanumeric() {
                    result.push(c);
                }
            }
        } else {
            // Subsequent characters can be letter, digit, hyphen, underscore, or period
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                result.push(c);
            } else {
                result.push('_');
            }
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

    #[test]
    fn test_publish_config_from_json() {
        let input = json!({
            "source": "output",
            "target": "json_string"
        });

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
        let input = json!({
            "target": "output"
        });

        let result = PublishConfig::from_json(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_publish_config_missing_target() {
        let input = json!({
            "source": "input"
        });

        let result = PublishConfig::from_json(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_execute_publish_json() {
        let mut message = Message::new(Arc::new(json!({})));
        message.context["data"] = json!({
            "user": {
                "name": "John",
                "age": 30
            }
        });

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

        // Verify JSON string was created
        let json_string = message.data()["user_json"].as_str().unwrap();
        assert!(json_string.contains("John"));
        assert!(json_string.contains("30"));
    }

    #[test]
    fn test_execute_publish_json_pretty() {
        let mut message = Message::new(Arc::new(json!({})));
        message.context["data"] = json!({
            "user": {
                "name": "Alice"
            }
        });

        let config = PublishConfig {
            source: "user".to_string(),
            target: "output".to_string(),
            pretty: true,
            root_element: "root".to_string(),
        };

        let result = execute_publish_json(&mut message, &config);
        assert!(result.is_ok());

        let json_string = message.data()["output"].as_str().unwrap();
        // Pretty printed JSON has newlines
        assert!(json_string.contains('\n'));
    }

    #[test]
    fn test_execute_publish_json_not_found() {
        let mut message = Message::new(Arc::new(json!({})));

        let config = PublishConfig {
            source: "nonexistent".to_string(),
            target: "output".to_string(),
            pretty: false,
            root_element: "root".to_string(),
        };

        let result = execute_publish_json(&mut message, &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_execute_publish_xml() {
        let mut message = Message::new(Arc::new(json!({})));
        message.context["data"] = json!({
            "user": {
                "name": "John",
                "age": 30
            }
        });

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

        // Verify XML string was created
        let xml_string = message.data()["user_xml"].as_str().unwrap();
        assert!(xml_string.contains("<user>"));
        assert!(xml_string.contains("</user>"));
        assert!(xml_string.contains("<name>John</name>"));
    }

    #[test]
    fn test_execute_publish_xml_not_found() {
        let mut message = Message::new(Arc::new(json!({})));

        let config = PublishConfig {
            source: "nonexistent".to_string(),
            target: "output".to_string(),
            pretty: false,
            root_element: "root".to_string(),
        };

        let result = execute_publish_xml(&mut message, &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_json_to_xml_simple() {
        let value = json!({
            "name": "Test",
            "value": 42
        });

        let result = json_to_xml(&value, "root");
        assert!(result.is_ok());

        let xml = result.unwrap();
        assert!(xml.contains("<root>"));
        assert!(xml.contains("</root>"));
        assert!(xml.contains("<name>Test</name>"));
        assert!(xml.contains("<value>42</value>"));
    }

    #[test]
    fn test_json_to_xml_nested() {
        let value = json!({
            "user": {
                "name": "Alice",
                "email": "alice@example.com"
            }
        });

        let result = json_to_xml(&value, "data");
        assert!(result.is_ok());

        let xml = result.unwrap();
        assert!(xml.contains("<data>"));
        assert!(xml.contains("<user>"));
        assert!(xml.contains("<name>Alice</name>"));
    }

    #[test]
    fn test_json_to_xml_array() {
        let value = json!([1, 2, 3]);

        let result = json_to_xml(&value, "numbers");
        assert!(result.is_ok());

        let xml = result.unwrap();
        assert!(xml.contains("<numbers>"));
        assert!(xml.contains("<item>1</item>"));
        assert!(xml.contains("<item>2</item>"));
        assert!(xml.contains("<item>3</item>"));
    }

    #[test]
    fn test_json_to_xml_special_chars() {
        let value = json!({
            "text": "<script>alert('xss')</script>"
        });

        let result = json_to_xml(&value, "root");
        assert!(result.is_ok());

        let xml = result.unwrap();
        // Should be escaped
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
        let mut message = Message::new(Arc::new(json!({})));
        message.context["data"] = json!({
            "response": {
                "body": {
                    "message": "success"
                }
            }
        });

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
