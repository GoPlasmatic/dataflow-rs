//! The write log: what changed since the view was built, in a trie
//! keyed by resolved segments, each a key's name or an array index, so the
//! checkpoint can bring the owned tree up to date by converting only the
//! paths that changed.
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
//! a path; a node is added, and a key's name copied into the log's one string
//! of names, only when the log keeps a new one. The tests read the marks back
//! as resolved hops.

use crate::expr::Seg;
use crate::expr::path;
use crate::expr::path::MAX_PATH_SEGMENTS;
use crate::expr::rules;
use datavalue::{DataValue, OwnedDataValue};
use std::collections::HashMap;

/// A path as the log sees it: the segments as written, and which of them
/// resolved to an array index (bit `i` of `kinds`). Copy, borrowed, free.
#[derive(Clone, Copy, Debug)]
pub(crate) struct At<'a> {
    /// The segments as written.
    segs: &'a [Seg],
    /// Bit `i` set when segment `i` resolved to an array index.
    kinds: u64,
}

/// The low `n` bits.
fn mask(n: usize) -> u64 {
    if n >= MAX_PATH_SEGMENTS {
        u64::MAX
    } else {
        (1 << n) - 1
    }
}

impl<'a> At<'a> {
    /// A path of `segs` whose segments in `kinds` resolved to array indices.
    pub(crate) fn new(segs: &'a [Seg], kinds: u64) -> Self {
        debug_assert!(
            segs.len() <= MAX_PATH_SEGMENTS,
            "a path is at most {MAX_PATH_SEGMENTS} segments deep"
        );
        Self {
            segs,
            kinds: kinds & mask(segs.len()),
        }
    }

    /// How many segments the path has.
    pub(crate) fn len(&self) -> usize {
        self.segs.len()
    }

    /// Whether the path has no segments.
    pub(crate) fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }

    /// The segments as written.
    pub(crate) fn segs(&self) -> &'a [Seg] {
        self.segs
    }

    /// Whether segment `i` resolved to an array index.
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

    /// The first `n` segments.
    pub(crate) fn prefix(&self, n: usize) -> Self {
        Self {
            segs: &self.segs[..n],
            kinds: self.kinds & mask(n),
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
        /// The array's length before the first append.
        from: usize,
        /// The appended elements, when every append carried them.
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

/// A write at segment `i` on of `at`, below an ancestor already marked
/// `above`. A marked node never has marked descendants, so the ancestor
/// accounts for the write, changing its own mark when it must. An owned
/// ancestor mirrors the view, so the write's own segments apply to it as they
/// did to the view.
pub(super) fn absorb(above: &mut Mark, at: At<'_>, i: usize, new: Mark) {
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
pub(super) fn combine(old: Option<Mark>, new: Mark, has_children: bool) -> Mark {
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
pub(super) fn read_under<'l>(mark: &'l Mark, rest: &[Seg]) -> Lookup<'l> {
    match mark {
        Mark::Set(Some(v)) => Lookup::Owned(path::get(v, rest)),
        Mark::Removed => Lookup::Owned(None),
        Mark::Set(None) | Mark::Appended { .. } => Lookup::Dirty,
    }
}

/// Whether `new`, at the last segment of `at`, removes an array element:
/// that shifts the elements after it, so no path names what changed, and
/// the array itself is rewritten.
pub(super) fn removes_an_element(at: At<'_>, new: &Mark) -> bool {
    matches!(new, Mark::Removed) && at.is_index(at.len() - 1)
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

/// Apply a settled `mark` to the child of `parent` at `key`, with the path
/// rules' one-segment primitives: a key replaced or appended, an index
/// padded and assigned, a child taken out.
fn apply_mark(parent: &mut OwnedDataValue, key: rules::Hop<'_>, mark: Mark) {
    match mark {
        Mark::Set(Some(v)) => {
            rules::put(parent, key, v);
        }
        Mark::Removed => {
            rules::take(parent, key);
        }
        Mark::Appended {
            owned: Some(items), ..
        } => match rules::owned_child_mut(parent, key) {
            Some(OwnedDataValue::Array(existing)) => existing.extend(items),
            _ => {
                rules::put(parent, key, OwnedDataValue::Array(items));
            }
        },
        Mark::Set(None) | Mark::Appended { owned: None, .. } => {
            unreachable!("applied before settled")
        }
    }
}

/// Reorder an owned object's keys to `order`, the view's, at O(keys): each
/// key's rank is its first place in `order`, from a map built once, and a
/// counting sort places every pair at its rank. A key `order` lacks goes
/// last, and pairs that share a rank keep their order.
fn reorder_keys(v: &mut OwnedDataValue, order: &[Box<str>]) {
    let OwnedDataValue::Object(pairs) = v else {
        return;
    };
    let mut first = HashMap::with_capacity(order.len());
    for (i, key) in order.iter().enumerate() {
        first.entry(&**key).or_insert(i);
    }
    let ranks: Vec<usize> = pairs
        .iter()
        .map(|(k, _)| first.get(k.as_str()).copied().unwrap_or(order.len()))
        .collect();
    // `next[r]`: where the next pair of rank `r` goes.
    let mut next = vec![0; order.len() + 2];
    for &r in &ranks {
        next[r + 1] += 1;
    }
    for r in 1..next.len() {
        next[r] += next[r - 1];
    }
    let mut placed: Vec<Option<(String, OwnedDataValue)>> = Vec::new();
    placed.resize_with(pairs.len(), || None);
    for (pair, r) in pairs.drain(..).zip(ranks) {
        placed[next[r]] = Some(pair);
        next[r] += 1;
    }
    pairs.extend(placed.into_iter().flatten());
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

/// No node: the end of a child list.
pub(super) const NONE: u32 = u32::MAX;

/// A node's key: a name in the trie's buffer, an index, or the root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Key {
    /// The trie's root, which has no key.
    Root,
    /// An object key: `len` bytes of the trie's name buffer from `start`.
    Name {
        /// Where the name starts.
        start: u32,
        /// How long it is.
        len: u32,
        /// The segment's [`Seg::hash`], compared before the name.
        hash: u32,
    },
    /// An array index.
    Index(usize),
}

/// A node of the trie: a key, its mark and reorder flag, and its children.
#[derive(Debug)]
pub(super) struct Node {
    /// This node's key under its parent.
    pub(super) key: Key,
    /// What a write left here, if anything.
    pub(super) mark: Option<Mark>,
    /// Whether a key of this object was removed.
    pub(super) reorder: bool,
    /// The view's key order, taken by `settle` for a reorder.
    order: Vec<Box<str>>,
    /// Children as a list in insertion order, which is the order new keys
    /// reach the owned tree: first, last, and each child's next sibling.
    pub(super) first: u32,
    /// The last child.
    pub(super) last: u32,
    /// The next sibling.
    pub(super) next: u32,
}

impl Node {
    /// A node with no mark and no children.
    pub(super) fn new(key: Key) -> Self {
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
/// checkpoint and pooled per thread when the log drops, so a warm run
/// allocates nothing for the log.
#[derive(Debug)]
pub(crate) struct Writes {
    /// Every node, the root first.
    pub(super) nodes: Vec<Node>,
    /// Every name, one after another.
    names: String,
}

/// Logs kept per thread.
const WRITES_POOLED: usize = 16;
/// A log whose node storage grew past this many nodes is not pooled.
const WRITES_NODES_CAP: usize = 4096;
/// A log whose name storage grew past this many bytes is not pooled, so one
/// huge computed key does not stay with the thread.
const WRITES_NAMES_CAP: usize = 64 << 10;

thread_local! {
    /// Storage of logs that were dropped, reused by the next.
    pub(super) static WRITES_POOL: std::cell::RefCell<Vec<(Vec<Node>, String)>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Default for Writes {
    fn default() -> Self {
        let (nodes, names) = WRITES_POOL
            .try_with(|p| p.borrow_mut().pop())
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut log = Self { nodes, names };
        log.reset();
        log
    }
}

impl Drop for Writes {
    fn drop(&mut self) {
        if self.nodes.capacity() > WRITES_NODES_CAP || self.names.capacity() > WRITES_NAMES_CAP {
            return;
        }
        let mut nodes = std::mem::take(&mut self.nodes);
        let mut names = std::mem::take(&mut self.names);
        nodes.clear();
        names.clear();
        let _ = WRITES_POOL.try_with(|p| {
            let mut p = p.borrow_mut();
            if p.len() < WRITES_POOLED {
                p.push((nodes, names));
            }
        });
    }
}

impl Writes {
    /// Empties the log, keeping its storage.
    pub(super) fn reset(&mut self) {
        self.nodes.clear();
        self.nodes.push(Node::new(Key::Root));
        self.names.clear();
    }

    /// The key text stored at `start..start + len`.
    pub(super) fn name(&self, start: u32, len: u32) -> &str {
        &self.names[start as usize..(start + len) as usize]
    }

    /// A node's key as a borrowed hop.
    fn key_ref(names: &str, key: Key) -> rules::Hop<'_> {
        match key {
            Key::Name { start, len, .. } => {
                rules::Hop::Key(&names[start as usize..(start + len) as usize])
            }
            Key::Index(i) => rules::Hop::Index(i),
            Key::Root => unreachable!("the root has no key"),
        }
    }

    /// Whether the node key `key` is segment `i` of `at`. A name is
    /// compared only when the hashes agree, and always then: keys can share
    /// a hash.
    fn matches(&self, key: Key, at: At<'_>, i: usize) -> bool {
        match key {
            Key::Index(j) => at.index(i) == Some(j),
            Key::Name { start, len, hash } => {
                let seg = &at.segs()[i];
                !at.is_index(i) && hash == seg.hash && self.name(start, len) == &*seg.key
            }
            Key::Root => false,
        }
    }

    /// The child of `parent` that segment `i` of `at` names.
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

    /// Appends a new child node with `key` at the end of `parent`'s child list.
    fn push_child(&mut self, parent: u32, key: Key) -> u32 {
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

    /// That child, created when missing, last in insertion order.
    fn child(&mut self, parent: u32, at: At<'_>, i: usize) -> u32 {
        match at.index(i) {
            Some(j) => {
                if let Some(c) = self.find(parent, at, i) {
                    return c;
                }
                self.push_child(parent, Key::Index(j))
            }
            None => {
                let seg = &at.segs()[i];
                self.child_name(parent, &seg.key, seg.hash)
            }
        }
    }

    /// Whether a child node key matches the object key `name` with `hash`.
    fn matches_name(&self, key: Key, name: &str, hash: u32) -> bool {
        matches!(key, Key::Name { start, len, hash: h } if h == hash && self.name(start, len) == name)
    }

    /// The child of `parent` for the object key `name` (with `hash`), created
    /// when missing, last in insertion order.
    fn child_name(&mut self, parent: u32, name: &str, hash: u32) -> u32 {
        let mut c = self.nodes[parent as usize].first;
        while c != NONE {
            if self.matches_name(self.nodes[c as usize].key, name, hash) {
                return c;
            }
            c = self.nodes[c as usize].next;
        }
        let start = self.names.len() as u32;
        self.names.push_str(name);
        let key = Key::Name {
            start,
            len: self.names.len() as u32 - start,
            hash,
        };
        self.push_child(parent, key)
    }

    /// Detaches a node's children: a subsumed subtree is unlinked, not freed.
    fn unlink_children(&mut self, n: u32) {
        let node = &mut self.nodes[n as usize];
        node.first = NONE;
        node.last = NONE;
    }

    /// Combines `new` onto node `n` and detaches subsumed children.
    fn finish_mark(&mut self, n: u32, new: Mark) {
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
}

impl Writes {
    /// Records a write at `at`, which is never empty.
    pub(crate) fn mark(&mut self, at: At<'_>, new: Mark) {
        debug_assert!(!at.is_empty(), "a write has a path");
        let element = removes_an_element(at, &new);
        let mut new = new;
        let mut n = 0u32;
        for i in 0..at.len() {
            if let Some(above) = self.nodes[n as usize].mark.as_mut() {
                return absorb(above, at, i, new);
            }
            // An element's removal rewrites its array, marked here, unless
            // an owned ancestor, met above, applied it instead.
            if element && i + 1 == at.len() {
                new = Mark::Set(None);
                break;
            }
            n = self.child(n, at, i);
        }
        self.finish_mark(n, new);
    }

    /// Records a write at a root key (`root`), or at `root.field` when `field`
    /// is `Some`, without allocating temporary `Seg`s.
    pub(crate) fn mark_root_key(&mut self, root: &str, field: Option<&str>, new: Mark) {
        use crate::expr::path::key_hash;
        let root_hash = key_hash(root);
        let mut n = self.child_name(0, root, root_hash);
        if let Some(field) = field {
            let field_hash = key_hash(field);
            if let Some(above) = self.nodes[n as usize].mark.as_mut() {
                match above {
                    Mark::Set(Some(value)) => {
                        let applied = match new {
                            Mark::Set(Some(v)) => {
                                rules::put(value, rules::Hop::Key(field), v).is_some()
                            }
                            _ => false,
                        };
                        if !applied {
                            *above = Mark::Set(None);
                        }
                    }
                    Mark::Set(None) => {}
                    Mark::Removed | Mark::Appended { .. } => *above = Mark::Set(None),
                }
                return;
            }
            n = self.child_name(n, field, field_hash);
        }
        self.finish_mark(n, new);
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

    /// What a read of `at` finds: the owned tree is current, only the view
    /// knows, or the log holds the answer.
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

    /// Whether nothing is marked and no reorder is flagged.
    pub(crate) fn is_empty(&self) -> bool {
        let root = &self.nodes[0];
        root.first == NONE && !root.reorder
    }

    /// The checkpoint's first phase, with the view `root` alive: every mark
    /// that needs the view takes what it needs from it, a value-less `Set`
    /// its value, an unowned `Appended` its tail, a reorder flag its key
    /// order, so the second phase needs no view.
    /// Returns how many marks it converted from the view.
    ///
    /// The view is looked up only below a node that reads it: a mark that
    /// already holds its owned value, or a removal, takes nothing from it,
    /// and that is most marks under capture.
    pub(crate) fn settle(&mut self, root: DataValue<'_>) -> u32 {
        /// Whether settling `node` reads the view at it.
        fn reads_view(node: &Node) -> bool {
            match &node.mark {
                Some(Mark::Set(None) | Mark::Appended { owned: None, .. }) => true,
                Some(Mark::Set(Some(_)) | Mark::Removed | Mark::Appended { .. }) => false,
                None => node.reorder || node.first != NONE,
            }
        }
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
                let child = &nodes[c as usize];
                let below = if reads_view(child) {
                    here.and_then(|v| rules::view_child(v, Writes::key_ref(names, child.key)))
                } else {
                    None
                };
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
                        if let Some(below) = rules::owned_child_mut(here, key) {
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
