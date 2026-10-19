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
    set_resolved(arena, root, segs, value).map(|(root, _, _)| root)
}

/// [`set`], also reporting what [`resolve`] would have and the displaced old
/// value (if any): one walk where a write needs the splice, the hops to mark,
/// and the old value for a `Change`.
pub(crate) fn set_resolved<'a>(
    arena: &'a Bump,
    root: DataValue<'a>,
    segs: &[Seg],
    value: DataValue<'a>,
) -> Option<(DataValue<'a>, Resolved, Option<DataValue<'a>>)> {
    if segs.len() > MAX_PATH_SEGMENTS {
        return None;
    }
    let (seg, rest) = segs.split_first()?;
    let mut r = Resolved {
        kinds: 0,
        created: None,
    };
    let mut old = None;
    let root = set_in(arena, root, seg, rest, value, 0, &mut r, &mut old)?;
    Some((root, r, old))
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

/// One level of [`set_resolved`]: writes below `node`, noting what it creates
/// and what leaf value it displaced.
#[allow(clippy::too_many_arguments, reason = "recursive spine walk state")]
fn set_in<'a>(
    arena: &'a Bump,
    node: DataValue<'a>,
    seg: &Seg,
    rest: &[Seg],
    value: DataValue<'a>,
    depth: usize,
    r: &mut Resolved,
    old: &mut Option<DataValue<'a>>,
) -> Option<DataValue<'a>> {
    match node {
        DataValue::Object(pairs) => {
            let at = pairs.iter().position(|(k, _)| **k == *seg.key);
            let child = match (rest.split_first(), at) {
                (None, Some(at)) => {
                    *old = Some(pairs[at].1);
                    value
                }
                (None, None) => {
                    r.created.get_or_insert(depth);
                    value
                }
                // An existing child is descended into, and a scalar or
                // `null` there makes the whole write do nothing.
                (Some((next, after)), Some(at)) => {
                    set_in(arena, pairs[at].1, next, after, value, depth + 1, r, old)?
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
            let slot = items.get(at).copied();
            let existing = slot.filter(|v| !v.is_null());
            let child = match (rest.split_first(), existing) {
                (None, Some(_)) => {
                    *old = slot;
                    value
                }
                (None, None) => {
                    *old = slot;
                    r.created.get_or_insert(depth);
                    value
                }
                (Some((next, after)), Some(existing)) => {
                    set_in(arena, existing, next, after, value, depth + 1, r, old)?
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
        None => DataValue::Object(arena.alloc_slice_copy(&[(&*arena.alloc_str(&seg.key), child)])),
    }
}

/// `root` with `segs` removed, or `None` when nothing was there. An object
/// keeps the order of its other keys; an array shifts its tail down.
pub(crate) fn remove<'a>(
    arena: &'a Bump,
    root: DataValue<'a>,
    segs: &[Seg],
) -> Option<DataValue<'a>> {
    remove_resolved(arena, root, segs).map(|(root, _, _)| root)
}

/// [`remove`], also reporting the `kinds` [`resolve`] would have (bit `i`
/// set when segment `i` is an array index) and the removed value. One walk
/// where a removal needs all three.
pub(crate) fn remove_resolved<'a>(
    arena: &'a Bump,
    root: DataValue<'a>,
    segs: &[Seg],
) -> Option<(DataValue<'a>, u64, DataValue<'a>)> {
    let (seg, rest) = segs.split_first()?;
    let mut kinds = 0;
    let mut removed = DataValue::Null;
    let root = remove_in(arena, root, seg, rest, 0, &mut kinds, &mut removed)?;
    Some((root, kinds, removed))
}

/// One level of [`remove_resolved`], at segment `depth`.
fn remove_in<'a>(
    arena: &'a Bump,
    node: DataValue<'a>,
    seg: &Seg,
    rest: &[Seg],
    depth: usize,
    kinds: &mut u64,
    removed: &mut DataValue<'a>,
) -> Option<DataValue<'a>> {
    match node {
        DataValue::Object(pairs) => {
            let at = pairs.iter().position(|(k, _)| **k == *seg.key)?;
            Some(DataValue::Object(match rest.split_first() {
                None => {
                    *removed = pairs[at].1;
                    without(arena, pairs, at)
                }
                Some((next, rest)) => {
                    let child =
                        remove_in(arena, pairs[at].1, next, rest, depth + 1, kinds, removed)?;
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
                None => {
                    *removed = items[at];
                    without(arena, items, at)
                }
                Some((next, rest)) => {
                    let child = remove_in(arena, items[at], next, rest, depth + 1, kinds, removed)?;
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
    Ok(set_resolved(arena, root, segs, array).map(|(root, r, _)| (root, r)))
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
        return DataValue::Object(arena.alloc_slice_copy(&[(&*arena.alloc_str(key), value)]));
    };
    let Some(at) = slots.iter().position(|(k, _)| *k == key) else {
        return DataValue::Object(appended(arena, slots, arena.alloc_str(key), value));
    };
    let merged = match (slots[at].1, value) {
        (DataValue::Object(existing), DataValue::Object(new_pairs)) => {
            // Count how many new keys are not already in `existing` so the
            // merged object is built in one arena allocation without a heap
            // `Vec`.
            let mut extra = 0usize;
            for (i, &(k, _)) in new_pairs.iter().enumerate() {
                let in_existing = existing.iter().any(|(ek, _)| *ek == k);
                let earlier_in_new = new_pairs[..i].iter().any(|(nk, _)| *nk == k);
                if !in_existing && !earlier_in_new {
                    extra += 1;
                }
            }
            let out = arena.alloc_slice_fill_copy(existing.len() + extra, ("", DataValue::Null));
            out[..existing.len()].copy_from_slice(existing);
            let mut len = existing.len();
            for &(k, v) in new_pairs {
                match out[..len].iter_mut().find(|(ek, _)| *ek == k) {
                    Some(slot) => slot.1 = v,
                    None => {
                        out[len] = (k, v);
                        len += 1;
                    }
                }
            }
            DataValue::Object(&*out)
        }
        (_, value) => value,
    };
    DataValue::Object(replaced(arena, slots, at, merged))
}

/// `root` with the existing object at `segs` updated by `update`, which
/// receives the object's pairs and returns the updated pairs (along with any
/// caller state `T`), or `None` when `segs` does not resolve to an object or
/// `update` declines it. One walk of `segs`.
pub(crate) fn update_object<'a, T>(
    arena: &'a Bump,
    root: DataValue<'a>,
    segs: &[Seg],
    update: impl FnOnce(
        &'a Bump,
        &'a [(&'a str, DataValue<'a>)],
    ) -> Option<(&'a [(&'a str, DataValue<'a>)], T)>,
) -> Option<(DataValue<'a>, u64, T)> {
    let (seg, rest) = segs.split_first()?;
    let mut kinds = 0u64;
    let mut out = None;
    let root = update_object_in(
        arena,
        root,
        seg,
        rest,
        0,
        &mut kinds,
        &mut Some(update),
        &mut out,
    )?;
    Some((root, kinds, out?))
}

#[allow(clippy::too_many_arguments, reason = "recursive spine walk state")]
fn update_object_in<'a, T, F>(
    arena: &'a Bump,
    node: DataValue<'a>,
    seg: &Seg,
    rest: &[Seg],
    depth: usize,
    kinds: &mut u64,
    update: &mut Option<F>,
    out: &mut Option<T>,
) -> Option<DataValue<'a>>
where
    F: FnOnce(
        &'a Bump,
        &'a [(&'a str, DataValue<'a>)],
    ) -> Option<(&'a [(&'a str, DataValue<'a>)], T)>,
{
    match node {
        DataValue::Object(pairs) => {
            let at = pairs.iter().position(|(k, _)| **k == *seg.key)?;
            let child = match rest.split_first() {
                None => {
                    let DataValue::Object(obj) = pairs[at].1 else {
                        return None;
                    };
                    let (updated, val) = update.take()?(arena, obj)?;
                    *out = Some(val);
                    if std::ptr::eq(updated, obj) {
                        return Some(node);
                    }
                    DataValue::Object(updated)
                }
                Some((next, after)) => {
                    let child = update_object_in(
                        arena,
                        pairs[at].1,
                        next,
                        after,
                        depth + 1,
                        kinds,
                        update,
                        out,
                    )?;
                    if match (pairs[at].1, child) {
                        (DataValue::Object(a), DataValue::Object(b)) => std::ptr::eq(a, b),
                        (DataValue::Array(a), DataValue::Array(b)) => std::ptr::eq(a, b),
                        _ => false,
                    } {
                        return Some(node);
                    }
                    child
                }
            };
            Some(DataValue::Object(replaced(arena, pairs, at, child)))
        }
        DataValue::Array(items) => {
            let at = seg.index?;
            if at >= items.len() {
                return None;
            }
            *kinds |= 1 << depth;
            let child = match rest.split_first() {
                None => {
                    let DataValue::Object(obj) = items[at] else {
                        return None;
                    };
                    let (updated, val) = update.take()?(arena, obj)?;
                    *out = Some(val);
                    if std::ptr::eq(updated, obj) {
                        return Some(node);
                    }
                    DataValue::Object(updated)
                }
                Some((next, after)) => {
                    let child = update_object_in(
                        arena,
                        items[at],
                        next,
                        after,
                        depth + 1,
                        kinds,
                        update,
                        out,
                    )?;
                    if match (items[at], child) {
                        (DataValue::Object(a), DataValue::Object(b)) => std::ptr::eq(a, b),
                        (DataValue::Array(a), DataValue::Array(b)) => std::ptr::eq(a, b),
                        _ => false,
                    } {
                        return Some(node);
                    }
                    child
                }
            };
            Some(DataValue::Array(
                arena
                    .alloc_slice_fill_with(items.len(), |i| if i == at { child } else { items[i] }),
            ))
        }
        _ => None,
    }
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
