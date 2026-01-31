//! # Parse Function Module
//!
//! This module provides parsing capabilities for converting payload data into structured
//! context data. It supports JSON and XML parsing, allowing workflows to start by loading
//! payload into the context where it can be accessed by subsequent tasks.
//!
//! ## Features
//!
//! - Parse JSON payload into data field
//! - Parse XML payload into JSON data field
//! - Support for nested source paths (payload.body, data.field)
//! - Automatic change tracking for audit trails
//!
//! ## Example Usage
//!
//! ```json
//! {
//!     "name": "parse_json",
//!     "input": {
//!         "source": "payload",
//!         "target": "input_data"
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

/// Configuration for parse functions.
///
/// Specifies where to read the source data from and where to store
/// the parsed result in the data context.
#[derive(Debug, Clone, Deserialize)]
pub struct ParseConfig {
    /// Source path to read from.
    /// - "payload" - Read the entire payload
    /// - "payload.field" - Read a nested field from payload
    /// - "data.field" - Read from existing data context
    pub source: String,

    /// Target field name in data where the parsed result will be stored.
    /// The result is stored at `data.{target}`.
    pub target: String,
}

impl ParseConfig {
    /// Parses a `ParseConfig` from a JSON value.
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
                DataflowError::Validation("Missing 'source' in parse config".to_string())
            })?
            .to_string();

        let target = input
            .get("target")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                DataflowError::Validation("Missing 'target' in parse config".to_string())
            })?
            .to_string();

        Ok(ParseConfig { source, target })
    }

    /// Extract source data based on the source path configuration.
    ///
    /// # Arguments
    /// * `message` - The message to extract data from
    ///
    /// # Returns
    /// The extracted value, or Value::Null if not found
    fn extract_source(&self, message: &Message) -> Value {
        if self.source == "payload" {
            (*message.payload).clone()
        } else if let Some(path) = self.source.strip_prefix("payload.") {
            get_nested_value(&message.payload, path)
                .cloned()
                .unwrap_or(Value::Null)
        } else if let Some(path) = self.source.strip_prefix("data.") {
            get_nested_value(message.data(), path)
                .cloned()
                .unwrap_or(Value::Null)
        } else {
            // Try to get from context directly
            get_nested_value(&message.context, &self.source)
                .cloned()
                .unwrap_or(Value::Null)
        }
    }
}

/// Execute parse_json operation.
///
/// Extracts JSON data from the source path and stores it in the target data field.
/// This is typically used at the start of a workflow to load payload into context.
///
/// # Arguments
/// * `message` - The message to process (modified in place)
/// * `config` - Parse configuration specifying source and target
///
/// # Returns
/// * `Ok((200, changes))` - Success with list of changes for audit trail
/// * `Err` - If configuration is invalid
pub fn execute_parse_json(
    message: &mut Message,
    config: &ParseConfig,
) -> Result<(usize, Vec<Change>)> {
    debug!(
        "ParseJson: Extracting from '{}' to 'data.{}'",
        config.source, config.target
    );

    // Extract source data
    let source_data = config.extract_source(message);

    // If source is a JSON string, parse it into a structured value
    let source_data = match &source_data {
        Value::String(s) => serde_json::from_str(s).unwrap_or(source_data),
        _ => source_data,
    };

    // Get old value for change tracking
    let old_value = message
        .data()
        .get(&config.target)
        .cloned()
        .unwrap_or(Value::Null);

    // Store to target in data
    if let Some(data_obj) = message.data_mut().as_object_mut() {
        data_obj.insert(config.target.clone(), source_data.clone());
    }

    // Invalidate context cache
    message.invalidate_context_cache();

    debug!(
        "ParseJson: Successfully stored data to 'data.{}'",
        config.target
    );

    Ok((
        200,
        vec![Change {
            path: Arc::from(format!("data.{}", config.target)),
            old_value: Arc::new(old_value),
            new_value: Arc::new(source_data),
        }],
    ))
}

/// Execute parse_xml operation.
///
/// Extracts XML string from the source path, parses it to JSON, and stores
/// it in the target data field.
///
/// # Arguments
/// * `message` - The message to process (modified in place)
/// * `config` - Parse configuration specifying source and target
///
/// # Returns
/// * `Ok((200, changes))` - Success with list of changes for audit trail
/// * `Err` - If configuration is invalid or XML parsing fails
pub fn execute_parse_xml(
    message: &mut Message,
    config: &ParseConfig,
) -> Result<(usize, Vec<Change>)> {
    debug!(
        "ParseXml: Extracting from '{}' to 'data.{}'",
        config.source, config.target
    );

    // Extract source data
    let source_data = config.extract_source(message);

    // Get XML string
    let xml_string = match &source_data {
        Value::String(s) => s.clone(),
        _ => {
            return Err(DataflowError::Validation(format!(
                "ParseXml: Source '{}' is not a string",
                config.source
            )));
        }
    };

    // Parse XML to JSON
    let parsed_json = xml_to_json(&xml_string)?;

    // Get old value for change tracking
    let old_value = message
        .data()
        .get(&config.target)
        .cloned()
        .unwrap_or(Value::Null);

    // Store to target in data
    if let Some(data_obj) = message.data_mut().as_object_mut() {
        data_obj.insert(config.target.clone(), parsed_json.clone());
    }

    // Invalidate context cache
    message.invalidate_context_cache();

    debug!(
        "ParseXml: Successfully parsed and stored XML to 'data.{}'",
        config.target
    );

    Ok((
        200,
        vec![Change {
            path: Arc::from(format!("data.{}", config.target)),
            old_value: Arc::new(old_value),
            new_value: Arc::new(parsed_json),
        }],
    ))
}

/// Convert XML string to JSON Value.
///
/// Uses quick-xml with serde for conversion. The resulting JSON structure
/// follows the convention where:
/// - Element names become object keys
/// - Text content is stored under "$text" key
/// - Attributes are stored under "$attr" key
/// - Multiple child elements with same name become arrays
fn xml_to_json(xml: &str) -> Result<Value> {
    use quick_xml::de::from_str;

    // Parse XML to JSON using quick-xml's serde support
    let parsed: Value = from_str(xml)
        .map_err(|e| DataflowError::Validation(format!("Failed to parse XML: {}", e)))?;

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_config_from_json() {
        let input = json!({
            "source": "payload",
            "target": "input_data"
        });

        let config = ParseConfig::from_json(&input).unwrap();
        assert_eq!(config.source, "payload");
        assert_eq!(config.target, "input_data");
    }

    #[test]
    fn test_parse_config_missing_source() {
        let input = json!({
            "target": "input_data"
        });

        let result = ParseConfig::from_json(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_config_missing_target() {
        let input = json!({
            "source": "payload"
        });

        let result = ParseConfig::from_json(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_execute_parse_json_from_payload() {
        let payload = json!({
            "name": "John",
            "age": 30
        });
        let mut message = Message::new(Arc::new(payload));

        let config = ParseConfig {
            source: "payload".to_string(),
            target: "input".to_string(),
        };

        let result = execute_parse_json(&mut message, &config);
        assert!(result.is_ok());

        let (status, changes) = result.unwrap();
        assert_eq!(status, 200);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path.as_ref(), "data.input");

        // Verify data was stored
        assert_eq!(message.data()["input"]["name"], json!("John"));
        assert_eq!(message.data()["input"]["age"], json!(30));
    }

    #[test]
    fn test_execute_parse_json_from_nested_payload() {
        let payload = json!({
            "body": {
                "user": {
                    "name": "Alice"
                }
            }
        });
        let mut message = Message::new(Arc::new(payload));

        let config = ParseConfig {
            source: "payload.body.user".to_string(),
            target: "user_data".to_string(),
        };

        let result = execute_parse_json(&mut message, &config);
        assert!(result.is_ok());

        let (status, _) = result.unwrap();
        assert_eq!(status, 200);

        // Verify nested data was extracted
        assert_eq!(message.data()["user_data"]["name"], json!("Alice"));
    }

    #[test]
    fn test_execute_parse_json_from_data() {
        let mut message = Message::new(Arc::new(json!({})));
        message.context["data"] = json!({
            "existing": {
                "value": 42
            }
        });

        let config = ParseConfig {
            source: "data.existing".to_string(),
            target: "copied".to_string(),
        };

        let result = execute_parse_json(&mut message, &config);
        assert!(result.is_ok());

        // Verify data was copied
        assert_eq!(message.data()["copied"]["value"], json!(42));
    }

    #[test]
    fn test_execute_parse_xml_simple() {
        let xml_payload = json!("<root><name>John</name><age>30</age></root>");
        let mut message = Message::new(Arc::new(xml_payload));

        let config = ParseConfig {
            source: "payload".to_string(),
            target: "parsed".to_string(),
        };

        let result = execute_parse_xml(&mut message, &config);
        assert!(result.is_ok());

        let (status, _) = result.unwrap();
        assert_eq!(status, 200);

        // Verify XML was parsed
        let parsed = &message.data()["parsed"];
        assert!(parsed.is_object());
    }

    #[test]
    fn test_execute_parse_xml_not_string() {
        let payload = json!({"not": "a string"});
        let mut message = Message::new(Arc::new(payload));

        let config = ParseConfig {
            source: "payload".to_string(),
            target: "parsed".to_string(),
        };

        let result = execute_parse_xml(&mut message, &config);
        assert!(result.is_err());
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
        // Test with unclosed tag
        let xml = "<root><unclosed>";
        let result = xml_to_json(xml);
        assert!(result.is_err());
    }

    #[test]
    fn test_xml_to_json_with_attributes() {
        let xml = r#"<person id="123"><name>John</name></person>"#;
        let result = xml_to_json(xml);
        assert!(result.is_ok());
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
        // Simulate WASM layer storing payload as a raw JSON string
        let payload = Value::String(r#"{"name":"John","age":30}"#.to_string());
        let mut message = Message::new(Arc::new(payload));

        let config = ParseConfig {
            source: "payload".to_string(),
            target: "input".to_string(),
        };

        let result = execute_parse_json(&mut message, &config);
        assert!(result.is_ok());

        let (status, _) = result.unwrap();
        assert_eq!(status, 200);

        // Verify the JSON string was parsed into a structured value
        assert_eq!(message.data()["input"]["name"], json!("John"));
        assert_eq!(message.data()["input"]["age"], json!(30));
    }
}
