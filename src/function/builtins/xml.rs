//! XML in and out for `parse_xml` and `publish_xml`: v3's `xml_to_json` and
//! its writer, text for text.
//!
//! An object's keys keep the order the value holds, in both directions: a
//! document's elements read in document order, and a value writes its keys in
//! its own order. v3 went through [`serde_json::Value`], whose keys are sorted
//! unless serde_json's `preserve_order` is on; v4 never does, so its order is
//! the same in every build (break 23: v3's default build sorted them).

use crate::base::{Error, Result};
use datavalue::{NumberValue, OwnedDataValue};
use serde::Deserializer;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use std::collections::HashMap;
use std::fmt;

/// A `VALIDATION_ERROR`, with the `Validation error: ` prefix v3's display
/// adds (I62).
pub(super) fn validation_error(message: impl std::fmt::Display) -> Error {
    Error::service("VALIDATION_ERROR", format!("Validation error: {message}"))
}

/// The document as a value, by `quick-xml`'s serde reader, its elements in
/// document order. A repeated element keeps its first position and its last
/// value, as `serde_json::Value` read it with `preserve_order` on.
///
/// # Errors
///
/// `VALIDATION_ERROR` "Validation error: Failed to parse XML: …" when the
/// text is not a document the reader accepts.
pub(super) fn xml_to_value(xml: &str) -> Result<OwnedDataValue> {
    quick_xml::de::from_str(xml)
        .map(|Read(value)| value)
        .map_err(|e| validation_error(format_args!("Failed to parse XML: {e}")))
}

/// [`xml_to_value`]'s reader: what `serde_json::Value`'s reader builds,
/// bridged to an [`OwnedDataValue`], with an object's keys in the order read.
struct InOrder;

/// One value read by [`InOrder`].
struct Read(OwnedDataValue);

impl<'de> de::Deserialize<'de> for Read {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        d.deserialize_any(InOrder).map(Read)
    }
}

/// Above this many keys, an object finds a repeated key through an index
/// rather than a scan.
const SCAN: usize = 8;

impl<'de> Visitor<'de> for InOrder {
    type Value = OwnedDataValue;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any valid JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> std::result::Result<OwnedDataValue, E> {
        Ok(OwnedDataValue::Null)
    }

    fn visit_none<E: de::Error>(self) -> std::result::Result<OwnedDataValue, E> {
        Ok(OwnedDataValue::Null)
    }

    fn visit_some<D: Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<OwnedDataValue, D::Error> {
        d.deserialize_any(Self)
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<OwnedDataValue, E> {
        Ok(OwnedDataValue::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<OwnedDataValue, E> {
        Ok(OwnedDataValue::Number(NumberValue::Integer(v)))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<OwnedDataValue, E> {
        Ok(OwnedDataValue::Number(NumberValue::from_u64(v)))
    }

    /// A finite float; anything else is `null`, as `serde_json::Value` reads
    /// one.
    fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<OwnedDataValue, E> {
        Ok(if v.is_finite() {
            OwnedDataValue::Number(NumberValue::Float(v))
        } else {
            OwnedDataValue::Null
        })
    }

    fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<OwnedDataValue, E> {
        Ok(OwnedDataValue::String(v.to_owned()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<OwnedDataValue, E> {
        Ok(OwnedDataValue::String(v))
    }

    fn visit_seq<A: SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<OwnedDataValue, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(Read(item)) = seq.next_element()? {
            items.push(item);
        }
        Ok(OwnedDataValue::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(
        self,
        mut map: A,
    ) -> std::result::Result<OwnedDataValue, A::Error> {
        let mut pairs: Vec<(String, OwnedDataValue)> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        while let Some(key) = map.next_key::<String>()? {
            let Read(value) = map.next_value()?;
            let at = if pairs.len() <= SCAN {
                pairs.iter().position(|(k, _)| *k == key)
            } else {
                if index.is_empty() {
                    index.extend(pairs.iter().enumerate().map(|(i, (k, _))| (k.clone(), i)));
                }
                index.get(&key).copied()
            };
            match at {
                Some(at) => pairs[at].1 = value,
                None => {
                    if !index.is_empty() {
                        index.insert(key.clone(), pairs.len());
                    }
                    pairs.push((key, value));
                }
            }
        }
        Ok(OwnedDataValue::Object(pairs))
    }
}

/// `value` as an XML document under `root_element`, its keys in the value's
/// order.
pub(super) fn value_to_xml(value: &OwnedDataValue, root_element: &str) -> String {
    let mut buffer = String::new();
    buffer.push_str(&format!("<{root_element}>"));
    match value {
        OwnedDataValue::Object(_) => buffer.push_str(&content(value)),
        OwnedDataValue::Array(items) => {
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
fn content(value: &OwnedDataValue) -> String {
    let mut result = String::new();
    match value {
        OwnedDataValue::Object(pairs) => {
            for (key, val) in pairs {
                let key = sanitize_name(key);
                result.push_str(&format!("<{key}>"));
                match val {
                    OwnedDataValue::Object(_) | OwnedDataValue::Array(_) => {
                        result.push_str(&content(val));
                    }
                    _ => result.push_str(&scalar(val)),
                }
                result.push_str(&format!("</{key}>"));
            }
        }
        OwnedDataValue::Array(items) => {
            for item in items {
                result.push_str("<item>");
                match item {
                    OwnedDataValue::Object(_) | OwnedDataValue::Array(_) => {
                        result.push_str(&content(item));
                    }
                    _ => result.push_str(&scalar(item)),
                }
                result.push_str("</item>");
            }
        }
        _ => result.push_str(&scalar(value)),
    }
    result
}

/// A scalar's text, as `serde_json::Value` writes it; `null` and a
/// non-finite float are empty.
fn scalar(value: &OwnedDataValue) -> String {
    match value {
        OwnedDataValue::Null => String::new(),
        OwnedDataValue::Bool(b) => b.to_string(),
        OwnedDataValue::Number(NumberValue::Integer(i)) => i.to_string(),
        OwnedDataValue::Number(NumberValue::Float(f)) => {
            serde_json::Number::from_f64(*f).map_or_else(String::new, |n| n.to_string())
        }
        OwnedDataValue::String(s) => escape(s),
        // An object or an array is no scalar: `content` walks those.
        OwnedDataValue::Object(_) | OwnedDataValue::Array(_) => String::new(),
        #[allow(
            unreachable_patterns,
            reason = "the catch-all is live only with datavalue's `datetime` or `tensor` \
                      feature, which feature unification can turn on from another crate"
        )]
        other => foreign(other),
    }
}

/// A value JSON has no type for (a datetime, a duration, a tensor), in its
/// JSON form: a datetime's and a duration's text, a tensor's tagged object
/// walked in the order its `Serialize` writes it.
#[cold]
fn foreign(value: &OwnedDataValue) -> String {
    let text = serde_json::to_string(value).unwrap_or_default();
    match crate::base::exact(&mut serde_json::Deserializer::from_str(&text)) {
        Ok(OwnedDataValue::String(s)) => escape(&s),
        Ok(json @ (OwnedDataValue::Object(_) | OwnedDataValue::Array(_))) => content(&json),
        Ok(other) => scalar(&other),
        Err(_) => String::new(),
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
