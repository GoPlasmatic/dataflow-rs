//! XML in and out for `parse_xml` and `publish_xml`: v3's `xml_to_json` and
//! its writer, text for text.
//!
//! An object's keys keep the order the value holds, in both directions: a
//! document's elements read in document order, and a value writes its keys in
//! its own order, in every build. v3 went through [`serde_json::Value`], whose
//! keys its default build sorted; v4 reads and writes `OwnedDataValue`.

use crate::base::{Error, Result, VALIDATION_ERROR, dedup_object};
use datavalue::{NumberValue, OwnedDataValue};
use serde::Deserializer;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use std::borrow::Cow;
use std::fmt;

/// A `VALIDATION_ERROR`, with the `Validation error: ` prefix v3's display
/// adds, so the record's text stays v3's.
pub(super) fn validation_error(message: impl std::fmt::Display) -> Error {
    Error::service(VALIDATION_ERROR, format!("Validation error: {message}"))
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
        let mut pairs = Vec::with_capacity(map.size_hint().unwrap_or(0));
        while let Some((key, Read(value))) = map.next_entry()? {
            pairs.push((key, value));
        }
        dedup_object(&mut pairs);
        Ok(OwnedDataValue::Object(pairs))
    }
}

/// `value` as an XML document under `root_element`, its keys in the value's
/// order. The root element's name is made an XML name as a key's is: it may
/// be computed from the message, and written raw it could close itself and
/// open elements of the message's choosing (3.x wrote it raw).
pub(super) fn value_to_xml(value: &OwnedDataValue, root_element: &str) -> String {
    let root_element = sanitize_name(root_element);
    let root_element = &*root_element;
    let mut out = String::new();
    tag(&mut out, "<", root_element);
    content(value, &mut out);
    tag(&mut out, "</", root_element);
    out
}

/// `{open}{name}>`: an element's opening or closing tag.
fn tag(out: &mut String, open: &str, name: &str) {
    out.push_str(open);
    out.push_str(name);
    out.push('>');
}

/// The inside of an element holding `value`, onto `out`.
fn content(value: &OwnedDataValue, out: &mut String) {
    match value {
        OwnedDataValue::Object(pairs) => {
            for (key, val) in pairs {
                let key = sanitize_name(key);
                tag(out, "<", &key);
                content(val, out);
                tag(out, "</", &key);
            }
        }
        OwnedDataValue::Array(items) => {
            for item in items {
                out.push_str("<item>");
                content(item, out);
                out.push_str("</item>");
            }
        }
        _ => scalar(value, out),
    }
}

/// A scalar's text, as `serde_json::Value` writes it, onto `out`; `null` and
/// a non-finite float are empty.
fn scalar(value: &OwnedDataValue, out: &mut String) {
    use std::fmt::Write as _;
    match value {
        OwnedDataValue::Null => {}
        OwnedDataValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        OwnedDataValue::Number(NumberValue::Integer(i)) => {
            let _ = write!(out, "{i}");
        }
        OwnedDataValue::Number(NumberValue::Float(f)) => {
            if let Some(n) = serde_json::Number::from_f64(*f) {
                let _ = write!(out, "{n}");
            }
        }
        OwnedDataValue::String(s) => escape(s, out),
        // An object or an array is no scalar: `content` walks those.
        OwnedDataValue::Object(_) | OwnedDataValue::Array(_) => {}
        #[allow(
            unreachable_patterns,
            reason = "the catch-all is live only with datavalue's `datetime` or `tensor` \
                      feature, which feature unification can turn on from another crate"
        )]
        other => foreign(other, out),
    }
}

/// A value JSON has no type for (a datetime, a duration, a tensor), in its
/// JSON form, onto `out`: a datetime's and a duration's text, a tensor's
/// tagged object walked in the order its `Serialize` writes it.
#[cold]
fn foreign(value: &OwnedDataValue, out: &mut String) {
    let text = serde_json::to_string(value).unwrap_or_default();
    match crate::base::exact(&mut serde_json::Deserializer::from_str(&text)) {
        Ok(OwnedDataValue::String(s)) => escape(&s, out),
        Ok(other) => content(&other, out),
        Err(_) => {}
    }
}

/// `s` with XML's five special characters escaped, onto `out`.
fn escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
}

/// An element name made of what XML allows, as v3's `sanitize_xml_name`:
/// borrowed when `name` is one already, as nearly every key is.
fn sanitize_name(name: &str) -> Cow<'_, str> {
    let mut chars = name.chars();
    let valid = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
    if valid {
        return Cow::Borrowed(name);
    }
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
    Cow::Owned(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(root: &str) -> String {
        let value = OwnedDataValue::from_json(r#"{"a":1}"#).expect("JSON");
        value_to_xml(&value, root)
    }

    #[test]
    fn a_root_element_is_made_an_xml_name_as_a_key_is() {
        assert_eq!(doc("envelope"), "<envelope><a>1</a></envelope>");
        // A computed root cannot close itself and open elements of its own.
        assert_eq!(
            doc("x><admin>true</admin><x"),
            "<x__admin_true__admin__x><a>1</a></x__admin_true__admin__x>"
        );
        assert_eq!(doc(""), "<_element><a>1</a></_element>");
        assert_eq!(doc("a b"), "<a_b><a>1</a></a_b>");
        assert_eq!(doc("1st"), "<_1st><a>1</a></_1st>");
    }

    #[test]
    fn a_valid_name_is_borrowed_and_any_other_rewritten() {
        for name in ["root", "_x", "a-b.c_9", "Z"] {
            assert!(
                matches!(sanitize_name(name), Cow::Borrowed(n) if n == name),
                "{name}"
            );
        }
        for (name, made) in [
            ("", "_element"),
            ("9", "_9"),
            ("-a", "_a"),
            ("a/b", "a_b"),
            ("é", "_"),
        ] {
            assert_eq!(sanitize_name(name), made, "{name}");
        }
    }
}
