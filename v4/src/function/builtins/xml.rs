//! XML in and out for `parse_xml` and `publish_xml`: v3's `xml_to_json` and
//! its writer, text for text.
//!
//! Both go through [`serde_json::Value`], as v3's did: an object's keys come
//! out sorted (v4 builds serde_json without `preserve_order`, as v3 does).

use crate::base::{Error, Result};
use serde_json::Value;

/// A `VALIDATION_ERROR`, with the `Validation error: ` prefix v3's display
/// adds (I62).
pub(super) fn validation_error(message: impl std::fmt::Display) -> Error {
    Error::service("VALIDATION_ERROR", format!("Validation error: {message}"))
}

/// The document as a JSON value, by `quick-xml`'s serde reader.
///
/// # Errors
///
/// `VALIDATION_ERROR` "Validation error: Failed to parse XML: …" when the
/// text is not a document the reader accepts.
pub(super) fn xml_to_json(xml: &str) -> Result<Value> {
    quick_xml::de::from_str(xml)
        .map_err(|e| validation_error(format_args!("Failed to parse XML: {e}")))
}

/// `value` as an XML document under `root_element`.
pub(super) fn json_to_xml(value: &Value, root_element: &str) -> String {
    let mut buffer = String::new();
    buffer.push_str(&format!("<{root_element}>"));
    match value {
        Value::Object(_) => buffer.push_str(&content(value)),
        Value::Array(items) => {
            for item in items {
                buffer.push_str("<item>");
                buffer.push_str(&content(item));
                buffer.push_str("</item>");
            }
        }
        _ => buffer.push_str(&scalar(value)),
    }
    buffer.push_str(&format!("</{root_element}>"));
    buffer
}

/// The inside of an element holding `value`.
fn content(value: &Value) -> String {
    let mut result = String::new();
    match value {
        Value::Object(map) => {
            for (key, val) in map {
                let key = sanitize_name(key);
                result.push_str(&format!("<{key}>"));
                match val {
                    Value::Object(_) | Value::Array(_) => result.push_str(&content(val)),
                    _ => result.push_str(&scalar(val)),
                }
                result.push_str(&format!("</{key}>"));
            }
        }
        Value::Array(items) => {
            for item in items {
                result.push_str("<item>");
                match item {
                    Value::Object(_) | Value::Array(_) => result.push_str(&content(item)),
                    _ => result.push_str(&scalar(item)),
                }
                result.push_str("</item>");
            }
        }
        _ => result.push_str(&scalar(value)),
    }
    result
}

/// A scalar's text; `null` is empty.
fn scalar(value: &Value) -> String {
    match value {
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => escape(s),
        _ => String::new(),
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// An element name made of what XML allows, as v3's `sanitize_xml_name`.
fn sanitize_name(name: &str) -> String {
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
