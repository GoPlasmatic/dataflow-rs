//! The write log's rules. `run_context.rs` checks the log end to end against
//! a model of the owned tree.

use super::random;
use crate::expr::Seg;
use crate::state::splice::Resolved;
use crate::state::writes::{
    At, Key, NONE, Node, WRITES_POOL, absorb, combine, read_under, removes_an_element,
};
use crate::state::{Lookup, Mark, Writes};
use datavalue::OwnedDataValue;
use list::List;

/// One resolved step of a path: what the tests read the write log's keys as.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum Hop {
    /// A key on an object.
    Key(Box<str>),
    /// An index on an array.
    Index(usize),
}

impl Hop {
    /// The segment that addresses exactly this hop: a key never reads as an
    /// index, and an index is spelled in decimal.
    pub(super) fn to_seg(&self) -> Seg {
        match self {
            Self::Key(k) => Seg::key(k),
            Self::Index(i) => Seg::index(*i),
        }
    }
}

/// The segments that address `hops` exactly.
pub(super) fn segs_of(hops: &[Hop]) -> Vec<Seg> {
    hops.iter().map(Hop::to_seg).collect()
}

impl Resolved {
    /// The hops, built: for the tests.
    pub(super) fn hops(&self, segs: &[Seg]) -> Vec<Hop> {
        At::new(segs, self.kinds).to_hops()
    }
}

impl At<'_> {
    /// Segment `i` as the hop the log keys by.
    pub(super) fn hop(&self, i: usize) -> Hop {
        match self.index(i) {
            Some(j) => Hop::Index(j),
            None => Hop::Key(self.segs()[i].key.clone()),
        }
    }

    /// Every segment as a hop.
    pub(super) fn to_hops(self) -> Vec<Hop> {
        (0..self.len()).map(|i| self.hop(i)).collect()
    }
}

/// Hops with their segments, for a caller that holds them: tests.
struct OwnedAt {
    /// The segments, each spelled so it resolves as its hop does.
    segs: Vec<Seg>,
    /// Bit `i` set when segment `i` is an index.
    kinds: u64,
}

impl OwnedAt {
    /// The borrowed form the log is asked with.
    fn at(&self) -> At<'_> {
        At::new(&self.segs, self.kinds)
    }
}

impl From<&[Hop]> for OwnedAt {
    fn from(hops: &[Hop]) -> Self {
        let kinds = hops
            .iter()
            .enumerate()
            .filter(|(_, h)| matches!(h, Hop::Index(_)))
            .fold(0u64, |k, (i, _)| k | 1 << i);
        Self {
            segs: hops.iter().map(Hop::to_seg).collect(),
            kinds,
        }
    }
}

/// One topmost marked path and what the checkpoint must do there.
#[derive(Clone, Debug, PartialEq)]
struct Dirty {
    /// Where the mark sits.
    at: Vec<Hop>,
    /// What happened there.
    mark: Mark,
}

/// Everything the checkpoint applies: the marks, in the order their paths
/// were first marked, then the objects whose key order must follow the view.
#[derive(Debug, Default, PartialEq)]
struct Drained {
    /// The topmost marks.
    dirty: Vec<Dirty>,
    /// The objects whose key order must follow the view.
    reorder: Vec<Vec<Hop>>,
}

/// The capacities, nodes and name bytes, of the storage this thread's pool
/// holds: for the test of what is pooled.
fn pooled() -> Vec<(usize, usize)> {
    WRITES_POOL.with(|p| {
        p.borrow()
            .iter()
            .map(|(nodes, names)| (nodes.capacity(), names.capacity()))
            .collect()
    })
}

impl Writes {
    /// Every topmost mark under `n`, with its path built: for `visit`.
    fn each(&self, n: u32, at: &mut Vec<Hop>, f: &mut dyn FnMut(&[Hop], &Node)) {
        let node = &self.nodes[n as usize];
        if node.mark.is_some() || node.reorder {
            f(at, node);
            if node.mark.is_some() {
                return;
            }
        }
        let mut c = node.first;
        while c != NONE {
            at.push(self.hop(self.nodes[c as usize].key));
            self.each(c, at, f);
            at.pop();
            c = self.nodes[c as usize].next;
        }
    }

    /// The hop a node's key names.
    fn hop(&self, key: Key) -> Hop {
        match key {
            Key::Name { start, len, .. } => Hop::Key(self.name(start, len).into()),
            Key::Index(i) => Hop::Index(i),
            Key::Root => unreachable!("the root has no key"),
        }
    }

    /// Every topmost mark, without draining: what the tests read the log's
    /// marks through.
    fn visit(&self, f: &mut dyn FnMut(&[Hop], &Mark)) {
        self.each(0, &mut Vec::new(), &mut |at, node| {
            if let Some(mark) = &node.mark {
                f(at, mark);
            }
        });
    }

    /// The topmost marks and the reorders, leaving the log empty: what the
    /// tests read the log's marks through.
    fn drain(&mut self) -> Drained {
        fn walk(log: &mut Writes, n: u32, at: &mut Vec<Hop>, out: &mut Drained) {
            if let Some(mark) = log.nodes[n as usize].mark.take() {
                return out.dirty.push(Dirty {
                    at: at.clone(),
                    mark,
                });
            }
            if log.nodes[n as usize].reorder {
                out.reorder.push(at.clone());
            }
            let mut c = log.nodes[n as usize].first;
            while c != NONE {
                at.push(log.hop(log.nodes[c as usize].key));
                walk(log, c, at, out);
                at.pop();
                c = log.nodes[c as usize].next;
            }
        }
        let mut out = Drained::default();
        walk(self, 0, &mut Vec::new(), &mut out);
        self.reset();
        out
    }
}

/// The measured alternative to [`Writes`], kept for the tests only: a flat
/// list of disjoint marks, scanned on every call. `the_trie_and_the_list_agree_on_random_marks`
/// holds the trie to it.
mod list {
    use super::{
        At, Dirty, Drained, Hop, Lookup, Mark, absorb, combine, read_under, removes_an_element,
    };

    impl At<'_> {
        /// Whether segment `i` is `hop`.
        fn matches(&self, i: usize, hop: &Hop) -> bool {
            match hop {
                Hop::Index(j) => self.index(i) == Some(*j),
                Hop::Key(k) => !self.is_index(i) && *self.segs()[i].key == **k,
            }
        }

        /// Whether `hops` is this path or a prefix of it.
        fn under(&self, hops: &[Hop]) -> bool {
            hops.len() <= self.len() && hops.iter().enumerate().all(|(i, h)| self.matches(i, h))
        }

        /// Whether this path is a strict prefix of `hops`.
        fn above(&self, hops: &[Hop]) -> bool {
            hops.len() > self.len() && (0..self.len()).all(|i| self.matches(i, &hops[i]))
        }
    }

    /// The write log as a flat list of disjoint marks, scanned on every call:
    /// the measured alternative to [`Writes`].
    #[derive(Default, Debug)]
    pub(super) struct List {
        entries: Vec<(Vec<Hop>, Mark)>,
        reorder: Vec<Vec<Hop>>,
    }

    impl List {
        /// Marks `at` as `Writes::mark` does, by scanning the list.
        pub(super) fn mark(&mut self, at: At<'_>, new: Mark) {
            debug_assert!(!at.is_empty(), "a write has a path");
            // A strict ancestor absorbs the write.
            if let Some((path, above)) = self
                .entries
                .iter_mut()
                .find(|(p, _)| p.len() < at.len() && at.under(p))
            {
                let depth = path.len();
                return absorb(above, at, depth, new);
            }
            // An array element's removal rewrites the array (as `Writes::mark`).
            if removes_an_element(at, &new) {
                return self.mark(at.prefix(at.len() - 1), Mark::Set(None));
            }
            // A reorder flag below counts as a child, as a trie node it creates
            // does; one at the path itself is cleared by any mark.
            let has_children = self.entries.iter().any(|(p, _)| at.above(p))
                || self.reorder.iter().any(|p| at.above(p));
            self.reorder
                .retain(|p| !(p.len() == at.len() && at.under(p)));
            // Combined in place, so a path keeps the position it was first
            // marked at, as the trie's child order does.
            let place = self
                .entries
                .iter()
                .position(|(p, _)| p.len() == at.len() && at.under(p));
            let old = place.map(|i| std::mem::replace(&mut self.entries[i].1, Mark::Removed));
            let mark = combine(old, new, has_children);
            if !matches!(mark, Mark::Appended { .. }) || has_children {
                self.entries.retain(|(p, _)| !at.above(p));
                self.reorder.retain(|p| !at.above(p));
            }
            match self
                .entries
                .iter()
                .position(|(p, _)| p.len() == at.len() && at.under(p))
            {
                Some(i) => self.entries[i].1 = mark,
                None => self.entries.push((at.to_hops(), mark)),
            }
        }

        /// Flags the object at `at` for a reorder.
        pub(super) fn reorder(&mut self, at: At<'_>) {
            let covered = self.entries.iter().any(|(p, _)| at.under(p));
            if !covered
                && !self
                    .reorder
                    .iter()
                    .any(|p| p.len() == at.len() && at.under(p))
            {
                self.reorder.push(at.to_hops());
            }
        }

        /// Every mark.
        pub(super) fn visit(&self, f: &mut dyn FnMut(&[Hop], &Mark)) {
            for (at, mark) in &self.entries {
                f(at, mark);
            }
        }

        /// What a read of `at` finds.
        pub(super) fn lookup(&self, at: At<'_>) -> Lookup<'_> {
            if let Some((path, mark)) = self.entries.iter().find(|(p, _)| at.under(p)) {
                return read_under(mark, &at.segs()[path.len()..]);
            }
            let below =
                |p: &Vec<Hop>| p.len() >= at.len() && (0..at.len()).all(|i| at.matches(i, &p[i]));
            if self.entries.iter().any(|(p, _)| below(p)) || self.reorder.iter().any(below) {
                Lookup::Dirty
            } else {
                Lookup::Clean
            }
        }

        /// The marks and reorders, leaving the list empty.
        pub(super) fn drain(&mut self) -> Drained {
            Drained {
                dirty: self
                    .entries
                    .drain(..)
                    .map(|(at, mark)| Dirty { at, mark })
                    .collect(),
                reorder: std::mem::take(&mut self.reorder),
            }
        }

        /// Whether nothing is marked and no reorder is flagged.
        pub(super) fn is_empty(&self) -> bool {
            self.entries.is_empty() && self.reorder.is_empty()
        }
    }
}

/// The log's view of a path given as hops.
fn p(hops: &[Hop]) -> OwnedAt {
    OwnedAt::from(hops)
}

fn k(s: &str) -> Hop {
    Hop::Key(s.into())
}
fn i(n: usize) -> Hop {
    Hop::Index(n)
}
fn n(v: i64) -> OwnedDataValue {
    OwnedDataValue::from(v)
}
fn owned(v: OwnedDataValue) -> Mark {
    Mark::Set(Some(v))
}

/// A lookup's verdict as text, an owned value as JSON so key order counts.
fn lookup_text(l: Lookup<'_>) -> String {
    match l {
        Lookup::Owned(v) => format!("Owned({:?})", v.map(random::exact)),
        other => format!("{other:?}"),
    }
}

/// A mark as text, its owned values as JSON, so key order counts.
fn text(m: &Mark) -> String {
    match m {
        Mark::Set(v) => format!("Set({:?})", v.as_ref().map(random::exact)),
        Mark::Removed => "Removed".to_string(),
        Mark::Appended { from, owned } => format!(
            "Appended({from}, {:?})",
            owned
                .as_ref()
                .map(|items| items.iter().map(random::exact).collect::<Vec<_>>())
        ),
    }
}

/// Drained marks as text, in order: paths and [`text`] of each mark.
fn texts(d: &[Dirty]) -> Vec<String> {
    d.iter()
        .map(|d| format!("{:?} {}", d.at, text(&d.mark)))
        .collect()
}

fn sorted(mut d: Vec<Dirty>) -> Vec<Dirty> {
    d.sort_by(|a, b| a.at.cmp(&b.at));
    d
}

#[test]
fn the_log_follows_the_rules() {
    // A write marks its path; reads above, at and below see it.
    let mut w = Writes::default();
    w.mark(p(&[k("data"), k("a")]).at(), Mark::Set(None));
    assert_eq!(w.lookup(p(&[k("data"), k("a")]).at()), Lookup::Dirty);
    assert_eq!(
        w.lookup(p(&[k("data"), k("a"), k("x")]).at()),
        Lookup::Dirty
    );
    assert_eq!(
        w.lookup(p(&[k("data")]).at()),
        Lookup::Dirty,
        "a parent with a written child"
    );
    assert_eq!(
        w.lookup(p(&[k("data"), k("b")]).at()),
        Lookup::Clean,
        "a sibling"
    );
    assert_eq!(w.lookup(p(&[k("metadata")]).at()), Lookup::Clean);

    // An owned mark answers reads itself, below it too.
    let mut w = Writes::default();
    w.mark(p(&[k("data"), k("a")]).at(), owned(n(1)));
    assert_eq!(
        w.lookup(p(&[k("data"), k("a")]).at()),
        Lookup::Owned(Some(&n(1)))
    );
    assert_eq!(
        w.lookup(p(&[k("data"), k("a"), k("x")]).at()),
        Lookup::Owned(None)
    );

    // An owned write below an owned mark is applied into its value.
    let obj = OwnedDataValue::Object(vec![("x".into(), n(1))]);
    let mut w = Writes::default();
    w.mark(p(&[k("data"), k("a")]).at(), owned(obj));
    w.mark(p(&[k("data"), k("a"), k("y")]).at(), owned(n(2)));
    assert_eq!(
        w.lookup(p(&[k("data"), k("a"), k("y")]).at()),
        Lookup::Owned(Some(&n(2)))
    );
    let want = OwnedDataValue::Object(vec![("x".into(), n(1)), ("y".into(), n(2))]);
    assert_eq!(
        texts(&w.drain().dirty),
        texts(&[Dirty {
            at: vec![k("data"), k("a")],
            mark: owned(want)
        }]),
        "by JSON text, so the key order counts"
    );
    assert!(w.is_empty());

    // An unowned write below an owned mark makes it read from the view.
    let mut w = Writes::default();
    w.mark(
        p(&[k("data"), k("a")]).at(),
        owned(OwnedDataValue::Object(vec![])),
    );
    w.mark(p(&[k("data"), k("a"), k("y")]).at(), Mark::Set(None));
    assert_eq!(
        w.drain().dirty,
        vec![Dirty {
            at: vec![k("data"), k("a")],
            mark: Mark::Set(None)
        }]
    );

    // A write over marked descendants drops them.
    let mut w = Writes::default();
    w.mark(p(&[k("data"), k("a"), k("x")]).at(), owned(n(1)));
    w.mark(p(&[k("data"), k("a"), k("y")]).at(), Mark::Set(None));
    w.mark(p(&[k("data"), k("a")]).at(), owned(n(3)));
    assert_eq!(
        w.drain().dirty,
        vec![Dirty {
            at: vec![k("data"), k("a")],
            mark: owned(n(3))
        }]
    );

    // A removal reads as absent; a write below it brings the path back.
    let mut w = Writes::default();
    w.mark(p(&[k("data"), k("a")]).at(), Mark::Removed);
    assert_eq!(
        w.lookup(p(&[k("data"), k("a"), k("x")]).at()),
        Lookup::Owned(None)
    );
    w.mark(p(&[k("data"), k("a"), k("x")]).at(), owned(n(1)));
    assert_eq!(
        w.drain().dirty,
        vec![Dirty {
            at: vec![k("data"), k("a")],
            mark: Mark::Set(None)
        }]
    );

    // An array element's removal rewrites the array.
    let mut w = Writes::default();
    w.mark(p(&[k("data"), k("xs"), i(2)]).at(), Mark::Removed);
    assert_eq!(
        w.drain().dirty,
        vec![Dirty {
            at: vec![k("data"), k("xs")],
            mark: Mark::Set(None)
        }]
    );

    // ...unless an owned ancestor applies it, shifting the tail.
    let xs = OwnedDataValue::Array(vec![n(0), n(1), n(2)]);
    let mut w = Writes::default();
    w.mark(p(&[k("data"), k("xs")]).at(), owned(xs));
    w.mark(p(&[k("data"), k("xs"), i(1)]).at(), Mark::Removed);
    let want = OwnedDataValue::Array(vec![n(0), n(2)]);
    assert_eq!(
        w.drain().dirty,
        vec![Dirty {
            at: vec![k("data"), k("xs")],
            mark: owned(want)
        }]
    );

    // Appends keep the first `from` and gather owned elements.
    let mut w = Writes::default();
    let at = [k("data"), k("log")];
    w.mark(
        p(&at).at(),
        Mark::Appended {
            from: 3,
            owned: Some(vec![n(3)]),
        },
    );
    w.mark(
        p(&at).at(),
        Mark::Appended {
            from: 4,
            owned: Some(vec![n(4)]),
        },
    );
    assert_eq!(w.lookup(p(&at).at()), Lookup::Dirty);
    assert_eq!(
        w.drain().dirty,
        vec![Dirty {
            at: at.to_vec(),
            mark: Mark::Appended {
                from: 3,
                owned: Some(vec![n(3), n(4)])
            }
        }]
    );

    // One unowned append makes the tail come from the view.
    let mut w = Writes::default();
    w.mark(
        p(&at).at(),
        Mark::Appended {
            from: 3,
            owned: Some(vec![n(3)]),
        },
    );
    w.mark(
        p(&at).at(),
        Mark::Appended {
            from: 4,
            owned: None,
        },
    );
    assert_eq!(
        w.drain().dirty,
        vec![Dirty {
            at: at.to_vec(),
            mark: Mark::Appended {
                from: 3,
                owned: None
            }
        }]
    );

    // A write into the tail of owned appended items makes the tail come from
    // the view, since the items no longer describe it.
    let mut w = Writes::default();
    w.mark(
        p(&at).at(),
        Mark::Appended {
            from: 3,
            owned: Some(vec![n(3)]),
        },
    );
    w.mark(p(&[k("data"), k("log"), i(3)]).at(), owned(n(9)));
    assert_eq!(
        w.drain().dirty,
        vec![Dirty {
            at: at.to_vec(),
            mark: Mark::Appended {
                from: 3,
                owned: None
            }
        }]
    );

    // A write into the appended tail is covered; one before it is not.
    let mut w = Writes::default();
    w.mark(
        p(&at).at(),
        Mark::Appended {
            from: 3,
            owned: None,
        },
    );
    w.mark(p(&[k("data"), k("log"), i(3), k("x")]).at(), owned(n(1)));
    assert!(matches!(
        &w.drain().dirty[..],
        [Dirty {
            mark: Mark::Appended { from: 3, .. },
            ..
        }]
    ));
    let mut w = Writes::default();
    w.mark(
        p(&at).at(),
        Mark::Appended {
            from: 3,
            owned: None,
        },
    );
    w.mark(p(&[k("data"), k("log"), i(1)]).at(), owned(n(1)));
    assert_eq!(
        w.drain().dirty,
        vec![Dirty {
            at: at.to_vec(),
            mark: Mark::Set(None)
        }]
    );

    // An append onto an owned array pushes into it.
    let mut w = Writes::default();
    w.mark(p(&at).at(), owned(OwnedDataValue::Array(vec![n(0)])));
    w.mark(
        p(&at).at(),
        Mark::Appended {
            from: 1,
            owned: Some(vec![n(1)]),
        },
    );
    assert_eq!(
        w.drain().dirty,
        vec![Dirty {
            at: at.to_vec(),
            mark: owned(OwnedDataValue::Array(vec![n(0), n(1)]))
        }]
    );

    // An append over marked children, or over a removal, reads the array.
    let mut w = Writes::default();
    w.mark(p(&[k("data"), k("log"), i(0)]).at(), owned(n(9)));
    w.mark(
        p(&at).at(),
        Mark::Appended {
            from: 2,
            owned: None,
        },
    );
    assert_eq!(
        w.drain().dirty,
        vec![Dirty {
            at: at.to_vec(),
            mark: Mark::Set(None)
        }]
    );
    let mut w = Writes::default();
    w.mark(p(&at).at(), Mark::Removed);
    w.mark(
        p(&at).at(),
        Mark::Appended {
            from: 0,
            owned: Some(vec![n(1)]),
        },
    );
    assert_eq!(
        w.drain().dirty,
        vec![Dirty {
            at: at.to_vec(),
            mark: Mark::Set(None)
        }]
    );

    // A removed key's object is flagged for a reorder, and reads of it go to
    // the view; a mark at or above the object drops the flag.
    let mut w = Writes::default();
    w.reorder(p(&[k("data")]).at());
    w.mark(p(&[k("data"), k("x")]).at(), Mark::Removed);
    assert_eq!(w.lookup(p(&[k("data")]).at()), Lookup::Dirty);
    assert_eq!(
        w.lookup(p(&[k("data"), k("y")]).at()),
        Lookup::Clean,
        "values are unaffected"
    );
    let d = w.drain();
    assert_eq!(d.reorder, vec![vec![k("data")]]);
    assert_eq!(
        d.dirty,
        vec![Dirty {
            at: vec![k("data"), k("x")],
            mark: Mark::Removed
        }]
    );
    let mut w = Writes::default();
    w.reorder(p(&[k("data"), k("o")]).at());
    w.mark(p(&[k("data")]).at(), Mark::Set(None));
    assert_eq!(
        w.drain(),
        Drained {
            dirty: vec![Dirty {
                at: vec![k("data")],
                mark: Mark::Set(None)
            }],
            reorder: vec![]
        }
    );
    let mut w = Writes::default();
    w.mark(p(&[k("data")]).at(), Mark::Set(None));
    w.reorder(p(&[k("data"), k("o")]).at());
    assert!(w.drain().reorder.is_empty(), "under a mark, no flag");

    // Marks drain in the order their paths were first marked, so new keys
    // reach the owned object in the order they were added.
    let mut w = Writes::default();
    w.mark(p(&[k("data"), k("b")]).at(), owned(n(1)));
    w.mark(p(&[k("data"), k("a")]).at(), owned(n(2)));
    w.mark(p(&[k("data"), k("b")]).at(), owned(n(3)));
    let order: Vec<Vec<Hop>> = w.drain().dirty.into_iter().map(|d| d.at).collect();
    assert_eq!(
        order,
        vec![vec![k("data"), k("b")], vec![k("data"), k("a")]]
    );

    // Disjoint marks drain as they are.
    let mut w = Writes::default();
    w.mark(p(&[k("data"), k("a")]).at(), owned(n(1)));
    w.mark(p(&[k("temp_data"), k("b"), k("c")]).at(), Mark::Set(None));
    w.mark(p(&[k("metadata"), k("p")]).at(), Mark::Removed);
    assert_eq!(
        sorted(w.drain().dirty),
        sorted(vec![
            Dirty {
                at: vec![k("data"), k("a")],
                mark: owned(n(1))
            },
            Dirty {
                at: vec![k("temp_data"), k("b"), k("c")],
                mark: Mark::Set(None)
            },
            Dirty {
                at: vec![k("metadata"), k("p")],
                mark: Mark::Removed
            },
        ])
    );
}

/// Two keys FNV-1a sends to one hash are still two siblings: the hash only
/// rejects, a match is decided on the names.
#[test]
fn keys_that_share_a_hash_stay_apart() {
    use crate::expr::path::key_hash;
    for (a, b) in [("costarring", "liquid"), ("declinate", "macallums")] {
        assert_eq!(key_hash(a), key_hash(b), "{a} and {b} collide");
        let mut w = Writes::default();
        w.mark(p(&[k("data"), k(a)]).at(), owned(n(1)));
        for probe in [vec![k("data"), k(b)], vec![k("data"), k(b), k("x")]] {
            assert_eq!(w.lookup(p(&probe).at()), Lookup::Clean, "{probe:?}");
        }
        w.mark(p(&[k("data"), k(b)]).at(), owned(n(2)));
        let read = |w: &Writes, key: &str| lookup_text(w.lookup(p(&[k("data"), k(key)]).at()));
        assert_eq!(read(&w, a), r#"Owned(Some("1"))"#);
        assert_eq!(read(&w, b), r#"Owned(Some("2"))"#);
        // A key under `b` cannot go into a number: `b` alone reads the view.
        w.mark(p(&[k("data"), k(b), k("x")]).at(), owned(n(3)));
        assert_eq!(read(&w, a), r#"Owned(Some("1"))"#);
        assert_eq!(read(&w, b), "Dirty");
        assert_eq!(
            texts(&w.drain().dirty),
            [
                format!(r#"[Key("data"), Key("{a}")] Set(Some("1"))"#),
                format!(r#"[Key("data"), Key("{b}")] Set(None)"#),
            ],
            "two marks, in the order first marked"
        );
    }
}

/// A random path; the first hop is a key, since a context is an object.
fn hops(rng: &mut random::Rng) -> Vec<Hop> {
    let len = 1 + rng.below(4);
    (0..len)
        .map(|n| match rng.below(3) {
            0 if n > 0 => i(rng.below(4)),
            _ => k(rng.pick(&["data", "a", "b", "c"])),
        })
        .collect()
}

fn mark(rng: &mut random::Rng) -> Mark {
    match rng.below(6) {
        0 | 1 => Mark::Set(None),
        2 => owned(random::value(rng)),
        3 => Mark::Removed,
        4 => Mark::Appended {
            from: rng.below(4),
            owned: None,
        },
        _ => Mark::Appended {
            from: rng.below(4),
            owned: Some(vec![random::value(rng)]),
        },
    }
}

/// The spike's agreement test, as written: the trie and a flat list of marks
/// answer every lookup alike and drain the same marks and reorders.
#[test]
fn the_trie_and_the_list_agree_on_random_marks() {
    let seed = random::seed(0x7e1e);
    let mut rng = random::Rng::new(seed);
    for case in 0..20_000 {
        let (mut trie, mut list) = (Writes::default(), List::default());
        for step in 0..1 + rng.below(12) {
            let at = hops(&mut rng);
            if rng.chance(1, 6) {
                trie.reorder(p(&at).at());
                list.reorder(p(&at).at());
            } else {
                let m = mark(&mut rng);
                trie.mark(p(&at).at(), m.clone());
                list.mark(p(&at).at(), m);
            }
            for _ in 0..3 {
                let probe = hops(&mut rng);
                assert_eq!(
                    trie.lookup(p(&probe).at()),
                    list.lookup(p(&probe).at()),
                    "seed {seed:#x}, case {case}, step {step}, {probe:?}"
                );
            }
        }
        assert_eq!(
            trie.is_empty(),
            list.is_empty(),
            "seed {seed:#x}, case {case}: emptiness"
        );
        let (mut tv, mut lv) = (Vec::new(), Vec::new());
        trie.visit(&mut |at, m| tv.push(format!("{at:?} {}", text(m))));
        list.visit(&mut |at, m| lv.push(format!("{at:?} {}", text(m))));
        tv.sort();
        lv.sort();
        assert_eq!(
            tv, lv,
            "seed {seed:#x}, case {case}: the same visited marks"
        );
        // The same marks and reorders. Their order is checked where it
        // matters, by the run-context model test: siblings may differ here
        // for a node first touched through a deeper path, which the run
        // context never produces for a new key.
        let (t, l) = (trie.drain(), list.drain());
        assert_eq!(
            texts(&sorted(t.dirty)),
            texts(&sorted(l.dirty)),
            "seed {seed:#x}, case {case}: the same marks"
        );
        let (mut tr, mut lr) = (t.reorder, l.reorder);
        tr.sort();
        lr.sort();
        assert_eq!(tr, lr, "seed {seed:#x}, case {case}: the same reorders");
    }
}

/// Reads and marks in any order never panic; `visit` shows exactly the marks
/// `drain` returns, in the same order; every visited mark reads as changed;
/// the drained marks are disjoint (none lies under another); and the drain
/// empties the log.
#[test]
fn random_marks_drain_disjoint_and_leave_the_log_empty() {
    let seed = random::seed(0x7e1f);
    let mut rng = random::Rng::new(seed);
    for case in 0..20_000 {
        let (mut w, mut list) = (Writes::default(), List::default());
        for _ in 0..1 + rng.below(12) {
            let at = hops(&mut rng);
            if rng.chance(1, 6) {
                w.reorder(p(&at).at());
                list.reorder(p(&at).at());
            } else {
                let m = mark(&mut rng);
                w.mark(p(&at).at(), m.clone());
                list.mark(p(&at).at(), m);
            }
            for _ in 0..3 {
                let probe = hops(&mut rng);
                assert_eq!(
                    lookup_text(w.lookup(p(&probe).at())),
                    lookup_text(list.lookup(p(&probe).at())),
                    "case {case}: the list answers {probe:?} alike"
                );
            }
        }
        let mut visited = Vec::new();
        w.visit(&mut |at, mark| {
            visited.push(Dirty {
                at: at.to_vec(),
                mark: mark.clone(),
            });
        });
        for v in &visited {
            assert_ne!(
                w.lookup(p(&v.at).at()),
                Lookup::Clean,
                "case {case}: {:?} is marked",
                v.at
            );
        }
        let d = w.drain();
        assert_eq!(
            texts(&visited),
            texts(&d.dirty),
            "case {case}: visit shows what drain returns"
        );
        for (a, x) in d.dirty.iter().enumerate() {
            for (b, y) in d.dirty.iter().enumerate() {
                assert!(
                    a == b || !y.at.starts_with(&x.at),
                    "case {case}: {:?} lies under {:?}",
                    y.at,
                    x.at
                );
            }
        }
        assert!(w.is_empty(), "case {case}: the drain empties the log");
    }
}

#[test]
fn a_log_that_grew_past_its_caps_is_not_pooled() {
    // A huge computed key, then the drop: its name storage leaves with it.
    let huge = "k".repeat(100 << 10);
    let mut w = Writes::default();
    w.mark(p(&[k("data"), k(&huge)]).at(), owned(n(1)));
    drop(w);
    assert!(
        pooled().iter().all(|&(_, names)| names < huge.len()),
        "{:?}",
        pooled()
    );
    // A small one is pooled, unless the pool is full already.
    let mut w = Writes::default();
    w.mark(p(&[k("data"), k("small")]).at(), owned(n(1)));
    drop(w);
    assert!(!pooled().is_empty());
}
