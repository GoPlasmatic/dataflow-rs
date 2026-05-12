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
use crate::engine::executor::ArenaContext;
use crate::engine::message::{null_arc, Change, Message};
use crate::engine::utils::{get_nested_value, set_nested_value};
use datavalue::OwnedDataValue;
use log::debug;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// Configuration for parse functions.
#[derive(Debug, Clone, Deserialize)]
pub struct ParseConfig {
    /// Source path to read from.
    pub source: String,

    /// Target field name in `data` (stored at `data.{target}`).
    pub target: String,
}

impl ParseConfig {
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

    /// Extract the source value as an owned `OwnedDataValue`.
    fn extract_source(&self, message: &Message) -> OwnedDataValue {
        if self.source == "payload" {
            (*message.payload).clone()
        } else if let Some(path) = self.source.strip_prefix("payload.") {
            get_nested_value(&message.payload, path)
                .cloned()
                .unwrap_or(OwnedDataValue::Null)
        } else if let Some(path) = self.source.strip_prefix("data.") {
            get_nested_value(message.data(), path)
                .cloned()
                .unwrap_or(OwnedDataValue::Null)
        } else {
            get_nested_value(&message.context, &self.source)
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
) -> Result<(usize, Vec<Change>)> {
    debug!(
        "ParseJson: Extracting from '{}' to 'data.{}'",
        config.source, config.target
    );

    let target_path = format!("data.{}", config.target);

    // Hot path: source == "payload" and not a JSON-string payload. The
    // payload Arc is already on the message; clone-into-context once, reuse
    // the Arc for the audit entry (refcount bump). This is the realistic
    // benchmark's exact shape.
    let payload_fast_path =
        config.source == "payload" && !matches!(*message.payload, OwnedDataValue::String(_));

    if message.capture_changes {
        let old_value_arc: Arc<OwnedDataValue> =
            match get_nested_value(&message.context, &target_path) {
                Some(v) => Arc::new(v.clone()),
                None => null_arc(),
            };

        let (new_value_arc, source_data_for_context) = if payload_fast_path {
            let arc = Arc::clone(&message.payload);
            let cloned: OwnedDataValue = (*arc).clone();
            (arc, cloned)
        } else {
            let raw = config.extract_source(message);
            let source_data = match &raw {
                OwnedDataValue::String(s) => {
                    OwnedDataValue::from_json(s).unwrap_or_else(|_| raw.clone())
                }
                _ => raw,
            };
            let arc = Arc::new(source_data);
            let cloned: OwnedDataValue = (*arc).clone();
            (arc, cloned)
        };

        set_nested_value(&mut message.context, &target_path, source_data_for_context);
        debug!(
            "ParseJson: Successfully stored data to 'data.{}'",
            config.target
        );
        return Ok((
            200,
            vec![Change {
                path: Arc::from(target_path),
                old_value: old_value_arc,
                new_value: new_value_arc,
            }],
        ));
    }

    // Audit-off fast path: only the deep clone into the context survives.
    let source_data_for_context: OwnedDataValue = if payload_fast_path {
        (*message.payload).clone()
    } else {
        let raw = config.extract_source(message);
        match &raw {
            OwnedDataValue::String(s) => OwnedDataValue::from_json(s).unwrap_or_else(|_| raw.clone()),
            _ => raw,
        }
    };
    set_nested_value(&mut message.context, &target_path, source_data_for_context);

    debug!(
        "ParseJson: Successfully stored data to 'data.{}'",
        config.target
    );

    Ok((200, Vec::new()))
}

/// Same as `execute_parse_json` but also refreshes the supplied
/// `ArenaContext` so subsequent sync tasks in the same workflow stretch see
/// the written `data.<target>` slot without rebuilding the whole arena form.
pub(crate) fn execute_parse_json_in_arena(
    message: &mut Message,
    config: &ParseConfig,
    arena_ctx: &mut ArenaContext<'_>,
) -> Result<(usize, Vec<Change>)> {
    // Resolve the write target before calling execute_parse_json so we can
    // refresh the arena slot afterwards using the same path.
    let target_path = format!("data.{}", config.target);
    let result = execute_parse_json(message, config)?;
    // Refresh ONLY the affected depth-2 slot in the arena cache. For
    // source == "payload" target = "input", this is `data.input` — the
    // heavy slot — but it's re-arena'd exactly once per workflow stretch
    // here, not once per subsequent map mapping.
    arena_ctx.refresh_for_path(&message.context, &target_path);
    Ok(result)
}

/// Execute `parse_xml`: read the source string, parse XML into a
/// `serde_json::Value` (existing quick-xml path), convert to `OwnedDataValue`,
/// store under `data.{target}`.
pub fn execute_parse_xml(
    message: &mut Message,
    config: &ParseConfig,
) -> Result<(usize, Vec<Change>)> {
    debug!(
        "ParseXml: Extracting from '{}' to 'data.{}'",
        config.source, config.target
    );

    let source_data = config.extract_source(message);

    let xml_string = match &source_data {
        OwnedDataValue::String(s) => s.clone(),
        _ => {
            return Err(DataflowError::Validation(format!(
                "ParseXml: Source '{}' is not a string",
                config.source
            )));
        }
    };

    let parsed_json = xml_to_json(&xml_string)?;
    let parsed_owned = OwnedDataValue::from(&parsed_json);

    let target_path = format!("data.{}", config.target);
    let old_value_arc: Arc<OwnedDataValue> =
        match get_nested_value(&message.context, &target_path) {
            Some(v) => Arc::new(v.clone()),
            None => null_arc(),
        };

    set_nested_value(&mut message.context, &target_path, parsed_owned.clone());

    debug!(
        "ParseXml: Successfully parsed and stored XML to 'data.{}'",
        config.target
    );

    Ok((
        200,
        vec![Change {
            path: Arc::from(target_path),
            old_value: old_value_arc,
            new_value: Arc::new(parsed_owned),
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
    use serde_json::json;

    fn dv(v: serde_json::Value) -> OwnedDataValue {
        OwnedDataValue::from(&v)
    }

    #[test]
    fn test_parse_config_from_json() {
        let input = json!({"source": "payload", "target": "input_data"});
        let config = ParseConfig::from_json(&input).unwrap();
        assert_eq!(config.source, "payload");
        assert_eq!(config.target, "input_data");
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
            source: "payload".to_string(),
            target: "input".to_string(),
        };

        let result = execute_parse_json(&mut message, &config);
        assert!(result.is_ok());

        let (status, changes) = result.unwrap();
        assert_eq!(status, 200);
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
            source: "payload.body.user".to_string(),
            target: "user_data".to_string(),
        };

        let result = execute_parse_json(&mut message, &config);
        assert!(result.is_ok());

        let (status, _) = result.unwrap();
        assert_eq!(status, 200);
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
            source: "data.existing".to_string(),
            target: "copied".to_string(),
        };

        let result = execute_parse_json(&mut message, &config);
        assert!(result.is_ok());

        assert_eq!(message.data()["copied"]["value"], dv(json!(42)));
    }

    #[test]
    fn test_execute_parse_xml_simple() {
        let xml_payload = json!("<root><name>John</name><age>30</age></root>");
        let mut message = Message::from_value(&xml_payload);

        let config = ParseConfig {
            source: "payload".to_string(),
            target: "parsed".to_string(),
        };

        let result = execute_parse_xml(&mut message, &config);
        assert!(result.is_ok());

        let (status, _) = result.unwrap();
        assert_eq!(status, 200);

        let parsed = &message.data()["parsed"];
        assert!(parsed.is_object());
    }

    #[test]
    fn test_execute_parse_xml_not_string() {
        let payload = json!({"not": "a string"});
        let mut message = Message::from_value(&payload);

        let config = ParseConfig {
            source: "payload".to_string(),
            target: "parsed".to_string(),
        };

        assert!(execute_parse_xml(&mut message, &config).is_err());
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
            source: "payload".to_string(),
            target: "input".to_string(),
        };

        let result = execute_parse_json(&mut message, &config);
        assert!(result.is_ok());

        let (status, _) = result.unwrap();
        assert_eq!(status, 200);

        assert_eq!(message.data()["input"]["name"], dv(json!("John")));
        assert_eq!(message.data()["input"]["age"], dv(json!(30)));
    }
}
