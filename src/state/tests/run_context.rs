//! `RunContext` against a model that writes the owned tree directly, with
//! the rules `tests/it/path.rs` and `splice_property.rs` already checked against
//! v3. Random sequences of every operation, with
//! invalidation on and off, and a garbage floor small enough that checkpoints
//! fall mid-sequence. Trees are compared as JSON, so key order counts.

use super::random::{self, exact};
use crate::expr::{Path, arena, path};
use crate::state::splice;
use crate::state::writes::fault;
use crate::state::{RunContext, Stats, Tuning};
use datavalue::OwnedDataValue;
use std::borrow::Cow;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// A path with a root, as the run takes them.
fn rooted(dotted: &str) -> Path {
    dotted.parse().expect("a rooted path")
}

#[derive(Debug)]
enum Op {
    SetOwned(String, OwnedDataValue),
    SetEval(String, OwnedDataValue, bool),
    EngineSet(String, OwnedDataValue),
    SetLarge(String),
    Remove(String),
    Append(String, Vec<OwnedDataValue>, bool),
    MergeRoot(&'static str, OwnedDataValue, bool),
    Read(String),
    Tick,
    Checkpoint,
}

fn op(rng: &mut random::Rng, model: &OwnedDataValue) -> Op {
    let p = random::rooted_path(rng, model);
    match rng.below(100) {
        0..=14 => Op::SetOwned(p, random::value(rng)),
        15..=34 => Op::SetEval(p, random::value(rng), rng.chance(1, 2)),
        35..=39 => Op::EngineSet(p, random::value(rng)),
        40..=41 => Op::SetLarge(p),
        42..=56 => Op::Remove(p),
        57..=69 => {
            let n = rng.below(3);
            Op::Append(
                p,
                (0..n).map(|_| random::value(rng)).collect(),
                rng.chance(1, 2),
            )
        }
        70..=74 => Op::MergeRoot(
            rng.pick(&["data", "metadata", "temp_data", "a"]),
            random::value(rng),
            rng.chance(1, 2),
        ),
        75..=86 => Op::Read(p),
        87..=95 => Op::Tick,
        _ => Op::Checkpoint,
    }
}

/// A value large enough to trip the invalidation rule.
fn large() -> OwnedDataValue {
    OwnedDataValue::Array((0..3000).map(OwnedDataValue::from).collect())
}

fn json(v: Option<Cow<'_, OwnedDataValue>>) -> Option<String> {
    v.map(|v| exact(&v))
}

fn run(name: &str, tuning: Tuning, seed: u64, cases: usize) -> Stats {
    let mut rng = random::Rng::new(seed);
    let mut total = Stats::default();
    for case in 0..cases {
        let mut model = random::rooted_tree(&mut rng, 4);
        let mut slot = model.clone();
        let mut touched: Vec<String> = Vec::new();
        {
            let mut rc = RunContext::new(&mut slot, tuning);
            let steps = 1 + rng.below(60);
            for step in 0..steps {
                let op = op(&mut rng, &model);
                let at = format!("{name}: case {case}, step {step}, {op:?}");
                match op {
                    Op::SetOwned(p, v) => {
                        let path = rooted(&p);
                        assert_eq!(
                            rc.set(&path, v.clone(), None),
                            path.assign(&mut model, v),
                            "{at}"
                        );
                        touched.push(p);
                    }
                    Op::SetEval(p, v, owned) => {
                        let path = rooted(&p);
                        let wrote = rc.edit(|e| {
                            let av = v.to_arena(e.arena());
                            e.set(&path, av, owned.then(|| v.clone()))
                        });
                        assert_eq!(wrote, path.assign(&mut model, v), "{at}");
                        touched.push(p);
                    }
                    Op::EngineSet(p, v) => {
                        let path = rooted(&p);
                        let wrote = rc.engine_set(&path, || v.clone(), |o| v.to_arena(&o.arena));
                        assert_eq!(wrote, path.assign(&mut model, v), "{at}");
                        touched.push(p);
                    }
                    Op::SetLarge(p) => {
                        let path = rooted(&p);
                        assert_eq!(
                            rc.set(&path, large(), None),
                            path.assign(&mut model, large()),
                            "{at}"
                        );
                        touched.push(p);
                    }
                    Op::Remove(p) => {
                        let path = rooted(&p);
                        assert_eq!(
                            rc.remove_segs(path.segs()),
                            path::remove(&mut model, path.segs()).is_some(),
                            "{at}"
                        );
                        touched.push(p);
                    }
                    Op::Append(p, items, owned) => {
                        let path = rooted(&p);
                        let got = rc.edit(|e| {
                            let av: Vec<_> = items.iter().map(|v| v.to_arena(e.arena())).collect();
                            e.append(&path, &av, owned.then(|| items.clone()))
                        });
                        let want = path::append(&mut model, path.segs(), items);
                        assert_eq!(got, want, "{at}");
                        touched.push(p);
                    }
                    Op::MergeRoot(key, v, owned) => {
                        rc.edit(|e| {
                            let av = v.to_arena(e.arena());
                            e.merge_root(key, av, owned.then(|| v.clone()))
                        });
                        crate::expr::path::merge_root(&mut model, key, v);
                        touched.push(key.to_string());
                    }
                    Op::Read(p) => {
                        let path = rooted(&p);
                        let got = rc.read(|_, root| {
                            splice::get(*root, path.segs()).map(|v| exact(&v.to_owned()))
                        });
                        assert_eq!(got, path.get(&model).map(exact), "{at}");
                    }
                    Op::Tick => rc.tick(),
                    Op::Checkpoint => rc.checkpoint(),
                }
                // Reads that build no view: the touched paths and a few random
                // ones every step, and each root (the whole context, as far as a
                // rooted `Path` reaches) every 10th step and at the last, since
                // converting whole roots is what makes this test slow.
                let mut probes: Vec<String> = touched
                    .iter()
                    .rev()
                    .take(8)
                    .filter(|t| t.parse::<Path>().is_ok())
                    .cloned()
                    .collect();
                probes.extend((0..3).map(|_| random::rooted_path(&mut rng, &model)));
                if step % 10 == 0 || step + 1 == steps {
                    probes.extend(random::ROOTS.iter().map(|r| (*r).to_string()));
                }
                for p in probes {
                    let path = rooted(&p);
                    let (got, want) = (json(rc.get(&path)), path.get(&model).map(exact));
                    if got != want {
                        let view = rc.read(|_, r| exact(&r.to_owned()));
                        let (g, w) = (got.unwrap_or_default(), want.unwrap_or_default());
                        panic!(
                            "{at}: get {p:?} {}\n view against the model: {}",
                            random::diff(&g, &w),
                            random::diff(&view, &exact(&model))
                        );
                    }
                }
            }
            if rng.chance(1, 4) {
                // A run dropped early leaves every write in the message.
                drop(rc);
            } else {
                let stats = rc.finish();
                total.views += stats.views;
                total.checkpoints += stats.checkpoints;
                total.invalidations += stats.invalidations;
                total.converted += stats.converted;
                total.live_max = total.live_max.max(stats.live_max);
                total.peak_arena = total.peak_arena.max(stats.peak_arena);
            }
        }
        let (got, want) = (exact(&slot), exact(&model));
        assert!(
            got == want,
            "{name}: case {case}, the final context {}",
            random::diff(&got, &want)
        );
    }
    total
}

fn tunings() -> [(&'static str, Tuning); 4] {
    let small = Tuning {
        garbage_floor: 256,
        ..Tuning::default()
    };
    [
        ("default", Tuning::default()),
        (
            "no invalidation",
            Tuning {
                invalidate: false,
                ..Tuning::default()
            },
        ),
        ("small garbage floor", small),
        (
            "small floor, no invalidation",
            Tuning {
                invalidate: false,
                ..small
            },
        ),
    ]
}

#[test]
fn the_run_matches_the_model() {
    let deep = random::deep();
    // State is feature-independent: run in the default build, or whenever
    // `DATAFLOW_DEEP` is set, rather than repeating in every operator-family
    // feature build.
    if !deep
        && (!crate::expr::FAMILIES.is_empty() || cfg!(any(feature = "tensor", feature = "budget")))
    {
        return;
    }
    // Sequences per tuning. Was 2_000 (about 90 s in a debug build); 150 keeps
    // the test near 4 s, and the default seeds (`0x1000 + i`) are unchanged,
    // so the first 150 sequences are the same ones as before.
    // `DATAFLOW_DEEP=1 cargo test --lib state::` runs the full 2_000, with the
    // original thresholds.
    let (sequences, floor, invalidations) = if deep {
        (2_000, 1000, 50)
    } else {
        (150, 75, 3)
    };
    let base_seed = random::seed(0x1000);
    for (i, (name, tuning)) in tunings().into_iter().enumerate() {
        let seed = base_seed.wrapping_add(i as u64);
        let stats = run(name, tuning, seed, sequences);
        assert!(
            stats.checkpoints > floor && stats.views > floor,
            "{name} (seed={seed:#x}): {stats:?}"
        );
        if tuning.invalidate {
            assert!(
                stats.invalidations > invalidations,
                "{name} (seed={seed:#x}): {stats:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Targeted
// ---------------------------------------------------------------------------

fn context() -> OwnedDataValue {
    let mut v = OwnedDataValue::Object(vec![]);
    for root in ["data", "metadata", "temp_data"] {
        rooted(root).assign(&mut v, OwnedDataValue::Object(vec![]));
    }
    v
}

#[test]
fn writes_alone_build_no_view() {
    let mut slot = context();
    let mut rc = RunContext::new(&mut slot, Tuning::default());
    for i in 0..10 {
        rc.set(
            &rooted(&format!("data.k{i}")),
            OwnedDataValue::from(i),
            None,
        );
    }
    assert_eq!(rc.stats().views, 0);
    rc.read(|_, _| ());
    assert_eq!(rc.stats().views, 1);
    rc.finish();
    assert_eq!(slot["data"]["k9"], OwnedDataValue::from(9));
}

#[test]
fn a_large_owned_write_invalidates_the_view() {
    let mut slot = context();
    let mut rc = RunContext::new(&mut slot, Tuning::default());
    rc.read(|_, _| ());
    rc.set(&rooted("data.big"), large(), None);
    assert_eq!(rc.stats().invalidations, 1);
    assert!(!rc.has_view(), "the next read rebuilds it");
    rc.set(&rooted("data.small"), OwnedDataValue::from(1), None);
    rc.read(|_, _| ());
    rc.set(&rooted("data.small"), OwnedDataValue::from(2), None);
    assert_eq!(rc.stats().invalidations, 1, "a small write is copied in");
    rc.finish();
}

#[test]
fn the_garbage_trigger_fires_past_its_threshold_and_not_before() {
    let floor = 16 << 10;
    let mut slot = context();
    let mut rc = RunContext::new(
        &mut slot,
        Tuning {
            garbage_floor: floor,
            ..Tuning::default()
        },
    );
    let live = rc.read(|o, _| arena::used(&o.arena));
    let path = rooted("data.counter");
    let mut fired = false;
    for i in 0..10_000 {
        rc.edit(|e| {
            let v = OwnedDataValue::from(i).to_arena(e.arena());
            e.set(&path, v, None)
        });
        let garbage = rc.read(|o, _| arena::used(&o.arena)) - live;
        let before = rc.stats().checkpoints;
        rc.tick();
        let after = rc.stats().checkpoints;
        if garbage > live.max(floor) {
            assert_eq!(after, before + 1, "fires at {garbage} bytes");
            fired = true;
            break;
        }
        assert_eq!(after, before, "holds at {garbage} bytes");
    }
    assert!(fired);
    rc.finish();
}

#[test]
fn a_run_dropped_while_panicking_keeps_the_last_checkpoint() {
    let mut slot = context();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let mut rc = RunContext::new(&mut slot, Tuning::default());
        rc.edit(|e| {
            let v = OwnedDataValue::from(1).to_arena(e.arena());
            e.set(&rooted("data.kept"), v, None)
        });
        rc.checkpoint();
        rc.edit(|e| {
            let v = OwnedDataValue::from(2).to_arena(e.arena());
            e.set(&rooted("data.lost"), v, None)
        });
        panic!("a handler panicked");
    }));
    assert!(outcome.is_err());
    assert_eq!(slot["data"]["kept"], OwnedDataValue::from(1));
    assert!(
        slot["data"].get("lost").is_none(),
        "no checkpoint while panicking"
    );
}

#[test]
fn a_read_borrows_unless_the_path_changed() {
    let mut slot = context();
    rooted("data.a").assign(&mut slot, OwnedDataValue::from("x"));
    let mut rc = RunContext::new(&mut slot, Tuning::default());
    rc.edit(|e| {
        let v = OwnedDataValue::from(1).to_arena(e.arena());
        e.set(&rooted("data.b"), v, None)
    });
    assert!(matches!(rc.get(&rooted("data.a")), Some(Cow::Borrowed(_))));
    assert!(matches!(rc.get(&rooted("data.b")), Some(Cow::Owned(_))));
    rc.finish();
}

/// A wide object whose keys were removed and written again takes the view's
/// key order back at the checkpoint: each key written again moves last, in
/// the order it came back, and a key only removed is gone.
#[test]
fn a_wide_object_takes_the_views_key_order_back() {
    const WIDE: usize = 1_000;
    let mut slot = context();
    let keys = (0..WIDE)
        .map(|i| (format!("k{i}"), OwnedDataValue::from(i)))
        .collect();
    rooted("data.wide").assign(&mut slot, OwnedDataValue::Object(keys));
    let mut rc = RunContext::new(&mut slot, Tuning::default());
    rc.read(|_, _| ());
    for key in ["k500", "k10", "k999"] {
        assert!(
            rc.remove_segs(rooted(&format!("data.wide.{key}")).segs()),
            "{key}"
        );
    }
    for key in ["k10", "k500"] {
        rc.set(
            &rooted(&format!("data.wide.{key}")),
            OwnedDataValue::from(key),
            None,
        );
    }
    assert!(rc.has_view(), "both writes were copied into the view");
    rc.finish();
    let OwnedDataValue::Object(pairs) = &slot["data"]["wide"] else {
        panic!("data.wide is an object");
    };
    let order: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
    let mut want: Vec<String> = (0..WIDE)
        .filter(|i| ![10, 500, 999].contains(i))
        .map(|i| format!("k{i}"))
        .collect();
    want.extend(["k10".to_owned(), "k500".to_owned()]);
    assert_eq!(order, want);
    assert_eq!(slot["data"]["wide"]["k500"], OwnedDataValue::from("k500"));
    assert_eq!(slot["data"]["wide"]["k11"], OwnedDataValue::from(11));
}

/// Keys that share a hash, written, read and removed through the whole run
/// context: each keeps its own value across a checkpoint.
#[test]
fn keys_that_share_a_hash_keep_their_own_values() {
    let mut slot = context();
    rooted("data.liquid").assign(&mut slot, OwnedDataValue::from("old"));
    let mut rc = RunContext::new(&mut slot, Tuning::default());
    rc.edit(|e| {
        let v = OwnedDataValue::from(1).to_arena(e.arena());
        e.set(&rooted("data.costarring"), v, None);
    });
    let read = |rc: &RunContext<'_, '_>, p: &str| json(rc.get(&rooted(p)));
    assert_eq!(read(&rc, "data.costarring"), Some("1".into()));
    assert_eq!(read(&rc, "data.liquid"), Some(r#""old""#.into()));
    rc.set(&rooted("data.liquid"), OwnedDataValue::from(2), None);
    assert_eq!(read(&rc, "data.costarring"), Some("1".into()));
    assert_eq!(read(&rc, "data.liquid"), Some("2".into()));
    rc.checkpoint();
    // The view again, so the removal is marked beside its twin.
    rc.read(|_, _| ());
    rc.remove_segs(rooted("data.costarring").segs());
    assert_eq!(read(&rc, "data.costarring"), None);
    assert_eq!(read(&rc, "data.liquid"), Some("2".into()));
    rc.finish();
    assert_eq!(exact(&slot["data"]), r#"{"liquid":2}"#);
}

#[test]
fn an_edit_reads_the_view_it_writes() {
    let mut slot = context();
    rooted("data.a").assign(&mut slot, OwnedDataValue::from("x"));
    let mut rc = RunContext::new(&mut slot, Tuning::default());
    rc.edit(|e| {
        assert_eq!(
            e.get(&rooted("data.a")).map(|v| exact(&v.to_owned())),
            Some(r#""x""#.to_string())
        );
        let v = OwnedDataValue::from(7).to_arena(e.arena());
        e.set(&rooted("data.b"), v, None);
        assert_eq!(
            e.get(&rooted("data.b")).map(|v| exact(&v.to_owned())),
            Some("7".to_string())
        );
        assert!(e.get(&rooted("data.c")).is_none());
        assert_eq!(
            exact(&e.root_ref().to_owned()),
            exact(&e.root().to_owned()),
            "root_ref and root are the same view"
        );
    });
    rc.finish();
    assert_eq!(slot["data"]["b"], OwnedDataValue::from(7));
}

#[test]
fn stats_report_the_view_and_the_arena() {
    let mut slot = context();
    let mut rc = RunContext::new(&mut slot, Tuning::default());
    assert_eq!(rc.stats().live_max, 0);
    rc.read(|_, _| ());
    let live = rc.stats().live_max;
    assert!(live > 0, "a view has spines");
    let path = rooted("data.counter");
    for i in 0..200 {
        rc.edit(|e| {
            let v = OwnedDataValue::from(i).to_arena(e.arena());
            e.set(&path, v, None)
        });
        rc.tick();
    }
    let stats = rc.finish();
    assert!(stats.peak_arena >= live, "{stats:?}");
}

/// A panic inside the checkpoint's `settle` or `apply` must not lose the
/// message's context: the slot holds an object with the three roots
/// afterwards, never `null`.
#[test]
fn a_panic_inside_a_checkpoint_keeps_the_context() {
    for phase in ["settle", "apply"] {
        let mut slot = context();
        rooted("data.kept").assign(&mut slot, OwnedDataValue::from(1));
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let mut rc = RunContext::new(&mut slot, Tuning::default());
            rc.edit(|e| {
                let v = OwnedDataValue::from(2).to_arena(e.arena());
                e.set(&rooted("data.pending"), v, None)
            });
            fault::arm(phase);
            rc.checkpoint();
        }));
        assert!(outcome.is_err(), "{phase}: the fault fired");
        let OwnedDataValue::Object(roots) = &slot else {
            panic!("{phase}: the slot holds {slot:?}, not the context");
        };
        let names: Vec<&str> = roots.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, random::ROOTS, "{phase}");
        assert_eq!(slot["data"]["kept"], OwnedDataValue::from(1), "{phase}");
    }
}
