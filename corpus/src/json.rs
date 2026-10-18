//! An order-preserving JSON value.
//!
//! Key order is wire data: the JSON a message serializes to keeps its keys in
//! the order they were written, and a removal changes that order. The corpus
//! keeps objects as ordered pairs, so its equality is order-sensitive whatever
//! map type serde_json is built with (`dataflow-rs` turns on its
//! `preserve_order`), and a removal's effect on order is its own model's.

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::Number;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Self>),
    Object(Vec<(String, Self)>),
}

impl Json {
    pub fn from_text(text: &str) -> serde_json::Result<Self> {
        serde_json::from_str(text)
    }

    /// Compact text, keys in order.
    pub fn to_text(&self) -> String {
        serde_json::to_string(self).expect("a Json always serializes")
    }

    /// Two-space indented text with a trailing newline, as recordings are stored.
    pub fn to_pretty(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("a Json always serializes");
        text.push('\n');
        text
    }

    pub fn object() -> Self {
        Self::Object(Vec::new())
    }

    pub fn str(s: impl Into<String>) -> Self {
        Self::String(s.into())
    }

    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Self> {
        match self {
            Self::Object(pairs) => pairs.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Sets `key`, in place when present and appended when not.
    pub fn insert(&mut self, key: impl Into<String>, value: Self) {
        let key = key.into();
        if let Self::Object(pairs) = self {
            match pairs.iter_mut().find(|(k, _)| *k == key) {
                Some((_, slot)) => *slot = value,
                None => pairs.push((key, value)),
            }
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Self>> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Self)]> {
        match self {
            Self::Object(pairs) => Some(pairs),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// RFC 6901: `/a/0/b`, with `~1` for `/` and `~0` for `~`.
    pub fn pointer(&self, pointer: &str) -> Option<&Self> {
        if pointer.is_empty() {
            return Some(self);
        }
        let mut at = self;
        for token in pointer.strip_prefix('/')?.split('/') {
            let token = unescape(token);
            at = match at {
                Self::Object(_) => at.get(&token)?,
                Self::Array(items) => items.get(token.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(at)
    }

    pub fn pointer_mut(&mut self, pointer: &str) -> Option<&mut Self> {
        if pointer.is_empty() {
            return Some(self);
        }
        let mut at = self;
        for token in pointer.strip_prefix('/')?.split('/') {
            let token = unescape(token);
            at = match at {
                Self::Object(_) => at.get_mut(&token)?,
                Self::Array(items) => items.get_mut(token.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(at)
    }
}

pub(crate) fn unescape(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

impl From<&serde_json::Value> for Json {
    fn from(v: &serde_json::Value) -> Self {
        use serde_json::Value;
        match v {
            Value::Null => Self::Null,
            Value::Bool(b) => Self::Bool(*b),
            Value::Number(n) => Self::Number(n.clone()),
            Value::String(s) => Self::String(s.clone()),
            Value::Array(items) => Self::Array(items.iter().map(Self::from).collect()),
            Value::Object(map) => Self::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), Self::from(v)))
                    .collect(),
            ),
        }
    }
}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => s.serialize_unit(),
            Self::Bool(b) => s.serialize_bool(*b),
            Self::Number(n) => n.serialize(s),
            Self::String(v) => s.serialize_str(v),
            Self::Array(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Self::Object(pairs) => {
                let mut map = s.serialize_map(Some(pairs.len()))?;
                for (k, v) in pairs {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(JsonVisitor)
    }
}

struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_unit<E>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_none<E>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Json, D::Error> {
        Json::deserialize(d)
    }

    fn visit_bool<E>(self, b: bool) -> Result<Json, E> {
        Ok(Json::Bool(b))
    }

    fn visit_i64<E>(self, n: i64) -> Result<Json, E> {
        Ok(Json::Number(n.into()))
    }

    fn visit_u64<E>(self, n: u64) -> Result<Json, E> {
        Ok(Json::Number(n.into()))
    }

    fn visit_f64<E: de::Error>(self, n: f64) -> Result<Json, E> {
        Number::from_f64(n)
            .map(Json::Number)
            .ok_or_else(|| E::custom(format!("{n} is not a JSON number")))
    }

    fn visit_str<E>(self, s: &str) -> Result<Json, E> {
        Ok(Json::String(s.to_owned()))
    }

    fn visit_string<E>(self, s: String) -> Result<Json, E> {
        Ok(Json::String(s))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Json::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
        let mut pairs: Vec<(String, Json)> = Vec::with_capacity(map.size_hint().unwrap_or(0));
        while let Some((key, value)) = map.next_entry::<String, Json>()? {
            // A repeated key keeps its first position and its last value, as
            // serde_json's own map does.
            match pairs.iter_mut().find(|(k, _)| *k == key) {
                Some((_, slot)) => *slot = value,
                None => pairs.push((key, value)),
            }
        }
        Ok(Json::Object(pairs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_order_survives_a_round_trip() {
        let text = r#"{"b":1,"a":{"z":[1,2.5,"x",null,true],"y":{}}}"#;
        assert_eq!(Json::from_text(text).unwrap().to_text(), text);
    }

    #[test]
    fn order_is_part_of_equality() {
        assert_ne!(
            Json::from_text(r#"{"a":1,"b":2}"#).unwrap(),
            Json::from_text(r#"{"b":2,"a":1}"#).unwrap()
        );
    }

    #[test]
    fn a_pointer_walks_objects_and_arrays_and_unescapes() {
        let j = Json::from_text(r#"{"a/b":[{"c~d":1}]}"#).unwrap();
        assert_eq!(j.pointer("/a~1b/0/c~0d"), Some(&Json::Number(1.into())));
        assert_eq!(j.pointer(""), Some(&j));
        assert_eq!(j.pointer("/missing"), None);
    }

    #[test]
    fn negative_and_fractional_numbers_round_trip() {
        let text = r#"[-3,0.1,18446744073709551615,1e300]"#;
        let j = Json::from_text(text).unwrap();
        assert_eq!(Json::from_text(&j.to_text()).unwrap(), j);
    }
}
