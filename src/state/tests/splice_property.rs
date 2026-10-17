//! `view(apply(c, w)) == splice(view(c), w)`: every splice against the owned
//! write it stands for, on random trees, one write at a time and in random
//! sequences over an already-spliced root.
//!
//! The owned append rule in `expr::path` is checked here too, against a
//! reference built from the owned read and write, and the root merge
//! against its reference.

use super::random;
use crate::expr::path;
use crate::state::splice;
use bumpalo::Bump;
use datavalue::{DataValue, OwnedDataValue};

#[derive(Debug)]
enum Op {
    Set(String, OwnedDataValue),
    Remove(String),
    Append(String, Vec<OwnedDataValue>),
    MergeRoot(&'static str, OwnedDataValue),
}

fn op(rng: &mut random::Rng, tree: &OwnedDataValue) -> Op {
    match rng.below(10) {
        0..=4 => Op::Set(random::path(rng, tree), random::value(rng)),
        5 | 6 => Op::Remove(random::path(rng, tree)),
        7 | 8 => {
            let n = rng.below(3);
            Op::Append(
                random::path(rng, tree),
                (0..n).map(|_| random::value(rng)).collect(),
            )
        }
        _ => Op::MergeRoot(
            rng.pick(&["data", "metadata", "temp_data", "a"]),
            random::value(rng),
        ),
    }
}

/// The append rule (3.x's `append_at` and `append_owned`): `Err` for a
/// target that is neither missing, `null` nor an array; otherwise extend or
/// create, as one owned read and one owned write.
fn reference_append(
    context: &mut OwnedDataValue,
    segs: &[crate::expr::Seg],
    items: Vec<OwnedDataValue>,
) -> Result<(), ()> {
    match path::get(context, segs) {
        None | Some(OwnedDataValue::Null) => {
            path::set(context, segs, OwnedDataValue::Array(items));
            Ok(())
        }
        Some(OwnedDataValue::Array(existing)) => {
            let mut all = existing.clone();
            all.extend(items);
            path::set(context, segs, OwnedDataValue::Array(all));
            Ok(())
        }
        Some(_) => Err(()),
    }
}

/// Applies `op` to the owned tree through `expr::path`, after checking the
/// owned rule against its reference, and to the view through `splice`. Returns the
/// new view root, or the old one when the op did nothing.
fn apply<'a>(
    case: usize,
    arena: &'a Bump,
    owned: &mut OwnedDataValue,
    view: DataValue<'a>,
    op: Op,
) -> DataValue<'a> {
    match op {
        Op::Set(dotted, value) => {
            let segs = random::segs(&dotted);
            // The hops a write resolves to agree on what does nothing, and
            // address exactly what the raw path does.
            let hops = splice::resolve(view, &segs);
            if let Some(hops) = hops.map(|r| r.hops(&segs)) {
                let mut by_hops = owned.clone();
                path::set(&mut by_hops, &splice::segs_of(&hops), value.clone());
                let mut by_path = owned.clone();
                path::set(&mut by_path, &segs, value.clone());
                assert_eq!(
                    random::exact(&by_hops),
                    random::exact(&by_path),
                    "case {case}: hops of {dotted:?} are {hops:?}"
                );
            }
            let wrote = path::set(owned, &segs, value.clone());
            let tracked = splice::set_resolved(arena, view, &segs, value.to_arena(arena));
            assert_eq!(
                tracked.map(|(_, r)| r),
                hops,
                "case {case}: one walk resolves as resolve does"
            );
            let spliced = tracked.map(|(root, _)| root);
            assert_eq!(
                spliced.is_some(),
                wrote,
                "case {case}: set {dotted:?}, no-op agreement"
            );
            assert_eq!(hops.is_some(), wrote, "case {case}: resolve {dotted:?}");
            spliced.unwrap_or(view)
        }
        Op::Remove(dotted) => {
            let segs = random::segs(&dotted);
            let removed = path::remove(owned, &segs);
            let spliced = splice::remove_resolved(arena, view, &segs);
            assert_eq!(
                spliced.is_some(),
                removed.is_some(),
                "case {case}: remove {dotted:?}"
            );
            if let Some((_, kinds)) = spliced {
                assert_eq!(
                    Some(kinds),
                    splice::resolve(view, &segs).map(|r| r.kinds),
                    "case {case}: remove {dotted:?} resolves"
                );
            }
            spliced.map_or(view, |(root, _)| root)
        }
        Op::Append(dotted, items) => {
            let segs = random::segs(&dotted);
            let mut reference = owned.clone();
            let expected = reference_append(&mut reference, &segs, items.clone());
            let got = path::append(owned, &segs, items.clone());
            assert_eq!(
                got.is_ok(),
                expected.is_ok(),
                "case {case}: append {dotted:?} ok"
            );
            assert_eq!(
                random::exact(owned),
                random::exact(&reference),
                "case {case}: owned append {dotted:?}"
            );
            let items_av: Vec<DataValue<'a>> = items.iter().map(|v| v.to_arena(arena)).collect();
            let existing = splice::get(view, &segs);
            match splice::append(arena, view, &segs, existing, &items_av) {
                Ok(Some((root, r))) => {
                    assert_eq!(
                        Some(r),
                        splice::resolve(view, &segs),
                        "case {case}: append {dotted:?} resolves"
                    );
                    root
                }
                Ok(None) => {
                    assert_eq!(got, Ok(false), "case {case}: append {dotted:?} no-op");
                    view
                }
                Err(_) => {
                    assert!(got.is_err(), "case {case}: append {dotted:?} error");
                    view
                }
            }
        }
        Op::MergeRoot(key, value) => {
            let mut reference = owned.clone();
            random::reference_merge_root(&mut reference, key, value.clone());
            crate::expr::path::merge_root(owned, key, value.clone());
            assert_eq!(
                random::exact(owned),
                random::exact(&reference),
                "case {case}: owned merge into {key}"
            );
            splice::merge_root(arena, view, key, value.to_arena(arena))
        }
    }
}

#[test]
fn every_splice_matches_the_owned_write() {
    let mut rng = random::Rng::new(0xa11ce);
    for case in 0..50_000 {
        let mut owned = random::tree(&mut rng, 4);
        let arena = Bump::new();
        let view = owned.view_in(&arena);
        let op = op(&mut rng, &owned);
        let what = format!("{op:?} on {owned:?}");
        // `view` borrows `owned`, so the owned side writes to a copy.
        let mut written = owned.clone();
        let root = apply(case, &arena, &mut written, view, op);
        assert_eq!(
            random::exact(&root.to_owned()),
            random::exact(&written),
            "case {case}: {what}"
        );
        owned = written;
        drop(owned);
    }
}

#[test]
fn sequences_of_splices_over_a_spliced_root_match() {
    let mut rng = random::Rng::new(0xb0b);
    for case in 0..5_000 {
        let start = random::tree(&mut rng, 4);
        let arena = Bump::new();
        let mut view = start.view_in(&arena);
        let mut owned = start.clone();
        for step in 0..1 + rng.below(20) {
            let op = op(&mut rng, &owned);
            view = apply(case * 100 + step, &arena, &mut owned, view, op);
            assert_eq!(
                random::exact(&view.to_owned()),
                random::exact(&owned),
                "case {case}, step {step}"
            );
        }
    }
}
