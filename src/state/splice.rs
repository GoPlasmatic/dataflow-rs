//! Writes into an arena view, copying only the spine.
//!
//! Each function returns a new root. Every container on the written path is
//! rebuilt as a new slice in the arena with its one changed child replaced or
//! appended; siblings are copied as values and never descended into, so a
//! write costs its path's spine, whatever the size of the tree. The rules are
//! [`crate::expr::rules`]'s, hand-inlined here for the hot path:
//! `state::tests::splice_property` checks `view(apply(c, w)) ==
//! splice(view(c), w)` on random trees and writes, and holds [`resolve`] and
//! [`set_resolved`] to [`rules::resolve`] on every small tree and path.

use crate::expr::Seg;
use crate::expr::path::MAX_PATH_SEGMENTS;
use crate::expr::rules::{self, Shape};
use bumpalo::Bump;
use datavalue::DataValue;

pub(crate) use crate::expr::rules::Resolved;

/// The value at `segs`, with [`crate::expr::path::get`]'s rules.
pub(crate) fn get<'a>(mut v: DataValue<'a>, segs: &[Seg]) -> Option<DataValue<'a>> {
    for seg in segs {
        v = match v {
            DataValue::Object(pairs) => pairs.iter().find(|(k, _)| **k == *seg.key)?.1,
            DataValue::Array(items) => *items.get(seg.index?)?,
            _ => return None,
        };
    }
    Some(v)
}

/// `root` with `value` written at `segs`, or `None` when the write does
/// nothing: in exactly the cases [`crate::expr::path::set`] returns false.
pub(crate) fn set<'a>(
    arena: &'a Bump,
    root: DataValue<'a>,
    segs: &[Seg],
    value: DataValue<'a>,
) -> Option<DataValue<'a>> {
    set_resolved(arena, root, segs, value).map(|(root, _)| root)
}

/// [`set`], also reporting what [`resolve`] would have: one walk where a
/// write needs both, the splice and the hops to mark.
pub(crate) fn set_resolved<'a>(
    arena: &'a Bump,
    root: DataValue<'a>,
    segs: &[Seg],
    value: DataValue<'a>,
) -> Option<(DataValue<'a>, Resolved)> {
    if segs.len() > MAX_PATH_SEGMENTS {
        return None;
    }
    let (seg, rest) = segs.split_first()?;
    let mut r = Resolved {
        kinds: 0,
        created: None,
    };
    let root = set_in(arena, root, seg, rest, value, 0, &mut r)?;
    Some((root, r))
}

/// The segments from `depth` on are created: an index wherever one parses.
fn created_from(r: &mut Resolved, depth: usize, segs: &[Seg]) {
    r.created.get_or_insert(depth);
    for (j, seg) in segs.iter().enumerate() {
        if seg.index.is_some() {
            r.kinds |= 1 << (depth + j);
        }
    }
}

/// One level of [`set_resolved`]: writes below `node`, noting what it creates.
fn set_in<'a>(
    arena: &'a Bump,
    node: DataValue<'a>,
    seg: &Seg,
    rest: &[Seg],
    value: DataValue<'a>,
    depth: usize,
    r: &mut Resolved,
) -> Option<DataValue<'a>> {
    match node {
        DataValue::Object(pairs) => {
            let at = pairs.iter().position(|(k, _)| **k == *seg.key);
            let child = match (rest.split_first(), at) {
                (None, Some(_)) => value,
                (None, None) => {
                    r.created.get_or_insert(depth);
                    value
                }
                // An existing child is descended into, and a scalar or
                // `null` there makes the whole write do nothing.
                (Some((next, after)), Some(at)) => {
                    set_in(arena, pairs[at].1, next, after, value, depth + 1, r)?
                }
                // A missing key on an object is a key, whatever it looks
                // like; what is created below it follows its segments.
                (Some(_), None) => {
                    r.created.get_or_insert(depth);
                    created_from(r, depth + 1, rest);
                    chain(arena, rest, value)
                }
            };
            Some(DataValue::Object(match at {
                Some(at) => replaced(arena, pairs, at, child),
                None => appended(arena, pairs, arena.alloc_str(&seg.key), child),
            }))
        }
        DataValue::Array(items) => {
            let at = seg.index?;
            r.kinds |= 1 << depth;
            let existing = items.get(at).filter(|v| !v.is_null());
            let child = match (rest.split_first(), existing) {
                (None, Some(_)) => value,
                (None, None) => {
                    r.created.get_or_insert(depth);
                    value
                }
                (Some((next, after)), Some(existing)) => {
                    set_in(arena, *existing, next, after, value, depth + 1, r)?
                }
                // A `null` or a padded slot becomes a new container, built
                // from `next` on.
                (Some(_), None) => {
                    r.created.get_or_insert(depth);
                    created_from(r, depth + 1, rest);
                    chain(arena, rest, value)
                }
            };
            let len = items.len().max(at + 1);
            Some(DataValue::Array(arena.alloc_slice_fill_with(len, |i| {
                if i == at {
                    child
                } else {
                    items.get(i).copied().unwrap_or(DataValue::Null)
                }
            })))
        }
        _ => None,
    }
}

/// What writing `value` at `segs` into a new, empty container produces: the
/// container is an array when `segs[0]` is an index, padded with `null`, and
/// an object otherwise. `segs` empty is `value` itself.
fn chain<'a>(arena: &'a Bump, segs: &[Seg], value: DataValue<'a>) -> DataValue<'a> {
    let Some((seg, rest)) = segs.split_first() else {
        return value;
    };
    let child = chain(arena, rest, value);
    match seg.index {
        Some(at) => DataValue::Array(
            arena.alloc_slice_fill_with(at + 1, |i| if i == at { child } else { DataValue::Null }),
        ),
        None => DataValue::Object(
            arena.alloc_slice_fill_iter(std::iter::once((&*arena.alloc_str(&seg.key), child))),
        ),
    }
}

/// `root` with `segs` removed, or `None` when nothing was there. An object
/// keeps the order of its other keys; an array shifts its tail down.
pub(crate) fn remove<'a>(
    arena: &'a Bump,
    root: DataValue<'a>,
    segs: &[Seg],
) -> Option<DataValue<'a>> {
    remove_resolved(arena, root, segs).map(|(root, _)| root)
}

/// [`remove`], also reporting the `kinds` [`resolve`] would have: bit `i`
/// set when segment `i` is an array index. One walk where a removal needs
/// both.
pub(crate) fn remove_resolved<'a>(
    arena: &'a Bump,
    root: DataValue<'a>,
    segs: &[Seg],
) -> Option<(DataValue<'a>, u64)> {
    let (seg, rest) = segs.split_first()?;
    let mut kinds = 0;
    let root = remove_in(arena, root, seg, rest, 0, &mut kinds)?;
    Some((root, kinds))
}

/// One level of [`remove_resolved`], at segment `depth`.
fn remove_in<'a>(
    arena: &'a Bump,
    node: DataValue<'a>,
    seg: &Seg,
    rest: &[Seg],
    depth: usize,
    kinds: &mut u64,
) -> Option<DataValue<'a>> {
    match node {
        DataValue::Object(pairs) => {
            let at = pairs.iter().position(|(k, _)| **k == *seg.key)?;
            Some(DataValue::Object(match rest.split_first() {
                None => without(arena, pairs, at),
                Some((next, rest)) => {
                    let child = remove_in(arena, pairs[at].1, next, rest, depth + 1, kinds)?;
                    replaced(arena, pairs, at, child)
                }
            }))
        }
        DataValue::Array(items) => {
            let at = seg.index?;
            if at >= items.len() {
                return None;
            }
            *kinds |= 1 << depth;
            Some(DataValue::Array(match rest.split_first() {
                None => without(arena, items, at),
                Some((next, rest)) => {
                    let child = remove_in(arena, items[at], next, rest, depth + 1, kinds)?;
                    arena.alloc_slice_fill_with(
                        items.len(),
                        |i| if i == at { child } else { items[i] },
                    )
                }
            }))
        }
        _ => None,
    }
}

/// `map`'s append and extend: `Ok(Some((root, resolved)))` with `items`
/// added to the array at `segs` (created over a missing or `null` target),
/// and what [`resolve`] would have reported for `segs`; `Ok(None)` when the
/// path is unreachable, which v3 treats as success with nothing written,
/// and `Err` naming the kind of any other target. `existing` is what
/// [`get`] finds at `segs`, which the caller has read.
///
/// The new array is a shallow copy of the old slice plus the items: O(array)
/// pointer copies in the arena, never a conversion, as v3's append is.
pub(crate) fn append<'a>(
    arena: &'a Bump,
    root: DataValue<'a>,
    segs: &[Seg],
    existing: Option<DataValue<'a>>,
    items: &[DataValue<'a>],
) -> Result<Option<(DataValue<'a>, Resolved)>, &'static str> {
    let array = match (
        rules::append_to(existing.map(|v| Shape::of_view(&v)))?,
        existing,
    ) {
        (true, Some(DataValue::Array(old))) => {
            DataValue::Array(arena.alloc_slice_fill_with(old.len() + items.len(), |i| {
                if i < old.len() {
                    old[i]
                } else {
                    items[i - old.len()]
                }
            }))
        }
        _ => DataValue::Array(arena.alloc_slice_copy(items)),
    };
    Ok(set_resolved(arena, root, segs, array))
}

/// `map`'s write to a root slot: two objects merge, existing keys replaced in
/// place and new keys appended in order; anything else replaces the slot. The
/// rules are the owned `path::merge_root`'s, which is v3's
/// `merge_root_field`, and which a replayed trace from an earlier build may
/// still need whole: `map` itself writes only an object at a root.
pub(crate) fn merge_root<'a>(
    arena: &'a Bump,
    root: DataValue<'a>,
    key: &str,
    value: DataValue<'a>,
) -> DataValue<'a> {
    let DataValue::Object(slots) = root else {
        return DataValue::Object(
            arena.alloc_slice_fill_iter(std::iter::once((&*arena.alloc_str(key), value))),
        );
    };
    let Some(at) = slots.iter().position(|(k, _)| *k == key) else {
        return DataValue::Object(appended(arena, slots, arena.alloc_str(key), value));
    };
    let merged = match (slots[at].1, value) {
        (DataValue::Object(existing), DataValue::Object(new_pairs)) => {
            let mut pairs: Vec<(&'a str, DataValue<'a>)> = existing.to_vec();
            for &(k, v) in new_pairs {
                match pairs.iter_mut().find(|(ek, _)| *ek == k) {
                    Some(slot) => slot.1 = v,
                    None => pairs.push((k, v)),
                }
            }
            DataValue::Object(arena.alloc_slice_copy(&pairs))
        }
        (_, value) => value,
    };
    DataValue::Object(replaced(arena, slots, at, merged))
}

/// The kind of a value, for an error: v3's `describe_kind`.
pub(crate) fn kind(v: &DataValue<'_>) -> &'static str {
    Shape::of_view(v).describe()
}

/// `pairs` with the value at `at` replaced.
fn replaced<'a>(
    arena: &'a Bump,
    pairs: &'a [(&'a str, DataValue<'a>)],
    at: usize,
    value: DataValue<'a>,
) -> &'a [(&'a str, DataValue<'a>)] {
    arena.alloc_slice_fill_with(pairs.len(), |i| {
        if i == at {
            (pairs[i].0, value)
        } else {
            pairs[i]
        }
    })
}

/// `pairs` with `key` and `value` added at the end.
fn appended<'a>(
    arena: &'a Bump,
    pairs: &'a [(&'a str, DataValue<'a>)],
    key: &'a str,
    value: DataValue<'a>,
) -> &'a [(&'a str, DataValue<'a>)] {
    arena.alloc_slice_fill_with(pairs.len() + 1, |i| {
        if i < pairs.len() {
            pairs[i]
        } else {
            (key, value)
        }
    })
}

/// A copy of `items` without the element at `at`, the tail shifted down.
fn without<'a, T: Copy>(arena: &'a Bump, items: &[T], at: usize) -> &'a [T] {
    arena.alloc_slice_fill_with(items.len() - 1, |i| {
        if i < at { items[i] } else { items[i + 1] }
    })
}

/// Where a write at `segs` lands ([`Resolved`]), [`rules::resolve`]'s walk
/// hand-inlined over the view, which every read of a written run takes:
/// which segments resolve to an array index
/// on the containers `root` actually has (bit `i` of `kinds`), and the first
/// segment whose node the write creates (a missing key, an index past the
/// end, or a `null` element replaced by a container), if any. Past the last
/// existing container, a segment is an index exactly when it parses as one.
/// `None` when the write would do nothing.
///
/// The write log is keyed by resolved hops, not segment text, because text
/// is ambiguous: `1` and `+1` name one array element, and `01` and `#01` one
/// object key, while `1` and `+1` are two different object keys. Resolving
/// to a bitmask rather than to a list of hops is what keeps a write free of
/// allocation: a hop is built only when the log creates a node for it.
pub(crate) fn resolve(root: DataValue<'_>, segs: &[Seg]) -> Option<Resolved> {
    if segs.is_empty() || segs.len() > MAX_PATH_SEGMENTS {
        return None;
    }
    let mut kinds = 0u64;
    let mut created = None;
    let mut at = Some(root);
    for (i, seg) in segs.iter().enumerate() {
        let last = i + 1 == segs.len();
        match at {
            Some(DataValue::Object(pairs)) => {
                let child = pairs.iter().find(|(k, _)| **k == *seg.key).map(|(_, v)| *v);
                // A scalar or `null` under an object stops a deeper write.
                at = match child {
                    Some(c)
                        if !last && !matches!(c, DataValue::Object(_) | DataValue::Array(_)) =>
                    {
                        return None;
                    }
                    Some(c) => Some(c),
                    None => {
                        created.get_or_insert(i);
                        None
                    }
                };
            }
            Some(DataValue::Array(items)) => {
                let index = seg.index?;
                kinds |= 1 << i;
                // A `null` element is replaced by a new container, like a
                // missing one; a scalar stops a deeper write.
                at = match items.get(index) {
                    Some(DataValue::Null) | None => {
                        created.get_or_insert(i);
                        None
                    }
                    Some(c)
                        if !last && !matches!(c, DataValue::Object(_) | DataValue::Array(_)) =>
                    {
                        return None;
                    }
                    Some(c) => Some(*c),
                };
            }
            Some(_) => return None,
            None => {
                if seg.index.is_some() {
                    kinds |= 1 << i;
                }
            }
        }
    }
    Some(Resolved { kinds, created })
}

#[cfg(test)]
impl Resolved {
    /// The hops, built: for the tests.
    pub(crate) fn hops(&self, segs: &[Seg]) -> Vec<Hop> {
        super::writes::At::new(segs, self.kinds).to_hops()
    }
}

/// One resolved step of a path: what the tests read the write log's keys as.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Hop {
    /// A key on an object.
    Key(Box<str>),
    /// An index on an array.
    Index(usize),
}

#[cfg(test)]
impl Hop {
    /// The segment that addresses exactly this hop: a key never reads as an
    /// index, and an index is spelled in decimal.
    pub(crate) fn to_seg(&self) -> Seg {
        match self {
            Self::Key(k) => Seg::key(k),
            Self::Index(i) => Seg::index(*i),
        }
    }
}

/// The segments that address `hops` exactly.
#[cfg(test)]
pub(crate) fn segs_of(hops: &[Hop]) -> Vec<Seg> {
    hops.iter().map(Hop::to_seg).collect()
}
