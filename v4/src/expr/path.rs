//! Paths into a context: a root plus v3's segment rules.
//!
//! A segment means one thing on an object and another on an array: on an
//! object it is a key, with one leading `#` stripped; on an array it is an
//! index, parsed from the raw segment, so `#20` is never an index. Both
//! readings are resolved at parse time, so no write re-parses.
//!
//! The write rules are v3's `set_nested_value`, and `tests/path.rs` checks
//! them against it. A write either applies in full or changes nothing:
//! containers are created only past the last existing one, and a hop that
//! cannot continue (a scalar, a `null` under an object, a key on an array)
//! comes before any creation.

use crate::base::{Error, ErrorKind, Result};
use datavalue::OwnedDataValue;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

/// The most segments a [`Path`] holds, the root's own included (I74). The
/// write log and the splice keep a path's resolved hops in a 64-bit mask.
pub(crate) const MAX_PATH_SEGMENTS: usize = 64;

/// One of the three context roots a path starts at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Root {
    /// The message's data.
    Data,
    /// The message's metadata.
    Metadata,
    /// The message's scratch space.
    TempData,
}

impl Root {
    /// The root's key in the context, as a path spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Metadata => "metadata",
            Self::TempData => "temp_data",
        }
    }
}

/// One path segment: its object key and, when the raw segment parses as one,
/// its array index.
///
/// Built only through its constructors, which compute `hash` from `key`, so
/// the derived equality stays consistent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Seg {
    /// The key on an object: the raw segment with one leading `#` stripped.
    pub(crate) key: Box<str>,
    /// The index on an array: the raw segment parsed as `usize`.
    pub(crate) index: Option<usize>,
    /// [`key_hash`] of `key`, computed once, so the write log rejects a
    /// sibling of another name with one compare. Two keys may share a hash,
    /// so a match is always confirmed on the names.
    pub(crate) hash: u32,
}

/// FNV-1a (32-bit) over a key's bytes: [`Seg::hash`].
pub(crate) fn key_hash(key: &str) -> u32 {
    key.bytes().fold(0x811c_9dc5, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

impl Seg {
    pub(crate) fn parse(raw: &str) -> Self {
        let key = seg_key(raw);
        Self {
            key: key.into(),
            index: seg_index(raw),
            hash: key_hash(key),
        }
    }

    /// A segment that is always an object key, never an index.
    pub(crate) fn key(key: &str) -> Self {
        Self {
            key: key.into(),
            index: None,
            hash: key_hash(key),
        }
    }

    /// The segment naming array element `i`: its key is `i`'s text.
    #[cfg(test)]
    pub(crate) fn index(i: usize) -> Self {
        let key = i.to_string();
        Self {
            hash: key_hash(&key),
            key: key.into(),
            index: Some(i),
        }
    }
}

/// A raw segment's key on an object: one leading `#` stripped.
fn seg_key(raw: &str) -> &str {
    raw.strip_prefix('#').unwrap_or(raw)
}

/// A raw segment's index on an array: the raw segment parsed as `usize`, so
/// `#20` is never one.
fn seg_index(raw: &str) -> Option<usize> {
    raw.parse().ok()
}

/// A root plus typed segments, parsed once. A segment is a key on an object
/// (one leading `#` stripped) and an index on an array (the raw segment parsed
/// as `usize`), resolved at parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Path {
    root: Root,
    /// The root's own segment first, then the segments below it: the chain the
    /// functions below run over the context.
    segs: Box<[Seg]>,
    dotted: Arc<str>,
}

impl FromStr for Path {
    type Err = Error;

    fn from_str(dotted: &str) -> Result<Self> {
        let (first, rest) = match dotted.split_once('.') {
            Some((first, rest)) => (first, Some(rest)),
            None => (dotted, None),
        };
        let root = match first {
            "data" => Root::Data,
            "metadata" => Root::Metadata,
            "temp_data" => Root::TempData,
            _ => {
                return Err(Error::new(
                    ErrorKind::Config,
                    format!("path `{dotted}` must start at `data`, `metadata` or `temp_data`"),
                ));
            }
        };
        let mut segs = vec![Seg::parse(first)];
        if let Some(rest) = rest {
            segs.extend(rest.split('.').map(Seg::parse));
        }
        if segs.len() > MAX_PATH_SEGMENTS {
            return Err(Error::new(
                ErrorKind::Config,
                format!(
                    "path `{dotted}` has {} segments; a path holds at most {MAX_PATH_SEGMENTS}",
                    segs.len()
                ),
            ));
        }
        Ok(Self {
            root,
            segs: segs.into(),
            dotted: Arc::from(dotted),
        })
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.dotted)
    }
}

impl Path {
    /// The root the path starts at.
    pub fn root(&self) -> Root {
        self.root
    }

    /// Does the path name a root itself, with nothing below it?
    pub fn is_root(&self) -> bool {
        self.segs.len() == 1
    }

    /// The segments, the root's own first.
    pub(crate) fn segs(&self) -> &[Seg] {
        &self.segs
    }

    /// The dotted spelling, shared: what a `Change` records as its path.
    pub(crate) fn dotted(&self) -> &Arc<str> {
        &self.dotted
    }

    /// The value at the path in `context`, an object holding the three roots.
    pub fn get<'v>(&self, context: &'v OwnedDataValue) -> Option<&'v OwnedDataValue> {
        get(context, &self.segs)
    }

    /// Assigns `value` at the path. At a root it replaces the root and accepts
    /// only an object. A hop that cannot continue writes nothing and is `Ok`.
    pub fn set(&self, context: &mut OwnedDataValue, value: OwnedDataValue) -> Result<()> {
        if self.is_root() && !matches!(value, OwnedDataValue::Object(_)) {
            return Err(Error::new(
                ErrorKind::Config,
                format!(
                    "`{}` is a root and takes only an object, not {}",
                    self.dotted,
                    kind(&value)
                ),
            ));
        }
        self.assign(context, value);
        Ok(())
    }

    /// Removes the value at the path and returns it. A root cannot be removed.
    pub fn remove(&self, context: &mut OwnedDataValue) -> Result<Option<OwnedDataValue>> {
        if self.is_root() {
            return Err(Error::new(
                ErrorKind::Config,
                format!("`{}` is a root and cannot be removed", self.dotted),
            ));
        }
        Ok(remove(context, &self.segs))
    }

    /// Assigns `value` at the path, reporting whether it wrote.
    pub(crate) fn assign(&self, context: &mut OwnedDataValue, value: OwnedDataValue) -> bool {
        set(context, &self.segs, value)
    }

    /// `map`'s append and extend at the path: the tests' owned reference for
    /// the splice's.
    #[cfg(test)]
    pub(crate) fn append(
        &self,
        context: &mut OwnedDataValue,
        items: Vec<OwnedDataValue>,
    ) -> std::result::Result<bool, &'static str> {
        append(context, &self.segs, items)
    }
}

/// The empty container a missing hop creates: an array when the next segment
/// is an index, an object otherwise.
fn empty(next: &Seg) -> OwnedDataValue {
    if next.index.is_some() {
        OwnedDataValue::Array(Vec::new())
    } else {
        OwnedDataValue::Object(Vec::new())
    }
}

/// What writing `value` at `segs` into a new, empty container produces: the
/// owned twin of the splice's chain, for a write marked where it starts
/// creating.
pub(crate) fn chain(segs: &[Seg], value: OwnedDataValue) -> OwnedDataValue {
    let Some(first) = segs.first() else {
        return value;
    };
    let mut container = empty(first);
    set(&mut container, segs, value);
    container
}

/// One step of a read: `key` on an object, `index` on an array.
fn hop<'v>(v: &'v OwnedDataValue, key: &str, index: Option<usize>) -> Option<&'v OwnedDataValue> {
    match v {
        OwnedDataValue::Object(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
        OwnedDataValue::Array(items) => items.get(index?),
        _ => None,
    }
}

pub(crate) fn get<'v>(v: &'v OwnedDataValue, segs: &[Seg]) -> Option<&'v OwnedDataValue> {
    segs.iter()
        .try_fold(v, |v, seg| hop(v, &seg.key, seg.index))
}

/// [`get`] over a dotted path read segment by segment, by the same rules,
/// allocating nothing: for a lookup that is not worth parsing once.
pub(crate) fn get_dotted<'v>(v: &'v OwnedDataValue, dotted: &str) -> Option<&'v OwnedDataValue> {
    dotted
        .split('.')
        .try_fold(v, |v, raw| hop(v, seg_key(raw), seg_index(raw)))
}

/// [`get`], mutably.
pub(crate) fn get_mut<'v>(
    mut v: &'v mut OwnedDataValue,
    segs: &[Seg],
) -> Option<&'v mut OwnedDataValue> {
    for seg in segs {
        v = match v {
            OwnedDataValue::Object(pairs) => {
                &mut pairs.iter_mut().find(|(k, _)| **k == *seg.key)?.1
            }
            OwnedDataValue::Array(items) => items.get_mut(seg.index?)?,
            _ => return None,
        };
    }
    Some(v)
}

/// v3's `set_nested_value_parts`, returning whether it wrote.
pub(crate) fn set(v: &mut OwnedDataValue, segs: &[Seg], value: OwnedDataValue) -> bool {
    let Some((last, parents)) = segs.split_last() else {
        return false;
    };
    let mut current = v;
    for (i, seg) in parents.iter().enumerate() {
        let next = &segs[i + 1];
        current = match current {
            OwnedDataValue::Object(pairs) => {
                let at = match pairs.iter().position(|(k, _)| **k == *seg.key) {
                    Some(at) => at,
                    None => {
                        // `String::from`, not `to_string`: a `Box<str>`
                        // formats through `Display`.
                        pairs.push((String::from(&*seg.key), empty(next)));
                        pairs.len() - 1
                    }
                };
                &mut pairs[at].1
            }
            OwnedDataValue::Array(items) => {
                let Some(at) = seg.index else {
                    return false;
                };
                if items.len() <= at {
                    items.resize(at + 1, OwnedDataValue::Null);
                }
                if items[at].is_null() {
                    items[at] = empty(next);
                }
                &mut items[at]
            }
            _ => return false,
        };
    }
    match current {
        OwnedDataValue::Object(pairs) => {
            match pairs.iter_mut().find(|(k, _)| **k == *last.key) {
                Some(slot) => slot.1 = value,
                None => pairs.push((String::from(&*last.key), value)),
            }
            true
        }
        OwnedDataValue::Array(items) => {
            let Some(at) = last.index else {
                return false;
            };
            if items.len() <= at {
                items.resize(at + 1, OwnedDataValue::Null);
            }
            items[at] = value;
            true
        }
        _ => false,
    }
}

/// v3's `remove_nested_value_parts`: an object key leaves with the order of
/// the rest kept, an array element with the tail shifted down.
pub(crate) fn remove(v: &mut OwnedDataValue, segs: &[Seg]) -> Option<OwnedDataValue> {
    let (last, parents) = segs.split_last()?;
    let parent = get_mut(v, parents)?;
    match parent {
        OwnedDataValue::Object(pairs) => {
            let at = pairs.iter().position(|(k, _)| **k == *last.key)?;
            Some(pairs.remove(at).1)
        }
        OwnedDataValue::Array(items) => {
            let at = last.index?;
            (at < items.len()).then(|| items.remove(at))
        }
        _ => None,
    }
}

/// The kind of a value, for an error: v3's `describe_kind`.
pub(crate) fn kind(v: &OwnedDataValue) -> &'static str {
    match v {
        OwnedDataValue::Null => "null",
        OwnedDataValue::Bool(_) => "a boolean",
        OwnedDataValue::Number(_) => "a number",
        OwnedDataValue::String(_) => "a string",
        OwnedDataValue::Array(_) => "an array",
        OwnedDataValue::Object(_) => "an object",
        #[allow(
            unreachable_patterns,
            reason = "the catch-all is live only with datavalue's `datetime` or `tensor` \
                      feature, which feature unification can turn on from another crate"
        )]
        _ => "a non-array value",
    }
}

/// `map`'s append and extend (v3's `append_owned`): extend the array at
/// `segs`, or create it over a missing or `null` target. `Ok(false)` when
/// the path is unreachable, which v3 treats as success with nothing written;
/// `Err` names the kind of any other target.
pub(crate) fn append(
    v: &mut OwnedDataValue,
    segs: &[Seg],
    items: Vec<OwnedDataValue>,
) -> std::result::Result<bool, &'static str> {
    match get_mut(v, segs) {
        Some(OwnedDataValue::Array(existing)) => {
            existing.extend(items);
            Ok(true)
        }
        None | Some(OwnedDataValue::Null) => Ok(set(v, segs, OwnedDataValue::Array(items))),
        Some(other) => Err(kind(other)),
    }
}

/// `map`'s write to a root slot (v3's `merge_root_field`): two objects merge,
/// existing keys replaced in place and new keys appended in order; anything
/// else replaces the slot. The owned statement of the rules the splice's
/// `merge_root` follows: `map` writes a root only through the view.
#[cfg(test)]
pub(crate) fn merge_root(context: &mut OwnedDataValue, key: &str, value: OwnedDataValue) {
    let OwnedDataValue::Object(slots) = context else {
        *context = OwnedDataValue::Object(vec![(key.to_string(), value)]);
        return;
    };
    let Some(slot) = slots.iter_mut().find(|(k, _)| k == key) else {
        slots.push((key.to_string(), value));
        return;
    };
    match (&mut slot.1, value) {
        (OwnedDataValue::Object(existing), OwnedDataValue::Object(new_pairs)) => {
            for (k, v) in new_pairs {
                match existing.iter_mut().find(|(ek, _)| *ek == k) {
                    Some(s) => s.1 = v,
                    None => existing.push((k, v)),
                }
            }
        }
        (slot, value) => *slot = value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(text: &str) -> OwnedDataValue {
        serde_json::from_str(text).unwrap()
    }

    fn text(v: &OwnedDataValue) -> String {
        serde_json::to_string(v).unwrap()
    }

    fn path(s: &str) -> Path {
        s.parse().unwrap()
    }

    #[test]
    fn a_path_parses_to_its_root_and_displays_back() {
        let p = path("data.rows.#20.total");
        assert_eq!(p.root(), Root::Data);
        assert!(!p.is_root());
        assert_eq!(p.to_string(), "data.rows.#20.total");
        assert_eq!(path("temp_data.x").root(), Root::TempData);
        assert_eq!(path("metadata.x").root().as_str(), "metadata");
    }

    #[test]
    fn what_is_not_a_context_root_is_refused() {
        for bad in ["", "rows.x", "#data.x", "payload", "payload.x", "Data.x"] {
            let error = bad.parse::<Path>().unwrap_err();
            assert_eq!(error.kind(), &ErrorKind::Config, "{bad}");
        }
    }

    #[test]
    fn a_bare_root_is_a_root_path() {
        assert!(path("data").is_root());
        assert!(path("temp_data").is_root());
    }

    #[test]
    fn empty_segments_below_the_root_are_empty_keys() {
        let mut ctx = json(r#"{"data":{}}"#);
        path("data..x").set(&mut ctx, json("1")).unwrap();
        assert_eq!(text(&ctx), r#"{"data":{"":{"x":1}}}"#);
    }

    #[test]
    fn set_at_a_root_replaces_with_an_object_and_refuses_anything_else() {
        let mut ctx = json(r#"{"data":{"a":1},"metadata":{}}"#);
        path("data").set(&mut ctx, json(r#"{"b":2}"#)).unwrap();
        assert_eq!(text(&ctx), r#"{"data":{"b":2},"metadata":{}}"#);
        let error = path("data").set(&mut ctx, json(r#""s""#)).unwrap_err();
        assert_eq!(error.kind(), &ErrorKind::Config);
        assert_eq!(text(&ctx), r#"{"data":{"b":2},"metadata":{}}"#);
    }

    #[test]
    fn remove_at_a_root_is_refused() {
        let mut ctx = json(r#"{"data":{}}"#);
        let error = path("data").remove(&mut ctx).unwrap_err();
        assert_eq!(error.kind(), &ErrorKind::Config);
    }

    #[test]
    fn a_hop_that_cannot_continue_writes_nothing_and_is_ok() {
        let mut ctx = json(r#"{"data":{"a":5}}"#);
        path("data.a.b").set(&mut ctx, json("1")).unwrap();
        assert_eq!(text(&ctx), r#"{"data":{"a":5}}"#);
        assert!(!path("data.a.b").assign(&mut ctx, json("1")));
    }

    #[test]
    fn get_set_remove_work_below_a_root() {
        let mut ctx = json(r#"{"data":{"rows":[{"t":1}]},"metadata":{}}"#);
        assert_eq!(text(path("data.rows.0.t").get(&ctx).unwrap()), "1");
        path("temp_data.x.y").set(&mut ctx, json("2")).unwrap();
        assert_eq!(
            text(&ctx),
            r#"{"data":{"rows":[{"t":1}]},"metadata":{},"temp_data":{"x":{"y":2}}}"#
        );
        let gone = path("data.rows.0.t").remove(&mut ctx).unwrap();
        assert_eq!(gone.map(|v| text(&v)), Some("1".into()));
    }

    #[test]
    fn a_dotted_read_follows_the_parsed_segments_rules() {
        let v = json(r##"{"a":{"20":"key","#b":"x","b":"hash","rows":[{"t":1},{"t":2}]}}"##);
        for dotted in [
            "a.20",
            "a.#20",
            "a.b",
            "a.#b",
            "a.##b",
            "a.rows.1.t",
            "a.rows.#1",
            "a.rows.x",
            "a",
            "a.",
            "",
            "zz",
        ] {
            let segs: Vec<Seg> = dotted.split('.').map(Seg::parse).collect();
            assert_eq!(
                get_dotted(&v, dotted).map(text),
                get(&v, &segs).map(text),
                "{dotted}"
            );
        }
        assert_eq!(get_dotted(&v, "a.#b").map(text), Some(r#""hash""#.into()));
        assert_eq!(get_dotted(&v, "a.rows.#1"), None);
    }

    #[test]
    fn a_path_holds_at_most_64_segments() {
        let deep = |n: usize| format!("data{}", ".k".repeat(n - 1));
        let ok: Path = deep(64).parse().expect("64 segments parse");
        assert_eq!(ok.segs().len(), 64);
        let err = deep(65)
            .parse::<Path>()
            .expect_err("65 segments are refused");
        assert_eq!(*err.kind(), ErrorKind::Config);
        assert!(err.to_string().contains("at most 64"), "{err}");
    }

    #[test]
    fn append_extends_or_creates() {
        let mut ctx = json(r#"{"data":{"a":[1]}}"#);
        assert_eq!(path("data.a").append(&mut ctx, vec![json("2")]), Ok(true));
        assert_eq!(path("data.b").append(&mut ctx, vec![json("3")]), Ok(true));
        assert_eq!(text(&ctx), r#"{"data":{"a":[1,2],"b":[3]}}"#);
        path("data.s").set(&mut ctx, json(r#""x""#)).unwrap();
        assert_eq!(path("data.s").append(&mut ctx, vec![]), Err("a string"));
    }

    #[test]
    fn merge_root_merges_objects_and_replaces_the_rest() {
        let mut ctx = json(r#"{"data":{"a":1,"b":2}}"#);
        merge_root(&mut ctx, "data", json(r#"{"b":3,"c":4}"#));
        assert_eq!(text(&ctx), r#"{"data":{"a":1,"b":3,"c":4}}"#);
        merge_root(&mut ctx, "data", json("5"));
        merge_root(&mut ctx, "metadata", json("{}"));
        assert_eq!(text(&ctx), r#"{"data":5,"metadata":{}}"#);
    }

    #[test]
    fn chain_builds_the_containers_a_write_would_create() {
        let segs = &path("data.a.0").segs[1..];
        assert_eq!(text(&chain(segs, json("1"))), r#"{"a":[1]}"#);
        assert_eq!(text(&chain(&[], json("1"))), "1");
    }
}
