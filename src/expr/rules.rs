//! The path rules, stated once: what a write meets at each container on its
//! path ([`step`]), the append target's rule ([`append_to`]), a value's
//! shape and its name in a message ([`Shape`]), and the one-segment
//! primitives ([`put`], [`take`]) the write log's checkpoint applies.
//!
//! Four writers carry a write out. The overlay's landing check
//! (`function::overlay`) and the write log's checkpoint (`state::writes`)
//! ask these rules directly. The two hot walks, the owned tree's
//! (`path::set`, `get`, `remove`) and the arena view's splice
//! (`state::splice`), are hand-inlined forms of [`step`]: asking it per
//! segment cost every `map`-heavy benchmark 1.5 to 2% of its instructions.
//! The tests' reference walk, `resolve`, is [`step`] at every segment over
//! either tree, and `state::tests::splice_property` holds both fast walks to
//! it on every small tree and path, and to the corpus's independent model
//! (with `tests/it/path.rs`).
//!
//! The rules are v3's `set_nested_value`:
//!
//! - on an object a segment is a key: a missing one is created, a container
//!   is descended into, and a scalar or a `null` stops a deeper write;
//! - on an array a segment is an index, or it stops the write: a missing or
//!   `null` element is created (the array padded with `null` up to it), a
//!   container is descended into, and a scalar stops a deeper write;
//! - a scalar or a `null` holds nothing, so a write below it does nothing;
//! - what is created below a missing node is an array where the next
//!   segment is an index, and an object otherwise;
//! - at the last segment an existing child is replaced, whatever it is.
//!
//! Every rule reads the first matching key of an object, and a new key goes
//! at the end.

#[cfg(test)]
use super::path::MAX_PATH_SEGMENTS;
use super::path::Seg;
use datavalue::{DataValue, OwnedDataValue};

/// A value's shape, as far as the path rules and their messages care.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    Null,
    Bool,
    Number,
    String,
    Array,
    Object,
    /// A datavalue variant past JSON's (a datetime, a tensor), which only
    /// feature unification turns on.
    Other,
}

impl Shape {
    /// An owned value's shape.
    #[inline]
    pub(crate) fn of_owned(v: &OwnedDataValue) -> Self {
        match v {
            OwnedDataValue::Null => Self::Null,
            OwnedDataValue::Bool(_) => Self::Bool,
            OwnedDataValue::Number(_) => Self::Number,
            OwnedDataValue::String(_) => Self::String,
            OwnedDataValue::Array(_) => Self::Array,
            OwnedDataValue::Object(_) => Self::Object,
            #[allow(
                unreachable_patterns,
                reason = "the catch-all is live only with datavalue's `datetime` or `tensor` \
                          feature, which feature unification can turn on from another crate"
            )]
            _ => Self::Other,
        }
    }

    /// A view's shape.
    #[inline]
    pub(crate) fn of_view(v: &DataValue<'_>) -> Self {
        match v {
            DataValue::Null => Self::Null,
            DataValue::Bool(_) => Self::Bool,
            DataValue::Number(_) => Self::Number,
            DataValue::String(_) => Self::String,
            DataValue::Array(_) => Self::Array,
            DataValue::Object(_) => Self::Object,
            #[allow(
                unreachable_patterns,
                reason = "the catch-all is live only with datavalue's `datetime` or `tensor` \
                          feature, which feature unification can turn on from another crate"
            )]
            _ => Self::Other,
        }
    }

    /// The shape as an error message names it: v3's `describe_kind`.
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool => "a boolean",
            Self::Number => "a number",
            Self::String => "a string",
            Self::Array => "an array",
            Self::Object => "an object",
            Self::Other => "a non-array value",
        }
    }

    /// Whether a write can go on below a value of this shape.
    #[inline]
    pub(crate) fn is_container(self) -> bool {
        matches!(self, Self::Array | Self::Object)
    }
}

/// The container a write creates for the segment `next` to go into: an
/// array when it is an index, an object otherwise.
pub(crate) fn container_for(next: &Seg) -> Shape {
    if next.index.is_some() {
        Shape::Array
    } else {
        Shape::Object
    }
}

/// What a write does at one segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// The child exists: it is descended into, or replaced at the last
    /// segment.
    Into,
    /// The child is created here, and everything below it with it.
    Create,
    /// The write goes no further, and does nothing.
    Block,
}

/// What a write at segment `seg` does to a `holder` whose child there has
/// the shape `child` (`None` when missing); `last` when `seg` is the path's
/// last segment.
#[inline]
pub(crate) fn step(holder: Shape, seg: &Seg, child: Option<Shape>, last: bool) -> Step {
    match holder {
        Shape::Object => match child {
            None => Step::Create,
            Some(c) if last || c.is_container() => Step::Into,
            Some(_) => Step::Block,
        },
        Shape::Array if seg.index.is_some() => match child {
            None | Some(Shape::Null) => Step::Create,
            Some(c) if last || c.is_container() => Step::Into,
            Some(_) => Step::Block,
        },
        _ => Step::Block,
    }
}

/// One hop of a path on a container of a known shape: a key on an object,
/// an index on an array.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hop<'k> {
    Key(&'k str),
    Index(usize),
}

/// A tree a path is walked over: the owned tree, or the arena view.
#[cfg(test)]
pub(crate) trait Node: Copy {
    /// The node's shape, and the child `seg` names on it (its key on an
    /// object, its index on an array), matching the node once.
    fn at(self, seg: &Seg) -> (Shape, Option<Self>);
    /// The node's shape.
    fn shape(self) -> Shape;
}

#[cfg(test)]
impl Node for &OwnedDataValue {
    #[inline]
    fn at(self, seg: &Seg) -> (Shape, Option<Self>) {
        match self {
            OwnedDataValue::Object(pairs) => (
                Shape::Object,
                pairs.iter().find(|(k, _)| **k == *seg.key).map(|(_, c)| c),
            ),
            OwnedDataValue::Array(items) => (Shape::Array, seg.index.and_then(|i| items.get(i))),
            other => (Shape::of_owned(other), None),
        }
    }

    #[inline]
    fn shape(self) -> Shape {
        Shape::of_owned(self)
    }
}

#[cfg(test)]
impl Node for DataValue<'_> {
    #[inline]
    fn at(self, seg: &Seg) -> (Shape, Option<Self>) {
        match self {
            DataValue::Object(pairs) => (
                Shape::Object,
                pairs.iter().find(|(k, _)| **k == *seg.key).map(|(_, c)| *c),
            ),
            DataValue::Array(items) => {
                (Shape::Array, seg.index.and_then(|i| items.get(i).copied()))
            }
            other => (Shape::of_view(&other), None),
        }
    }

    #[inline]
    fn shape(self) -> Shape {
        Shape::of_view(&self)
    }
}

/// [`owned_child`], mutably.
pub(crate) fn owned_child_mut<'v>(
    v: &'v mut OwnedDataValue,
    hop: Hop<'_>,
) -> Option<&'v mut OwnedDataValue> {
    match (v, hop) {
        (OwnedDataValue::Object(pairs), Hop::Key(k)) => {
            pairs.iter_mut().find(|(pk, _)| pk == k).map(|(_, c)| c)
        }
        (OwnedDataValue::Array(items), Hop::Index(i)) => items.get_mut(i),
        _ => None,
    }
}

/// The child of a view node at `hop`: the first matching key.
#[inline]
pub(crate) fn view_child<'v>(v: DataValue<'v>, hop: Hop<'_>) -> Option<DataValue<'v>> {
    match (v, hop) {
        (DataValue::Object(pairs), Hop::Key(k)) => {
            pairs.iter().find(|(pk, _)| **pk == *k).map(|(_, c)| *c)
        }
        (DataValue::Array(items), Hop::Index(i)) => items.get(i).copied(),
        _ => None,
    }
}

/// Puts `v` at `hop` of an owned node: an existing key replaced, a new one
/// appended, an index padded with `null` up to it and assigned. Returns the
/// slot; `None`, putting nothing, when the node takes no such hop.
pub(crate) fn put<'v>(
    parent: &'v mut OwnedDataValue,
    hop: Hop<'_>,
    v: OwnedDataValue,
) -> Option<&'v mut OwnedDataValue> {
    match (parent, hop) {
        (OwnedDataValue::Object(pairs), Hop::Key(k)) => {
            let at = match pairs.iter().position(|(pk, _)| pk == k) {
                Some(at) => {
                    pairs[at].1 = v;
                    at
                }
                None => {
                    pairs.push((k.to_owned(), v));
                    pairs.len() - 1
                }
            };
            Some(&mut pairs[at].1)
        }
        (OwnedDataValue::Array(items), Hop::Index(i)) => {
            if items.len() <= i {
                items.resize(i + 1, OwnedDataValue::Null);
            }
            items[i] = v;
            Some(&mut items[i])
        }
        _ => None,
    }
}

/// Takes the child at `hop` out of an owned node: an object keeps the order
/// of its other keys, an array shifts its tail down. `None` when nothing is
/// there.
pub(crate) fn take(parent: &mut OwnedDataValue, hop: Hop<'_>) -> Option<OwnedDataValue> {
    match (parent, hop) {
        (OwnedDataValue::Object(pairs), Hop::Key(k)) => {
            let at = pairs.iter().position(|(pk, _)| pk == k)?;
            Some(pairs.remove(at).1)
        }
        (OwnedDataValue::Array(items), Hop::Index(i)) => (i < items.len()).then(|| items.remove(i)),
        _ => None,
    }
}

/// Where a write at a path lands: which segments resolve to an array index
/// on the containers the tree actually has (bit `i` of `kinds`), and the
/// first segment whose node the write creates (a missing key, an index past
/// the end, or a `null` element replaced by a container), if any. Past the
/// last existing container, a segment is an index exactly when it parses as
/// one.
///
/// The write log is keyed by resolved hops, not segment text, because text
/// is ambiguous: `1` and `+1` name one array element, and `01` and `#01` one
/// object key, while `1` and `+1` are two different object keys. Resolving
/// to a bitmask rather than to a list of hops is what keeps a write free of
/// allocation: a hop is built only when the log creates a node for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Resolved {
    /// Bit `i` set when segment `i` resolved to an array index.
    pub(crate) kinds: u64,
    /// The first segment whose node the write creates, if any.
    pub(crate) created: Option<usize>,
}

/// Where a write at `segs` lands on `root` ([`Resolved`]), by [`step`] at
/// every segment, over either tree; `None` when the write would do nothing.
/// The reference the hot walks are held to.
#[cfg(test)]
pub(crate) fn resolve<N: Node>(root: N, segs: &[Seg]) -> Option<Resolved> {
    if segs.is_empty() || segs.len() > MAX_PATH_SEGMENTS {
        return None;
    }
    let mut kinds = 0u64;
    let mut created = None;
    let mut at = Some(root);
    for (i, seg) in segs.iter().enumerate() {
        let Some(node) = at else {
            // Past the last existing container: what is created below.
            if seg.index.is_some() {
                kinds |= 1 << i;
            }
            continue;
        };
        let (holder, child) = node.at(seg);
        if holder == Shape::Array {
            kinds |= 1 << i;
        }
        match step(holder, seg, child.map(N::shape), i + 1 == segs.len()) {
            Step::Block => return None,
            Step::Create => {
                created.get_or_insert(i);
                at = None;
            }
            Step::Into => at = child,
        }
    }
    Some(Resolved { kinds, created })
}

/// What `map`'s append and extend do over a target of shape `held`
/// (`None` when missing): `Ok(true)` extends the array there, `Ok(false)`
/// creates one over a missing or `null` target, and `Err` names any other
/// target's shape, which refuses the append.
pub(crate) fn append_to(held: Option<Shape>) -> Result<bool, &'static str> {
    match held {
        Some(Shape::Array) => Ok(true),
        None | Some(Shape::Null) => Ok(false),
        Some(other) => Err(other.describe()),
    }
}

impl Seg {
    /// Whether this segment and `other` can name one node: by key, as on an
    /// object, or by index, as on an array. Either reading matching is
    /// enough.
    pub(crate) fn may_alias(&self, other: &Self) -> bool {
        self.key == other.key || self.index.is_some_and(|i| other.index == Some(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_is_v3s_rule_at_one_segment() {
        let key = Seg::parse("a");
        let index = Seg::parse("0");
        use Shape::{Array, Null, Number, Object};
        // On an object: a missing key is created, a container or the last
        // segment's child is entered, a scalar or `null` blocks.
        assert_eq!(step(Object, &key, None, false), Step::Create);
        assert_eq!(step(Object, &key, Some(Array), false), Step::Into);
        assert_eq!(step(Object, &key, Some(Null), false), Step::Block);
        assert_eq!(step(Object, &key, Some(Null), true), Step::Into);
        assert_eq!(step(Object, &key, Some(Number), false), Step::Block);
        // On an array: a key blocks, a missing or `null` element is created.
        assert_eq!(step(Array, &key, None, true), Step::Block);
        assert_eq!(step(Array, &index, None, false), Step::Create);
        assert_eq!(step(Array, &index, Some(Null), true), Step::Create);
        assert_eq!(step(Array, &index, Some(Number), true), Step::Into);
        assert_eq!(step(Array, &index, Some(Number), false), Step::Block);
        // A scalar or `null` holds nothing.
        assert_eq!(step(Number, &key, None, true), Step::Block);
        assert_eq!(step(Null, &index, None, true), Step::Block);
    }

    #[test]
    fn resolve_agrees_over_the_owned_tree_and_its_view() {
        let tree: OwnedDataValue =
            serde_json::from_str(r#"{"a": [null, {"b": 1}, 2], "c": {"d": null}, "e": "s"}"#)
                .unwrap();
        let arena = bumpalo::Bump::new();
        let view = tree.view_in(&arena);
        for dotted in [
            "a.0.x", "a.1.b", "a.1.z.0", "a.2.x", "a.5", "a.x", "c.d", "c.d.e", "e.f", "f.0.g",
            "a.01", "a.+1.b",
        ] {
            let segs: Vec<Seg> = dotted.split('.').map(Seg::parse).collect();
            assert_eq!(resolve(&tree, &segs), resolve(view, &segs), "{dotted}");
        }
    }

    #[test]
    fn an_append_extends_creates_or_refuses_by_the_target() {
        assert_eq!(append_to(Some(Shape::Array)), Ok(true));
        assert_eq!(append_to(None), Ok(false));
        assert_eq!(append_to(Some(Shape::Null)), Ok(false));
        assert_eq!(append_to(Some(Shape::String)), Err("a string"));
    }
}
