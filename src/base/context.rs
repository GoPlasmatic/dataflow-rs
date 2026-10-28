//! The context, `{data, metadata, temp_data}`, on v3's wire shape, and the
//! reader every value of a wire shape is read back with.
//!
//! The context's reads and writes over a [`Path`](crate::expr::Path) live in
//! `run`, beside the message, since `expr` sits above `base`.

use datavalue::{NumberValue, OwnedDataValue};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashSet;
use std::fmt;

/// The three roots, in the order a context holds them.
pub(crate) const ROOTS: [&str; 3] = ["data", "metadata", "temp_data"];

/// The context: always `data`, `metadata` and `temp_data`, in that order.
/// Each root starts as an object, and every write keeps it one: `map`,
/// `Ctx::set` and [`Context::set`] refuse anything else at a root.
///
/// Read from JSON, it takes the three roots in any order and writes them in
/// that order, reads a missing one as `{}`, takes any value at each (a
/// trace's redacted root is `null`), and refuses any other key,
/// since expressions evaluate against the whole object and a fourth key
/// would be readable by `{"var": …}`. An object anywhere in it holding a key
/// twice is refused too: the engine reads the first, where a JSON parser
/// keeps the last, so the two would disagree on what the context says.
#[derive(Clone, Debug, PartialEq)]
pub struct Context {
    pub(crate) root: OwnedDataValue,
}

/// A seed for a root: an object as it is, anything else `{}` with capacity
/// `cap`.
fn slot(seed: OwnedDataValue, cap: usize) -> OwnedDataValue {
    if seed.is_object() {
        seed
    } else {
        OwnedDataValue::Object(Vec::with_capacity(cap))
    }
}

impl Context {
    /// A context seeded with `data` and `metadata`, each ignored when it is
    /// not an object, and an empty `temp_data`. `metadata` is sized for the
    /// engine's run stamps and progress object so the first run does not
    /// reallocate it.
    pub(crate) fn new(data: OwnedDataValue, metadata: OwnedDataValue) -> Self {
        Self::from_roots([
            slot(data, 0),
            slot(metadata, 4),
            OwnedDataValue::Object(Vec::new()),
        ])
    }

    /// A context over `root`, the object holding the three roots: a trace's
    /// `initial` and each context its replay rebuilds.
    pub(crate) fn from_root(root: OwnedDataValue) -> Self {
        Self { root }
    }

    /// A context over the three roots' values, in [`ROOTS`]' order. A
    /// literal, not an iterator over `ROOTS`: every message is built through
    /// it, and the zipped iterator cost about 0.5% of `micro_cond_bench`'s
    /// instructions.
    fn from_roots([data, metadata, temp_data]: [OwnedDataValue; 3]) -> Self {
        let [d, m, t] = ROOTS;
        Self::from_root(OwnedDataValue::Object(vec![
            (d.to_owned(), data),
            (m.to_owned(), metadata),
            (t.to_owned(), temp_data),
        ]))
    }

    fn root(&self, key: &str) -> &OwnedDataValue {
        &self.root[key]
    }

    /// The `data` root.
    pub fn data(&self) -> &OwnedDataValue {
        self.root("data")
    }

    /// The `metadata` root.
    pub fn metadata(&self) -> &OwnedDataValue {
        self.root("metadata")
    }

    /// The `temp_data` root.
    pub fn temp_data(&self) -> &OwnedDataValue {
        self.root("temp_data")
    }

    /// The object holding the three roots: what expressions evaluate against.
    pub fn as_value(&self) -> &OwnedDataValue {
        &self.root
    }

    /// Takes the `data` root, leaving an empty object where it was so the
    /// context keeps its three roots and still serializes as one. `metadata`
    /// and `temp_data` are untouched.
    ///
    /// The tree is moved, not cloned: its buffers, key order and numeric
    /// representation are the ones it had, so a completed child run's result
    /// can cross into its parent without a deep copy. A second take returns
    /// the empty object the first left.
    ///
    /// A root that is not an object (a trace's redacted `null`) is returned
    /// as it is and replaced with `{}`, as every root starts as an object.
    #[must_use = "the data root is taken and left empty"]
    pub fn take_data(&mut self) -> OwnedDataValue {
        let OwnedDataValue::Object(entries) = &mut self.root else {
            return OwnedDataValue::Object(Vec::new());
        };
        let Some((_, data)) = entries.iter_mut().find(|(key, _)| key.as_str() == "data") else {
            return OwnedDataValue::Object(Vec::new());
        };
        std::mem::replace(data, OwnedDataValue::Object(Vec::new()))
    }
}

impl Serialize for Context {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.root.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Context {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_map(Roots)
    }
}

#[cfg(feature = "ts")]
crate::base::ts::ts_decl!(
    Context,
    "Context",
    [
        " The context: `data`, `metadata` and `temp_data`, in that order. Each",
        " root starts as an object; a trace's redacted root is `null`.",
    ],
    |_cfg| {
        use crate::base::ts::{Prop, object};
        object(&[
            Prop::new("data", "JsonValue", &[" The `data` root."]),
            Prop::new("metadata", "JsonValue", &[" The `metadata` root."]),
            Prop::new("temp_data", "JsonValue", &[" The `temp_data` root."]),
        ])
    }
);

/// [`Context`]'s reader: the three roots, by name, once each.
struct Roots;

impl<'de> Visitor<'de> for Roots {
    type Value = Context;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a context: an object holding `data`, `metadata` and `temp_data`")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Context, A::Error> {
        let mut values: [Option<OwnedDataValue>; 3] = [None, None, None];
        while let Some(key) = map.next_key::<String>()? {
            let Some(at) = ROOTS.iter().position(|root| *root == key) else {
                return Err(de::Error::custom(format_args!(
                    "a context holds only `data`, `metadata` and `temp_data`, not `{key}`"
                )));
            };
            if values[at].is_some() {
                return Err(de::Error::duplicate_field(ROOTS[at]));
            }
            values[at] = Some(map.next_value::<ReadValue<false>>()?.0);
        }
        Ok(Context::from_roots(values.map(|value| {
            value.unwrap_or_else(|| OwnedDataValue::Object(Vec::new()))
        })))
    }
}

/// A value read back as v4 wrote it: as [`OwnedDataValue`]'s own reader
/// reads it, except that a number written with a fraction or an exponent
/// stays a float, as `OwnedDataValue::from(&serde_json::Value)` keeps one,
/// and an object holding a key twice is refused. That reader makes `2.0` the
/// integer `2`, which writes back as `2`, so a message holding a
/// whole-number float would not read back byte for byte; and it keeps both
/// pairs of a repeated key, which every path rule reads as one.
pub(crate) fn exact<'de, D: Deserializer<'de>>(d: D) -> Result<OwnedDataValue, D::Error> {
    d.deserialize_any(ValueVisitor::<false>)
}

/// A value read in document order, folding any repeated object key to its
/// first position and last value (as `serde_json::Value` with `preserve_order`
/// reads one).
pub(crate) fn in_order<'de, D: Deserializer<'de>>(d: D) -> Result<OwnedDataValue, D::Error> {
    d.deserialize_any(ValueVisitor::<true>)
}

/// A value read by [`ValueVisitor`].
struct ReadValue<const DEDUP: bool>(OwnedDataValue);

impl<'de, const DEDUP: bool> Deserialize<'de> for ReadValue<DEDUP> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(ValueVisitor::<DEDUP>).map(Self)
    }
}

/// [`exact`]'s and [`in_order`]'s reader: `OwnedDataValue`'s, keeping floats
/// as floats and either refusing (`DEDUP = false`) or folding (`DEDUP = true`)
/// repeated object keys.
struct ValueVisitor<const DEDUP: bool>;

impl<'de, const DEDUP: bool> Visitor<'de> for ValueVisitor<DEDUP> {
    type Value = OwnedDataValue;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any valid JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<OwnedDataValue, E> {
        Ok(OwnedDataValue::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<OwnedDataValue, E> {
        Ok(OwnedDataValue::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<OwnedDataValue, D::Error> {
        d.deserialize_any(Self)
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<OwnedDataValue, E> {
        Ok(OwnedDataValue::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<OwnedDataValue, E> {
        Ok(OwnedDataValue::Number(NumberValue::Integer(v)))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<OwnedDataValue, E> {
        Ok(OwnedDataValue::Number(NumberValue::from_u64(v)))
    }

    fn visit_i128<E: de::Error>(self, v: i128) -> Result<OwnedDataValue, E> {
        Ok(OwnedDataValue::Number(match i64::try_from(v) {
            Ok(i) => NumberValue::Integer(i),
            Err(_) => NumberValue::Float(v as f64),
        }))
    }

    fn visit_u128<E: de::Error>(self, v: u128) -> Result<OwnedDataValue, E> {
        Ok(OwnedDataValue::Number(match i64::try_from(v) {
            Ok(i) => NumberValue::Integer(i),
            Err(_) => NumberValue::Float(v as f64),
        }))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<OwnedDataValue, E> {
        Ok(if v.is_finite() {
            OwnedDataValue::Number(NumberValue::Float(v))
        } else {
            OwnedDataValue::Null
        })
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<OwnedDataValue, E> {
        Ok(OwnedDataValue::String(v.to_owned()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<OwnedDataValue, E> {
        Ok(OwnedDataValue::String(v))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<OwnedDataValue, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(ReadValue(item)) = seq.next_element::<ReadValue<DEDUP>>()? {
            items.push(item);
        }
        Ok(OwnedDataValue::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<OwnedDataValue, A::Error> {
        let mut pairs = Vec::with_capacity(map.size_hint().unwrap_or(0));
        while let Some((key, ReadValue(value))) = map.next_entry::<String, ReadValue<DEDUP>>()? {
            pairs.push((key, value));
        }
        if DEDUP {
            dedup_object(&mut pairs);
        } else if let Some(key) = repeated_key(&pairs) {
            return Err(de::Error::custom(format_args!(
                "an object holds the key `{key}` twice"
            )));
        }
        Ok(OwnedDataValue::Object(pairs))
    }
}

/// Above this many keys, an object finds a repeated key through a set
/// rather than a scan.
const SCAN: usize = 8;

/// The first key `pairs` holds a second time, if any.
fn repeated_key(pairs: &[(String, OwnedDataValue)]) -> Option<&str> {
    if pairs.len() <= SCAN {
        return pairs
            .iter()
            .enumerate()
            .find(|(i, (key, _))| pairs[..*i].iter().any(|(k, _)| k == key))
            .map(|(_, (key, _))| key.as_str());
    }
    let mut seen = HashSet::with_capacity(pairs.len());
    pairs
        .iter()
        .map(|(key, _)| key.as_str())
        .find(|key| !seen.insert(*key))
}

/// Ensures `pairs` holds each key once: a repeated key keeps its first
/// position and its last value, as `serde_json::Value` reads one.
fn dedup_object(pairs: &mut Vec<(String, OwnedDataValue)>) {
    if repeated_key(pairs).is_some() {
        fold_repeats(pairs);
    }
}

/// `value` with every object in it holding each key once: a repeated key
/// keeps its first position and its last value, as `serde_json::Value`
/// reads one.
pub(crate) fn keep_last(value: &mut OwnedDataValue) {
    match value {
        OwnedDataValue::Object(pairs) => {
            dedup_object(pairs);
            pairs.iter_mut().for_each(|(_, v)| keep_last(v));
        }
        OwnedDataValue::Array(items) => items.iter_mut().for_each(keep_last),
        _ => {}
    }
}

/// [`keep_last`] on one object known to hold a key twice.
#[cold]
fn fold_repeats(pairs: &mut Vec<(String, OwnedDataValue)>) {
    let mut len = 0;
    for i in 0..pairs.len() {
        let (key, value) = std::mem::take(&mut pairs[i]);
        if let Some((_, slot)) = pairs[..len].iter_mut().find(|(k, _)| *k == key) {
            *slot = value;
        } else {
            pairs[len] = (key, value);
            len += 1;
        }
    }
    pairs.truncate(len);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(c: &Context) -> String {
        serde_json::to_string(c).unwrap()
    }

    #[test]
    fn a_seed_that_is_not_an_object_is_ignored() {
        let c = Context::new(
            OwnedDataValue::from(&json!([1])),
            OwnedDataValue::from(&json!({"m": 1})),
        );
        assert_eq!(text(&c), r#"{"data":{},"metadata":{"m":1},"temp_data":{}}"#);
    }

    #[test]
    fn a_context_reads_what_it_writes() {
        let wire = r#"{"data":{"b":1,"a":[null]},"metadata":null,"temp_data":"t"}"#;
        let c: Context = serde_json::from_str(wire).unwrap();
        assert_eq!(text(&c), wire);
        assert_eq!(c.metadata(), &OwnedDataValue::Null);
        assert_eq!(
            c,
            Context::from_root(OwnedDataValue::from(&json!({
                "data": {"b": 1, "a": [null]}, "metadata": null, "temp_data": "t"
            })))
        );
    }

    #[test]
    fn a_value_reads_back_as_written_and_as_the_json_bridge_builds_it() {
        let wire =
            r#"[2.0,-3.0,2.5,1e+300,7,-7,9223372036854775807,"s",true,null,{"b":[1.0],"a":{}}]"#;
        let mut de = serde_json::Deserializer::from_str(wire);
        let read = exact(&mut de).unwrap();
        assert_eq!(serde_json::to_string(&read).unwrap(), wire);
        let bridged: serde_json::Value = serde_json::from_str(wire).unwrap();
        assert_eq!(read, OwnedDataValue::from(&bridged));
        // `OwnedDataValue`'s own reader makes the whole floats integers.
        let collapsed: OwnedDataValue = serde_json::from_str(wire).unwrap();
        assert!(
            serde_json::to_string(&collapsed)
                .unwrap()
                .starts_with("[2,-3,2.5,")
        );
        // Above `i64::MAX`, an integer is a float, as the bridge makes it.
        let big = exact(&mut serde_json::Deserializer::from_str(
            "18446744073709551615",
        ))
        .unwrap();
        assert_eq!(big, OwnedDataValue::from(&json!(u64::MAX)));
    }
    #[test]
    fn a_repeated_key_keeps_its_first_position_and_its_last_value() {
        let fold = |text: &str| {
            let mut value = OwnedDataValue::from_json(text).unwrap();
            keep_last(&mut value);
            serde_json::to_string(&value).unwrap()
        };
        assert_eq!(
            fold(r#"{"a":1,"b":2,"a":3,"c":4,"b":5}"#),
            r#"{"a":3,"b":5,"c":4}"#
        );
        // Inside arrays and objects, and inside a repeated key's last value.
        assert_eq!(
            fold(r#"[{"x":{"y":1,"y":2}},{"a":{"q":1},"a":{"q":2,"q":3}}]"#),
            r#"[{"x":{"y":2}},{"a":{"q":3}}]"#
        );
        // Past the scan, through the set.
        let wide: String = (0..12).map(|i| format!(r#""k{i}":{i},"#)).collect();
        let wanted: String = (0..12)
            .map(|i| format!(r#""k{i}":{},"#, if i == 3 { 99 } else { i }))
            .collect();
        assert_eq!(
            fold(&format!(r#"{{{wide}"k3":99}}"#)),
            format!("{{{}}}", wanted.trim_end_matches(','))
        );
        // Nothing repeated, nothing moves.
        let plain = r#"{"b":1,"a":{"b":2},"c":[{"b":3}]}"#;
        assert_eq!(fold(plain), plain);
    }

    /// The buffer of `value`'s `a.b` array, when it has one.
    fn array_buffer(value: &OwnedDataValue) -> *const OwnedDataValue {
        let OwnedDataValue::Object(entries) = value else {
            return std::ptr::null();
        };
        let Some((_, OwnedDataValue::Object(inner))) = entries.iter().find(|(key, _)| key == "a")
        else {
            return std::ptr::null();
        };
        let Some((_, OwnedDataValue::Array(items))) = inner.iter().find(|(key, _)| key == "b")
        else {
            return std::ptr::null();
        };
        items.as_ptr()
    }

    #[test]
    fn taking_the_data_root_moves_the_tree_and_leaves_an_object() {
        let data = json!({"a": {"b": [1, {"c": 2.0}]}, "big": 12345678901234i64});
        let mut c = Context::from_root(OwnedDataValue::from(&json!({
            "data": data, "metadata": {"m": 1}, "temp_data": {"t": true}
        })));
        let buffer = array_buffer(c.data());
        assert!(!buffer.is_null());
        let taken = c.take_data();
        // The value is the one that was there, its buffer included: the tree
        // moved, and nothing was copied.
        assert_eq!(taken, OwnedDataValue::from(&data));
        assert_eq!(array_buffer(&taken), buffer);
        assert_eq!(
            serde_json::to_string(&taken).unwrap(),
            r#"{"a":{"b":[1,{"c":2.0}]},"big":12345678901234}"#
        );
        // The context keeps its three roots, `data` as an object, and the
        // other two as they were.
        assert_eq!(
            text(&c),
            r#"{"data":{},"metadata":{"m":1},"temp_data":{"t":true}}"#
        );
    }

    #[test]
    fn a_second_take_returns_the_empty_object_the_first_left() {
        let mut c = Context::new(
            OwnedDataValue::from(&json!({"a": 1})),
            OwnedDataValue::from(&json!({"m": 1})),
        );
        assert_eq!(c.take_data(), OwnedDataValue::from(&json!({"a": 1})));
        assert_eq!(c.take_data(), OwnedDataValue::Object(Vec::new()));
        assert_eq!(text(&c), r#"{"data":{},"metadata":{"m":1},"temp_data":{}}"#);
    }

    #[test]
    fn an_empty_data_root_is_taken_as_an_empty_object() {
        let mut c = Context::new(OwnedDataValue::Object(Vec::new()), OwnedDataValue::Null);
        assert_eq!(c.take_data(), OwnedDataValue::Object(Vec::new()));
        assert_eq!(text(&c), r#"{"data":{},"metadata":{},"temp_data":{}}"#);
    }

    #[test]
    fn a_redacted_root_is_taken_as_it_is_and_replaced_with_an_object() {
        let mut c = Context::from_root(OwnedDataValue::from(&json!({
            "data": null, "metadata": null, "temp_data": null
        })));
        assert_eq!(c.take_data(), OwnedDataValue::Null);
        assert_eq!(text(&c), r#"{"data":{},"metadata":null,"temp_data":null}"#);
    }
}
