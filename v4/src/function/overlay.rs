//! A host call's overlay: the writes one call makes, kept apart from the run
//! until the call settles (05, "A host `Function`").
//!
//! As built it is the run log's trie, [`Writes`] (I87: Task 2 measures a
//! plain list against it), behind the operations [`Ctx`](super::Ctx) uses,
//! so a change of structure touches this file only.
//!
//! The run log may hold a mark with no value, which its view supplies at the
//! checkpoint. An overlay has no view of its own, so every mark it makes owns
//! its value. A write is resolved against the call's own root, the base with
//! the overlay spliced on (I87), and marked where it starts creating, with
//! what it creates. Three writes the trie cannot hold by value at their own
//! path are marked as a write of the container they change:
//!
//! - an element written past an array's end, which pads the array with
//!   `null`s no node stands for;
//! - an element removed, which shifts the elements after it;
//! - a key written again after this call removed it, which moves to the end
//!   of its object.
//!
//! So the marks stay disjoint and owned, and splicing or applying them in
//! the trie's order is the writes applied in the order they were made.

use crate::expr::arena::with_arena;
use crate::expr::path;
use crate::expr::{Path, Seg};
use crate::state::splice;
use crate::state::{At, Lookup, Mark, RunContext, Writes};
use bumpalo::Bump;
use datavalue::{DataValue, OwnedDataValue};

/// The writes of one call, in the order they were made.
#[derive(Debug, Default)]
pub(crate) struct Overlay(Writes);

/// Where a write is marked, and with what: the path's resolved kinds, how
/// many of its segments the mark sits at, and the mark.
type Planned = (u64, usize, Mark);

impl Overlay {
    /// Whether the call has written nothing.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Writes `value` at `path` over `base`, the root the call reads beneath
    /// its own writes. A write that cannot land, below a scalar, does
    /// nothing, as `Path::set` does.
    pub(crate) fn set(&mut self, base: DataValue<'_>, path: &Path, value: OwnedDataValue) {
        let segs = path.segs();
        if let Some((kinds, len, mark)) = self.over(base, |o, root| o.plan_set(root, segs, value)) {
            self.0.mark(At::new(&segs[..len], kinds), mark);
        }
    }

    /// Removes `path` over `base`. Nothing there is nothing to do.
    pub(crate) fn remove(&mut self, base: DataValue<'_>, path: &Path) {
        let segs = path.segs();
        if let Some((kinds, len, mark)) = self.over(base, |o, root| o.plan_remove(root, segs)) {
            self.0.mark(At::new(&segs[..len], kinds), mark);
        }
    }

    /// What a read of `path` over `base` finds here: `Owned` written here
    /// (`None` when removed), `Dirty` when written below it, so the read
    /// splices, or `Clean` when the overlay has nothing at, above or below
    /// it, and the base answers.
    pub(crate) fn lookup<'o>(&'o self, base: DataValue<'_>, path: &Path) -> Lookup<'o> {
        if self.is_empty() {
            return Lookup::Clean;
        }
        self.0
            .lookup(At::new(path.segs(), kinds(base, path.segs())))
    }

    /// `root` with every write spliced on, in order, in `arena`. The values
    /// are viewed, not copied.
    pub(crate) fn splice_onto<'a>(&'a self, root: DataValue<'a>, arena: &'a Bump) -> DataValue<'a> {
        let mut root = root;
        self.0.visit(&mut |hops, mark| {
            let segs = splice::segs_of(hops);
            root = match mark {
                Mark::Set(Some(value)) => {
                    let spliced = splice::set(arena, root, &segs, value.view_in(arena));
                    debug_assert!(spliced.is_some(), "an overlay's write lands on its base");
                    spliced.unwrap_or(root)
                }
                // A path this call created and then removed is not in the
                // base.
                Mark::Removed => splice::remove(arena, root, &segs).unwrap_or(root),
                Mark::Set(None) | Mark::Appended { .. } => {
                    unreachable!("an overlay's marks own their values")
                }
            };
        });
        root
    }

    /// Applies every write, in order, through `rc`, whose view is the base
    /// the call read. A write is not recorded again: the call recorded it.
    pub(crate) fn apply(mut self, rc: &mut RunContext<'_, '_>) {
        if self.is_empty() {
            return;
        }
        let drained = self.0.drain();
        debug_assert!(drained.reorder.is_empty(), "an overlay flags no reorder");
        for dirty in drained.dirty {
            let segs = splice::segs_of(&dirty.at);
            match dirty.mark {
                Mark::Set(Some(value)) => {
                    rc.set_segs(&segs, value, None);
                }
                Mark::Removed => {
                    rc.remove_segs(&segs);
                }
                Mark::Set(None) | Mark::Appended { .. } => {
                    unreachable!("an overlay's marks own their values")
                }
            }
        }
    }

    /// `f` with the call's own root: `base` while nothing is written, else
    /// `base` with the overlay spliced on, in a pooled arena.
    fn over<R>(&self, base: DataValue<'_>, f: impl FnOnce(&Self, DataValue<'_>) -> R) -> R {
        if self.is_empty() {
            return f(self, base);
        }
        with_arena(|arena| f(self, self.splice_onto(base, arena)))
    }

    /// Where a write of `value` at `segs` over the call's `root` is marked.
    fn plan_set(
        &self,
        root: DataValue<'_>,
        segs: &[Seg],
        value: OwnedDataValue,
    ) -> Option<Planned> {
        let r = splice::resolve(root, segs)?;
        let at = At::new(segs, r.kinds);
        let (len, value) = match r.created {
            None => (segs.len(), value),
            Some(i) => match (at.index(i), splice::get(root, &segs[..i])) {
                // Padded: the array, since no node stands for its `null`s.
                (Some(j), Some(array @ DataValue::Array(items))) if j > items.len() => {
                    let mut array = array.to_owned();
                    path::set(&mut array, &segs[i..], value);
                    (i, array)
                }
                _ => (i + 1, path::chain(&segs[i + 1..], value)),
            },
        };
        // A key this call removed comes back at its object's end, which only
        // a write of the object says.
        let last = len - 1;
        if last > 0 && !at.is_index(last) && self.removed_here(at.prefix(len)) {
            let mut object = splice::get(root, &segs[..last])
                .expect("a removed key's object is in the root")
                .to_owned();
            path::set(&mut object, &segs[last..len], value);
            return Some((r.kinds, last, Mark::Set(Some(object))));
        }
        Some((r.kinds, len, Mark::Set(Some(value))))
    }

    /// Where a removal of `segs` from the call's `root` is marked.
    fn plan_remove(&self, root: DataValue<'_>, segs: &[Seg]) -> Option<Planned> {
        splice::get(root, segs)?;
        let r = splice::resolve(root, segs).expect("a path that holds a value resolves");
        let last = segs.len() - 1;
        if !At::new(segs, r.kinds).is_index(last) {
            return Some((r.kinds, segs.len(), Mark::Removed));
        }
        // An element's removal shifts the rest: the array, written whole.
        let mut array = splice::get(root, &segs[..last])
            .expect("an element's array is in the root")
            .to_owned();
        path::remove(&mut array, &segs[last..]);
        Some((r.kinds, last, Mark::Set(Some(array))))
    }

    /// Whether this call removed the key at `at` itself: the overlay says it
    /// is gone, and holds no value for its object, under which a missing key
    /// is merely new.
    fn removed_here(&self, at: At<'_>) -> bool {
        matches!(self.0.lookup(at), Lookup::Owned(None))
            && !matches!(self.0.lookup(at.prefix(at.len() - 1)), Lookup::Owned(_))
    }
}

/// Which of `segs` are array indices, read off `base` while it has the
/// containers and past them as `splice::resolve` reads (an index wherever one
/// parses). A lookup needs no more: a node of the overlay with no mark at or
/// above it is a container of the base, of the base's type, since a write
/// that creates or replaces one is marked there.
fn kinds(base: DataValue<'_>, segs: &[Seg]) -> u64 {
    let mut kinds = 0;
    let mut at = Some(base);
    for (i, seg) in segs.iter().enumerate() {
        at = match at {
            Some(DataValue::Object(pairs)) => {
                pairs.iter().find(|(k, _)| **k == *seg.key).map(|(_, v)| *v)
            }
            Some(DataValue::Array(items)) => seg.index.and_then(|j| {
                kinds |= 1 << i;
                items.get(j).copied()
            }),
            _ => {
                if seg.index.is_some() {
                    kinds |= 1 << i;
                }
                None
            }
        };
    }
    kinds
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
            overlay.remove(base, &path);
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
                    let got = match overlay.lookup(base, &read) {
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
        overlay.remove(base, &parse("data.a"));
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
        overlay.remove(base, &parse("data.n"));
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
        // The padding reads as `null`, not as missing.
        assert_eq!(
            overlay.lookup(base, &parse("data.list.3")),
            Lookup::Owned(Some(&OwnedDataValue::Null))
        );
        overlay.remove(base, &parse("data.list.0"));
        assert_eq!(
            spliced(&overlay, base),
            r#"{"data":{"list":[2,null,null,5]}}"#
        );
    }
}
