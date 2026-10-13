//! The write log (R55): what changed since the view was built, keyed by
//! resolved [`Hop`]s, so the checkpoint can bring the owned tree up to date by
//! converting only the paths that changed.
//!
//! A marked node subsumes everything below it, so the marks the checkpoint
//! applies are disjoint and their values need no ordering rule. Key order
//! does: an object key removed and written again moves to the end, which
//! assigning it in place would not reproduce. So a key's removal also flags
//! its object for a reorder, applied after the marks, which reorders the
//! owned object's keys to the view's at O(keys), converting nothing. New keys
//! need no flag: they are pushed in the order they were first marked, which is
//! the order they were added. [`Writes`] is a trie: a write costs its path's
//! depth.
//!
//! A write reaches the log as an [`At`]: the path's own segments and which of
//! them resolved to an array index. Nothing is allocated to ask the log about
//! a path; a [`Hop`] is built only when the log keeps a new node.

use super::splice::Hop;
use crate::expr::Seg;
use crate::expr::path;
use datavalue::{DataValue, OwnedDataValue};

/// A path as the log sees it: the segments as written, and which of them
/// resolved to an array index (bit `i` of `kinds`). Copy, borrowed, free.
#[derive(Clone, Copy, Debug)]
pub(crate) struct At<'a> {
    segs: &'a [Seg],
    kinds: u64,
}

fn mask(n: usize) -> u64 {
    if n >= 64 { u64::MAX } else { (1 << n) - 1 }
}

impl<'a> At<'a> {
    pub(crate) fn new(segs: &'a [Seg], kinds: u64) -> Self {
        debug_assert!(segs.len() <= 64, "a path is at most 64 segments deep");
        Self {
            segs,
            kinds: kinds & mask(segs.len()),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.segs.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }

    pub(crate) fn segs(&self) -> &'a [Seg] {
        self.segs
    }

    pub(crate) fn is_index(&self, i: usize) -> bool {
        self.kinds >> i & 1 == 1
    }

    /// The array index of segment `i`, when it resolved to one.
    pub(crate) fn index(&self, i: usize) -> Option<usize> {
        if self.is_index(i) {
            self.segs[i].index
        } else {
            None
        }
    }

    pub(crate) fn prefix(&self, n: usize) -> Self {
        Self {
            segs: &self.segs[..n],
            kinds: self.kinds & mask(n),
        }
    }

    pub(crate) fn hop(&self, i: usize) -> Hop {
        match self.index(i) {
            Some(j) => Hop::Index(j),
            None => Hop::Key(self.segs[i].key.clone()),
        }
    }

    pub(crate) fn to_hops(self) -> Vec<Hop> {
        (0..self.len()).map(|i| self.hop(i)).collect()
    }
}

/// Hops with their segments, for a caller that holds them: tests.
pub(crate) struct OwnedAt {
    segs: Vec<Seg>,
    kinds: u64,
}

impl OwnedAt {
    pub(crate) fn at(&self) -> At<'_> {
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

/// What a write leaves at its path until the next checkpoint.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Mark {
    /// The path holds a new value: the owned value when the writer had one,
    /// else `None`, and the checkpoint converts it from the view.
    Set(Option<OwnedDataValue>),
    /// The path was removed.
    Removed,
    /// The array at the path gained elements from index `from` on, `from`
    /// being its length before the first append since the view was built.
    /// `owned` holds the elements when every append carried them.
    Appended {
        from: usize,
        owned: Option<Vec<OwnedDataValue>>,
    },
}

/// What a read finds at a path.
#[derive(Debug, PartialEq)]
pub(crate) enum Lookup<'l> {
    /// Nothing at, above or below the path changed: the owned tree is current.
    Clean,
    /// The path changed and only the view knows how: read the view.
    Dirty,
    /// The log itself holds the answer: a value under an owned mark, or
    /// `None` under a removal.
    Owned(Option<&'l OwnedDataValue>),
}

/// One topmost marked path and what the checkpoint must do there.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Dirty {
    pub(crate) at: Vec<Hop>,
    pub(crate) mark: Mark,
}

/// Everything the checkpoint applies: the marks, in the order their paths
/// were first marked, then the objects whose key order must follow the view.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Drained {
    pub(crate) dirty: Vec<Dirty>,
    pub(crate) reorder: Vec<Vec<Hop>>,
}

/// A write at segment `i` on of `at`, below an ancestor already marked
/// `above`. A marked node never has marked descendants, so the ancestor
/// accounts for the write, changing its own mark when it must. An owned
/// ancestor mirrors the view, so the write's own segments apply to it as they
/// did to the view.
fn absorb(above: &mut Mark, at: At<'_>, i: usize, new: Mark) {
    match above {
        // An owned ancestor takes an owned write into its value. Without an
        // owned value to apply, it reads the whole subtree from the view.
        Mark::Set(Some(value)) => {
            let rest = &at.segs()[i..];
            let applied = match new {
                Mark::Set(Some(v)) => path::set(value, rest, v),
                Mark::Removed => {
                    path::remove(value, rest);
                    true
                }
                Mark::Appended {
                    owned: Some(items), ..
                } => path::append(value, rest, items).is_ok(),
                Mark::Set(None) | Mark::Appended { owned: None, .. } => false,
            };
            if !applied {
                *above = Mark::Set(None);
            }
        }
        // The whole subtree is read from the view anyway.
        Mark::Set(None) => {}
        // Something was written below a removed path, so the path exists
        // again, as the view shows it.
        Mark::Removed => *above = Mark::Set(None),
        // A write into the appended tail changes elements the owned items no
        // longer describe, so the tail is converted from the view; one before
        // `from` changes an element the owned tree has.
        Mark::Appended { from, owned } => {
            if matches!(at.index(i), Some(j) if j >= *from) {
                *owned = None;
            } else {
                *above = Mark::Set(None);
            }
        }
    }
}

/// The mark at a node that had `old` and receives `new`, given whether the
/// node has marked descendants (which a set or removal drops).
fn combine(old: Option<Mark>, new: Mark, has_children: bool) -> Mark {
    match (old, new) {
        (_, new @ (Mark::Set(_) | Mark::Removed)) => new,
        (Some(Mark::Appended { from, owned: a }), Mark::Appended { owned: b, .. }) => {
            let owned = match (a, b) {
                (Some(mut a), Some(b)) => {
                    a.extend(b);
                    Some(a)
                }
                _ => None,
            };
            Mark::Appended { from, owned }
        }
        (
            Some(Mark::Set(Some(mut v))),
            Mark::Appended {
                owned: Some(items), ..
            },
        ) => match path::append(&mut v, &[], items) {
            Ok(_) => Mark::Set(Some(v)),
            Err(_) => Mark::Set(None),
        },
        // An append over a removed array, an unowned value, or children
        // already marked: read the whole array from the view.
        (Some(_), Mark::Appended { .. }) => Mark::Set(None),
        (None, new @ Mark::Appended { .. }) if !has_children => new,
        (None, Mark::Appended { .. }) => Mark::Set(None),
    }
}

/// What a read of `rest` under a mark finds.
fn read_under<'l>(mark: &'l Mark, rest: &[Seg]) -> Lookup<'l> {
    match mark {
        Mark::Set(Some(v)) => Lookup::Owned(path::get(v, rest)),
        Mark::Removed => Lookup::Owned(None),
        Mark::Set(None) | Mark::Appended { .. } => Lookup::Dirty,
    }
}

/// Whether `new`, at the last segment of `at`, removes an array element:
/// that shifts the elements after it, so no path names what changed, and
/// the array itself is rewritten.
fn removes_an_element(at: At<'_>, new: &Mark) -> bool {
    matches!(new, Mark::Removed) && at.is_index(at.len() - 1)
}

/// A borrowed hop: a key as `&str`, or an index.
#[derive(Clone, Copy, Debug)]
enum KeyRef<'k> {
    Name(&'k str),
    Index(usize),
}

/// The child of a view node at `key`.
fn view_child<'v>(v: DataValue<'v>, key: KeyRef<'_>) -> Option<DataValue<'v>> {
    match (v, key) {
        (DataValue::Object(pairs), KeyRef::Name(k)) => {
            pairs.iter().find(|(pk, _)| **pk == *k).map(|(_, c)| *c)
        }
        (DataValue::Array(items), KeyRef::Index(i)) => items.get(i).copied(),
        _ => None,
    }
}

/// Turn `mark`, made where the view holds `here`, into one that needs no
/// view: what the checkpoint's first phase does to every mark.
fn settle_mark(mark: &mut Mark, here: Option<DataValue<'_>>) -> u32 {
    match mark {
        Mark::Set(slot @ None) => {
            *slot = Some(here.expect("a marked path is in the view").to_owned());
            1
        }
        Mark::Appended {
            from,
            owned: slot @ None,
        } => match here {
            Some(DataValue::Array(items)) => {
                *slot = Some(items[*from..].iter().map(DataValue::to_owned).collect());
                1
            }
            other => unreachable!("an appended path holds an array, not {other:?}"),
        },
        _ => 0,
    }
}

/// Apply a settled `mark` to the child of `parent` at `key`, with
/// `path::set`'s single-segment rules: a key replaced or appended, an index
/// padded and assigned.
fn apply_mark(parent: &mut OwnedDataValue, key: KeyRef<'_>, mark: Mark) {
    match mark {
        Mark::Set(Some(v)) => assign(parent, key, v),
        Mark::Removed => match (parent, key) {
            (OwnedDataValue::Object(pairs), KeyRef::Name(k)) => {
                if let Some(at) = pairs.iter().position(|(pk, _)| pk == k) {
                    pairs.remove(at);
                }
            }
            (OwnedDataValue::Array(items), KeyRef::Index(i)) if i < items.len() => {
                items.remove(i);
            }
            _ => {}
        },
        Mark::Appended {
            owned: Some(items), ..
        } => match owned_child(parent, key) {
            Some(OwnedDataValue::Array(existing)) => existing.extend(items),
            _ => assign(parent, key, OwnedDataValue::Array(items)),
        },
        Mark::Set(None) | Mark::Appended { owned: None, .. } => {
            unreachable!("applied before settled")
        }
    }
}

/// `path::set` for one segment.
fn assign(parent: &mut OwnedDataValue, key: KeyRef<'_>, v: OwnedDataValue) {
    match (parent, key) {
        (OwnedDataValue::Object(pairs), KeyRef::Name(k)) => {
            match pairs.iter_mut().find(|(pk, _)| pk == k) {
                Some(slot) => slot.1 = v,
                None => pairs.push((k.to_string(), v)),
            }
        }
        (OwnedDataValue::Array(items), KeyRef::Index(i)) => {
            if items.len() <= i {
                items.resize(i + 1, OwnedDataValue::Null);
            }
            items[i] = v;
        }
        _ => {}
    }
}

/// The owned child of `v` at `key`. An unmarked node of the log exists in
/// the owned tree: a write is marked where it starts creating.
fn owned_child<'v>(v: &'v mut OwnedDataValue, key: KeyRef<'_>) -> Option<&'v mut OwnedDataValue> {
    match (v, key) {
        (OwnedDataValue::Object(pairs), KeyRef::Name(k)) => {
            pairs.iter_mut().find(|(pk, _)| pk == k).map(|(_, c)| c)
        }
        (OwnedDataValue::Array(items), KeyRef::Index(i)) => items.get_mut(i),
        _ => None,
    }
}

/// Reorder an owned object's keys to `order`, the view's.
fn reorder_keys(v: &mut OwnedDataValue, order: &[Box<str>]) {
    if let OwnedDataValue::Object(pairs) = v {
        let rank = |k: &str| order.iter().position(|o| **o == *k).unwrap_or(usize::MAX);
        pairs.sort_by_cached_key(|(k, _)| rank(k));
    }
}

/// A view object's key order.
fn key_order(v: Option<DataValue<'_>>) -> Vec<Box<str>> {
    match v {
        Some(DataValue::Object(pairs)) => pairs.iter().map(|(k, _)| (*k).into()).collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// The trie
// ---------------------------------------------------------------------------

const NONE: u32 = u32::MAX;

/// A node's key: a name in the trie's buffer, an index, or the root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Root,
    Name { start: u32, len: u32 },
    Index(usize),
}

#[derive(Debug)]
struct Node {
    key: Key,
    mark: Option<Mark>,
    reorder: bool,
    /// The view's key order, taken by `settle` for a reorder.
    order: Vec<Box<str>>,
    /// Children as a list in insertion order, which is the order new keys
    /// reach the owned tree: first, last, and each child's next sibling.
    first: u32,
    last: u32,
    next: u32,
}

impl Node {
    fn new(key: Key) -> Self {
        Self {
            key,
            mark: None,
            reorder: false,
            order: Vec::new(),
            first: NONE,
            last: NONE,
            next: NONE,
        }
    }
}

/// The write log as a trie keyed by hop: a write or a read costs the depth of
/// its path, whatever the log holds.
///
/// Flat: every node in one vector, every key in one string. A subsumed
/// subtree is unlinked, not freed, and the storage is reset at the
/// checkpoint and pooled per thread when the log drops, so a warm run, and a
/// call's overlay, allocate nothing for the log.
#[derive(Debug)]
pub(crate) struct Writes {
    nodes: Vec<Node>,
    names: String,
}

/// Logs kept per thread, and the size past which one is not kept.
const TRIE_POOL: usize = 16;
const TRIE_NODES_CAP: usize = 4096;

thread_local! {
    static WRITES_POOL: std::cell::RefCell<Vec<(Vec<Node>, String)>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Default for Writes {
    fn default() -> Self {
        let (nodes, names) = WRITES_POOL
            .try_with(|p| p.borrow_mut().pop())
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut trie = Self { nodes, names };
        trie.reset();
        trie
    }
}

impl Drop for Writes {
    fn drop(&mut self) {
        if self.nodes.capacity() > TRIE_NODES_CAP {
            return;
        }
        let mut nodes = std::mem::take(&mut self.nodes);
        let mut names = std::mem::take(&mut self.names);
        nodes.clear();
        names.clear();
        let _ = WRITES_POOL.try_with(|p| {
            let mut p = p.borrow_mut();
            if p.len() < TRIE_POOL {
                p.push((nodes, names));
            }
        });
    }
}

impl Writes {
    fn reset(&mut self) {
        self.nodes.clear();
        self.nodes.push(Node::new(Key::Root));
        self.names.clear();
    }

    fn name(&self, start: u32, len: u32) -> &str {
        &self.names[start as usize..(start + len) as usize]
    }

    fn key_ref(names: &str, key: Key) -> KeyRef<'_> {
        match key {
            Key::Name { start, len } => {
                KeyRef::Name(&names[start as usize..(start + len) as usize])
            }
            Key::Index(i) => KeyRef::Index(i),
            Key::Root => unreachable!("the root has no key"),
        }
    }

    fn matches(&self, key: Key, at: At<'_>, i: usize) -> bool {
        match key {
            Key::Index(j) => at.index(i) == Some(j),
            Key::Name { start, len } => {
                !at.is_index(i) && self.name(start, len) == &*at.segs()[i].key
            }
            Key::Root => false,
        }
    }

    fn find(&self, parent: u32, at: At<'_>, i: usize) -> Option<u32> {
        let mut c = self.nodes[parent as usize].first;
        while c != NONE {
            if self.matches(self.nodes[c as usize].key, at, i) {
                return Some(c);
            }
            c = self.nodes[c as usize].next;
        }
        None
    }

    fn child(&mut self, parent: u32, at: At<'_>, i: usize) -> u32 {
        if let Some(c) = self.find(parent, at, i) {
            return c;
        }
        let key = match at.index(i) {
            Some(j) => Key::Index(j),
            None => {
                let start = self.names.len() as u32;
                self.names.push_str(&at.segs()[i].key);
                Key::Name {
                    start,
                    len: self.names.len() as u32 - start,
                }
            }
        };
        let id = self.nodes.len() as u32;
        self.nodes.push(Node::new(key));
        let last = self.nodes[parent as usize].last;
        if last == NONE {
            self.nodes[parent as usize].first = id;
        } else {
            self.nodes[last as usize].next = id;
        }
        self.nodes[parent as usize].last = id;
        id
    }

    fn unlink_children(&mut self, n: u32) {
        let node = &mut self.nodes[n as usize];
        node.first = NONE;
        node.last = NONE;
    }

    fn hop(&self, key: Key) -> Hop {
        match key {
            Key::Name { start, len } => Hop::Key(self.name(start, len).into()),
            Key::Index(i) => Hop::Index(i),
            Key::Root => unreachable!("the root has no key"),
        }
    }
}

impl Writes {
    /// Records a write at `at`, which is never empty.
    pub(crate) fn mark(&mut self, at: At<'_>, new: Mark) {
        debug_assert!(!at.is_empty(), "a write has a path");
        let element = removes_an_element(at, &new);
        let mut n = 0u32;
        for i in 0..at.len() {
            if let Some(above) = self.nodes[n as usize].mark.as_mut() {
                return absorb(above, at, i, new);
            }
            // An element's removal rewrites its array, unless an owned
            // ancestor, met above, applied it instead.
            if element && i + 1 == at.len() {
                let has_children = self.nodes[n as usize].first != NONE;
                self.unlink_children(n);
                let node = &mut self.nodes[n as usize];
                node.reorder = false;
                node.mark = Some(combine(node.mark.take(), Mark::Set(None), has_children));
                return;
            }
            n = self.child(n, at, i);
        }
        let has_children = self.nodes[n as usize].first != NONE;
        let node = &mut self.nodes[n as usize];
        let mark = combine(node.mark.take(), new, has_children);
        let keep = matches!(mark, Mark::Appended { .. }) && !has_children;
        node.reorder = false;
        node.mark = Some(mark);
        if !keep {
            self.unlink_children(n);
        }
    }

    /// Flag the object at `at` for a reorder: a key of it was removed.
    pub(crate) fn reorder(&mut self, at: At<'_>) {
        let mut n = 0u32;
        for i in 0..at.len() {
            if self.nodes[n as usize].mark.is_some() {
                return;
            }
            n = self.child(n, at, i);
        }
        let node = &mut self.nodes[n as usize];
        if node.mark.is_none() {
            node.reorder = true;
        }
    }

    pub(crate) fn lookup(&self, at: At<'_>) -> Lookup<'_> {
        let mut n = 0u32;
        for i in 0..at.len() {
            if let Some(mark) = &self.nodes[n as usize].mark {
                return read_under(mark, &at.segs()[i..]);
            }
            match self.find(n, at, i) {
                Some(c) => n = c,
                None => return Lookup::Clean,
            }
        }
        let node = &self.nodes[n as usize];
        match &node.mark {
            Some(mark) => read_under(mark, &[]),
            None if node.first == NONE && !node.reorder => Lookup::Clean,
            None => Lookup::Dirty,
        }
    }

    /// The topmost marks and the reorders, leaving the log empty.
    pub(crate) fn drain(&mut self) -> Drained {
        // Marks are moved out, never cloned: an overlay's are the call's
        // written values.
        fn walk(trie: &mut Writes, n: u32, at: &mut Vec<Hop>, out: &mut Drained) {
            if let Some(mark) = trie.nodes[n as usize].mark.take() {
                return out.dirty.push(Dirty {
                    at: at.clone(),
                    mark,
                });
            }
            if trie.nodes[n as usize].reorder {
                out.reorder.push(at.clone());
            }
            let mut c = trie.nodes[n as usize].first;
            while c != NONE {
                at.push(trie.hop(trie.nodes[c as usize].key));
                walk(trie, c, at, out);
                at.pop();
                c = trie.nodes[c as usize].next;
            }
        }
        let mut out = Drained::default();
        walk(self, 0, &mut Vec::new(), &mut out);
        self.reset();
        out
    }

    pub(crate) fn is_empty(&self) -> bool {
        let root = &self.nodes[0];
        root.first == NONE && !root.reorder
    }

    /// The checkpoint's first phase, with the view `root` alive: every mark
    /// that needs the view takes what it needs from it, a value-less `Set`
    /// its value, an unowned `Appended` its tail, a reorder flag its key
    /// order, so the second phase needs no view.
    /// Returns how many marks it converted from the view.
    pub(crate) fn settle(&mut self, root: DataValue<'_>) -> u32 {
        fn walk(nodes: &mut [Node], names: &str, n: u32, here: Option<DataValue<'_>>) -> u32 {
            if let Some(mark) = &mut nodes[n as usize].mark {
                return settle_mark(mark, here);
            }
            if nodes[n as usize].reorder {
                nodes[n as usize].order = key_order(here);
            }
            let mut converted = 0;
            let mut c = nodes[n as usize].first;
            while c != NONE {
                let below =
                    here.and_then(|v| view_child(v, Writes::key_ref(names, nodes[c as usize].key)));
                converted += walk(nodes, names, c, below);
                c = nodes[c as usize].next;
            }
            converted
        }
        let Self { nodes, names } = self;
        walk(nodes, names, 0, Some(root))
    }

    /// The checkpoint's second phase: apply every mark to `owned`, then the
    /// reorders, leaving the log empty. Only after [`Self::settle`].
    pub(crate) fn apply(&mut self, owned: &mut OwnedDataValue) {
        fn walk(nodes: &mut [Node], names: &str, n: u32, here: &mut OwnedDataValue) {
            let mut c = nodes[n as usize].first;
            while c != NONE {
                let key = Writes::key_ref(names, nodes[c as usize].key);
                match nodes[c as usize].mark.take() {
                    Some(mark) => apply_mark(here, key, mark),
                    None => {
                        if let Some(below) = owned_child(here, key) {
                            walk(nodes, names, c, below);
                        }
                    }
                }
                c = nodes[c as usize].next;
            }
            if nodes[n as usize].reorder {
                reorder_keys(here, &nodes[n as usize].order);
            }
        }
        let Self { nodes, names } = self;
        walk(nodes, names, 0, owned);
        self.reset();
    }
}
