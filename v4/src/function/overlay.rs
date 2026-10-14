//! A host call's overlay: the writes one call makes, kept apart from the run
//! until the call settles (05, "A host `Function`").
//!
//! It is the list of the call's writes as it made them (I87): each a path
//! and the value written there, or a removal. [`Ctx`](super::Ctx) uses it
//! through the operations below, so a change of structure touches this file
//! only.
//!
//! - A read scans the list from the newest write. The newest write at or
//!   above the path decides it; one below it, or one that may have moved
//!   it, sends the read to the call's own root: the base with the writes
//!   spliced on in order.
//! - An evaluation reads that root.
//! - The settle replays the writes through the run context in order, each at
//!   its own path, against the context as it stands then. So overlays
//!   planned over one base compose, as v3's fold replays each element's
//!   `Change`s, and a write lands on the writes settled before it.
//!
//! Two segments can name one place without being equal (`1`, `01` and `+1`
//! one element; `1` and `#1` one key), an element's removal moves the
//! elements after it, and a write past an array's end pads it. A read the
//! list cannot answer for certain is sent to the spliced root, which can.

use crate::expr::arena::with_arena;
use crate::expr::path;
use crate::expr::{Path, Seg};
use crate::state::splice;
use crate::state::{Lookup, RunContext};
use bumpalo::Bump;
use datavalue::{DataValue, OwnedDataValue};

/// The writes of one call, in the order they were made.
#[derive(Debug, Default)]
pub(crate) struct Overlay(Vec<(Path, Write)>);

/// One write a call made, at the path it named.
#[derive(Debug)]
pub(crate) enum Write {
    /// A value written, which the call's own root took.
    Set(OwnedDataValue),
    /// A value written where the call's own root could not take it, below
    /// a scalar. It changes nothing the call reads, and the settle still
    /// replays it, since the context may take it then, as v3 replays every
    /// `Change`.
    Blocked(OwnedDataValue),
    /// A value the call saw, removed.
    Removed,
}

impl Overlay {
    /// Whether the call has written nothing.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Writes `value` at `path` over `base`, the root the call reads beneath
    /// its own writes. Whether the call's own root takes it, by `Path::set`'s
    /// rules, is settled here, against `base` and the writes before it.
    pub(crate) fn set(&mut self, base: DataValue<'_>, path: &Path, value: OwnedDataValue) {
        let lands = self.lands(base, path.segs());
        self.push(path, value, lands);
    }

    /// Writes `value` at `path` where the caller has settled whether the
    /// call's own root takes it: `Ctx`'s first write learns it in the walk
    /// that reads the old value.
    pub(crate) fn push(&mut self, path: &Path, value: OwnedDataValue, lands: bool) {
        let write = if lands {
            Write::Set(value)
        } else {
            Write::Blocked(value)
        };
        self.0.push((path.clone(), write));
    }

    /// Removes `path`, where the call sees a value.
    pub(crate) fn remove(&mut self, path: &Path) {
        self.0.push((path.clone(), Write::Removed));
    }

    /// What a read of `path` finds here: `Owned` written here (`None` when
    /// removed), `Dirty` when written below it or moved, so the read
    /// splices, or `Clean` when no write reaches it, and the base answers.
    pub(crate) fn lookup(&self, path: &Path) -> Lookup<'_> {
        let p = path.segs();
        for (at, write) in self.0.iter().rev() {
            let e = at.segs();
            let set = match write {
                Write::Set(value) => Some(value),
                Write::Removed => None,
                Write::Blocked(_) => continue,
            };
            match relation(e, p) {
                Rel::Apart => {}
                Rel::Siblings { at, e: x, p: y } if !moves(e.len(), set.is_some(), at, x, y) => {}
                Rel::Above => {
                    return match set {
                        Some(value) => Lookup::Owned(path::get(value, &p[e.len()..])),
                        None if may_shift(e) => Lookup::Dirty,
                        None => Lookup::Owned(None),
                    };
                }
                Rel::Below | Rel::Siblings { .. } | Rel::Unsure => return Lookup::Dirty,
            }
        }
        Lookup::Clean
    }

    /// `root` with every write spliced on, in order, in `arena`. The values
    /// are viewed, not copied.
    pub(crate) fn splice_onto<'a>(&'a self, root: DataValue<'a>, arena: &'a Bump) -> DataValue<'a> {
        self.0.iter().fold(root, |root, (path, write)| {
            let spliced = match write {
                Write::Set(value) => splice::set(arena, root, path.segs(), value.view_in(arena)),
                Write::Removed => splice::remove(arena, root, path.segs()),
                Write::Blocked(_) => return root,
            };
            debug_assert!(
                spliced.is_some(),
                "a write lands on the root it was made on"
            );
            spliced.unwrap_or(root)
        })
    }

    /// Replays every write through `rc`, in the order the call made them,
    /// each at its own path, against the context as it stands: one that no
    /// longer lands writes nothing. A write is not recorded again: the call
    /// recorded it.
    pub(crate) fn apply(self, rc: &mut RunContext<'_, '_>) {
        for (path, write) in self.0 {
            match write {
                Write::Set(value) | Write::Blocked(value) => {
                    rc.set_segs(path.segs(), value, None);
                }
                Write::Removed => {
                    rc.remove_segs(path.segs());
                }
            }
        }
    }

    /// Whether a write at `segs` lands on the call's own root, by
    /// `Path::set`'s rules: each container on the way is an object, or an
    /// array the next segment indexes, and a missing one or a `null` element
    /// is created, with everything below it.
    fn lands(&self, base: DataValue<'_>, segs: &[Seg]) -> bool {
        if self.is_empty() {
            return splice::resolve(base, segs).is_some();
        }
        // Every root is an object (R52), and so holds `segs[1]`.
        let mut holder = Kind::Object;
        for i in 1..segs.len() {
            let fits = match holder {
                Kind::Object => true,
                Kind::Array => segs[i].index.is_some(),
                _ => false,
            };
            if !fits {
                return false;
            }
            if i + 1 == segs.len() {
                break;
            }
            holder = match self.kind_at(base, &segs[..=i]) {
                Some(Kind::Missing) => return true,
                Some(Kind::Null) if holder == Kind::Array => return true,
                Some(kind) => kind,
                None => {
                    return with_arena(|arena| {
                        splice::resolve(self.splice_onto(base, arena), segs).is_some()
                    });
                }
            };
        }
        true
    }

    /// What the call's own root holds at `q`, as far as a write through it
    /// cares; `None` when only the spliced root can tell.
    fn kind_at(&self, base: DataValue<'_>, q: &[Seg]) -> Option<Kind> {
        // The first segment below `q` of the earliest write since the one
        // that decides `q` that landed below it: what created `q`, if it
        // was missing or a `null` element.
        let mut created = None;
        for (at, write) in self.0.iter().rev() {
            let e = at.segs();
            let set = match write {
                Write::Set(value) => Some(value),
                Write::Removed => None,
                Write::Blocked(_) => continue,
            };
            match relation(e, q) {
                Rel::Apart => {}
                Rel::Siblings { at, e: x, p: y } if !moves(e.len(), set.is_some(), at, x, y) => {}
                Rel::Below => {
                    if set.is_some() {
                        created = Some(&e[q.len()]);
                    }
                }
                Rel::Above => {
                    let kind = match set {
                        Some(value) => Kind::of_owned(path::get(value, &q[e.len()..])),
                        None if may_shift(e) => return None,
                        None => Kind::Missing,
                    };
                    return Some(kind.or_created(created));
                }
                Rel::Siblings { .. } | Rel::Unsure => return None,
            }
        }
        Some(Kind::of_view(splice::get(base, q)).or_created(created))
    }
}

/// What a path holds, as far as a write through it cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Missing,
    Null,
    Scalar,
    Object,
    Array,
}

impl Kind {
    fn of_view(v: Option<DataValue<'_>>) -> Self {
        match v {
            None => Self::Missing,
            Some(DataValue::Null) => Self::Null,
            Some(DataValue::Object(_)) => Self::Object,
            Some(DataValue::Array(_)) => Self::Array,
            Some(_) => Self::Scalar,
        }
    }

    fn of_owned(v: Option<&OwnedDataValue>) -> Self {
        match v {
            None => Self::Missing,
            Some(OwnedDataValue::Null) => Self::Null,
            Some(OwnedDataValue::Object(_)) => Self::Object,
            Some(OwnedDataValue::Array(_)) => Self::Array,
            Some(_) => Self::Scalar,
        }
    }

    /// The kind once a write landed below: a missing container, or a `null`
    /// element, is created as `Path::set` creates it, an array when the
    /// segment below it is an index. Only a `null` element takes a write
    /// below it, so a `null` that stays is one under an object.
    fn or_created(self, below: Option<&Seg>) -> Self {
        match (self, below) {
            (Self::Missing | Self::Null, Some(next)) if next.index.is_some() => Self::Array,
            (Self::Missing | Self::Null, Some(_)) => Self::Object,
            (kind, _) => kind,
        }
    }
}

/// How a write's path `e` stands to a path `p` the overlay is asked about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rel {
    /// `e` names `p` or a path above it: what `e` wrote decides `p`.
    Above,
    /// `e` names a path below `p`.
    Below,
    /// The two part at a segment where they name different places.
    Apart,
    /// The two part at segment `at`, where both name an array element:
    /// `e`'s and `p`'s.
    Siblings { at: usize, e: usize, p: usize },
    /// The two part at a segment that may name one place spelled two ways:
    /// one element (`1`, `01`), or one key, one spelling an index (`1`,
    /// `#1`).
    Unsure,
}

fn relation(e: &[Seg], p: &[Seg]) -> Rel {
    for (i, (a, b)) in e.iter().zip(p).enumerate() {
        let one_key = a.hash == b.hash && a.key == b.key;
        if one_key && a.index == b.index {
            continue;
        }
        return match (a.index, b.index) {
            (Some(x), Some(y)) if x != y => Rel::Siblings { at: i, e: x, p: y },
            (Some(_), Some(_)) => Rel::Unsure,
            _ if one_key => Rel::Unsure,
            _ => Rel::Apart,
        };
    }
    if e.len() <= p.len() {
        Rel::Above
    } else {
        Rel::Below
    }
}

/// Whether a write naming element `x`, at segment `at` of a path `e_len`
/// segments long, may change what element `y` holds: a value written past
/// `y` may pad the array up to it, and an element removed before `y` moves
/// the next one into its place.
fn moves(e_len: usize, set: bool, at: usize, x: usize, y: usize) -> bool {
    if set { x > y } else { e_len == at + 1 && x < y }
}

/// Whether a removal at `e` may have been of an array element, so the next
/// element moved into its place.
fn may_shift(e: &[Seg]) -> bool {
    e.last().is_some_and(|seg| seg.index.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::tests::random::{self, Rng};
    use crate::state::{RunContext, Tuning};

    /// Applies one write or removal to the model and the overlay, as `Ctx`
    /// would make it: a root takes only an object and is never removed, and
    /// removing nothing is nothing.
    fn step(rng: &mut Rng, overlay: &mut Overlay, base: DataValue<'_>, model: &mut OwnedDataValue) {
        let Ok(path) = random::rooted_path(rng, model).parse::<Path>() else {
            return;
        };
        if rng.chance(1, 3) {
            if path.is_root() || path.get(model).is_none() {
                return;
            }
            path::remove(model, path.segs());
            overlay.remove(&path);
        } else {
            let value = random::value(rng);
            if path.check_set(&value).is_err() {
                return;
            }
            path.assign(model, value.clone());
            overlay.set(base, &path, value);
        }
    }

    #[test]
    fn the_overlay_is_its_writes_applied_in_order() {
        let mut rng = Rng::new(0x5eed_0f0e_71a7);
        for case in 0..1500 {
            let owned = random::rooted_tree(&mut rng, 3);
            let arena = Bump::new();
            let base = owned.view_in(&arena);
            let mut overlay = Overlay::default();
            let mut model = owned.clone();
            for at in 0..1 + rng.below(6) {
                step(&mut rng, &mut overlay, base, &mut model);
                let here = Bump::new();
                let root = overlay.splice_onto(base, &here);
                let want = random::exact(&model);
                assert_eq!(
                    random::exact(&root.to_owned()),
                    want,
                    "case {case} step {at}: the spliced root"
                );
                // Every read agrees with the model, whichever part answers.
                for _ in 0..4 {
                    let Ok(read) = random::rooted_path(&mut rng, &model).parse::<Path>() else {
                        continue;
                    };
                    let got = match overlay.lookup(&read) {
                        Lookup::Clean => read.get(&owned).cloned(),
                        Lookup::Owned(v) => v.cloned(),
                        Lookup::Dirty => splice::get(root, read.segs()).map(|v| v.to_owned()),
                    };
                    assert_eq!(
                        got.as_ref().map(random::exact),
                        read.get(&model).map(random::exact),
                        "case {case} step {at}: a read of {read}"
                    );
                }
            }
            // Applied through a run context, with or without its view.
            let mut slot = owned.clone();
            let mut rc = RunContext::new(&mut slot, Tuning::default());
            if rng.chance(1, 2) {
                rc.read(|_, _| ());
            }
            overlay.apply(&mut rc);
            rc.finish();
            assert_eq!(
                random::exact(&slot),
                random::exact(&model),
                "case {case}: applied"
            );
        }
    }

    /// One write or removal a call makes over `own`, its own root, by
    /// `Ctx`'s rules: a root takes only an object and is never removed, and
    /// a removal is made only where the call sees a value. `None` when the
    /// draw is none of these.
    fn draw(rng: &mut Rng, own: &OwnedDataValue) -> Option<(Path, Option<OwnedDataValue>)> {
        let path = random::rooted_path(rng, own).parse::<Path>().ok()?;
        if rng.chance(1, 3) {
            (!path.is_root() && path.get(own).is_some()).then_some((path, None))
        } else {
            let value = random::value(rng);
            path.check_set(&value)
                .is_ok()
                .then_some((path, Some(value)))
        }
    }

    /// `path` written with `value`, or removed, in an owned context: what
    /// `Ctx::set` and `Ctx::remove` mean.
    fn write(context: &mut OwnedDataValue, path: &Path, value: Option<&OwnedDataValue>) {
        match value {
            Some(value) => {
                path.assign(context, value.clone());
            }
            None => {
                path::remove(context, path.segs());
            }
        }
    }

    /// Two or three calls each plan their writes over one base, as a
    /// fan-out's elements do, and their overlays are applied in call order
    /// through one run context. That must equal every call's writes applied
    /// in that order to one context: each write replayed at its own path,
    /// against the context as it stands, as v3's fold replays each
    /// element's `Change`s.
    #[test]
    fn overlays_planned_over_one_base_compose_in_order() {
        let mut rng = Rng::new(0xc0_4405_e0f1);
        for case in 0..1500 {
            let owned = random::rooted_tree(&mut rng, 3);
            let arena = Bump::new();
            let base = owned.view_in(&arena);
            let mut sequential = owned.clone();
            let mut overlays = Vec::new();
            for _ in 0..2 + rng.below(2) {
                let mut overlay = Overlay::default();
                let mut own = owned.clone();
                for _ in 0..1 + rng.below(5) {
                    let Some((path, value)) = draw(&mut rng, &own) else {
                        continue;
                    };
                    write(&mut own, &path, value.as_ref());
                    write(&mut sequential, &path, value.as_ref());
                    match value {
                        Some(value) => overlay.set(base, &path, value),
                        None => overlay.remove(&path),
                    }
                }
                overlays.push(overlay);
            }
            let mut slot = owned.clone();
            let mut rc = RunContext::new(&mut slot, Tuning::default());
            if rng.chance(1, 2) {
                rc.read(|_, _| ());
            }
            for overlay in overlays {
                overlay.apply(&mut rc);
            }
            rc.finish();
            let (got, want) = (random::exact(&slot), random::exact(&sequential));
            assert!(got == want, "case {case}: {}", random::diff(&got, &want));
        }
    }

    fn parse(dotted: &str) -> Path {
        dotted.parse().unwrap()
    }

    fn dv(v: serde_json::Value) -> OwnedDataValue {
        OwnedDataValue::from(&v)
    }

    /// The overlay's writes over `base`, spliced, as wire text: key order
    /// is what these tests check.
    fn spliced(overlay: &Overlay, base: DataValue<'_>) -> String {
        let arena = Bump::new();
        random::exact(&overlay.splice_onto(base, &arena).to_owned())
    }

    #[test]
    fn a_key_removed_and_written_again_moves_to_its_objects_end() {
        let owned = dv(serde_json::json!({"data": {"a": 1, "b": 2, "c": 3}}));
        let arena = Bump::new();
        let base = owned.view_in(&arena);
        let mut overlay = Overlay::default();
        overlay.remove(&parse("data.a"));
        overlay.set(base, &parse("data.a"), dv(serde_json::json!(4)));
        assert_eq!(spliced(&overlay, base), r#"{"data":{"b":2,"c":3,"a":4}}"#);
    }

    #[test]
    fn a_new_key_removed_and_written_again_follows_a_later_new_key() {
        let owned = dv(serde_json::json!({"data": {"a": 1}}));
        let arena = Bump::new();
        let base = owned.view_in(&arena);
        let mut overlay = Overlay::default();
        overlay.set(base, &parse("data.n"), dv(serde_json::json!(1)));
        overlay.set(base, &parse("data.m"), dv(serde_json::json!(2)));
        overlay.remove(&parse("data.n"));
        overlay.set(base, &parse("data.n"), dv(serde_json::json!(3)));
        assert_eq!(spliced(&overlay, base), r#"{"data":{"a":1,"m":2,"n":3}}"#);
    }

    #[test]
    fn padding_and_a_removed_element_are_read_as_written() {
        let owned = dv(serde_json::json!({"data": {"list": [1, 2]}}));
        let arena = Bump::new();
        let base = owned.view_in(&arena);
        let mut overlay = Overlay::default();
        overlay.set(base, &parse("data.list.4"), dv(serde_json::json!(5)));
        // The padding is not the base's to answer: it reads off the spliced
        // root, as `null`, not as missing.
        assert_eq!(overlay.lookup(&parse("data.list.3")), Lookup::Dirty);
        assert_eq!(
            spliced(&overlay, base),
            r#"{"data":{"list":[1,2,null,null,5]}}"#
        );
        overlay.remove(&parse("data.list.0"));
        assert_eq!(
            spliced(&overlay, base),
            r#"{"data":{"list":[2,null,null,5]}}"#
        );
    }
}
