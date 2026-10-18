//! The run context: the only writer of a run.
//!
//! It holds the message's context in two parts. The owned tree is the source
//! of truth at every checkpoint. The run view is one arena view of it,
//! built with `view_in` at the first read, so its leaves borrow the owned
//! tree and only its spines live in the run arena. While the view exists,
//! the owned tree is not written: every write is spliced into the view and
//! marked in the write log, and a checkpoint converts the marked paths,
//! drops the view, resets the arena and applies them.
//!
//! The view lives in a [`self_cell`](mod@self_cell) whose owner carries the `Program`'s
//! lifetime `'p`: datalogic's `evaluate` ties the compiled `Logic` to
//! its result, so only an expression that outlives the view's own lifetime
//! can have its result spliced in, and the owner's `'p` is what proves that
//! inside the cell's `for<'q>` closures.

use super::size;
use super::splice::{self, Resolved};
use super::writes::{At, Lookup, Mark, Writes};
use crate::base::Change;
use crate::expr::arena;
use crate::expr::path;
use crate::expr::{Filled, Path, Seg};
use bumpalo::Bump;
use datavalue::{DataValue, OwnedDataValue};
use self_cell::self_cell;
use std::borrow::Cow;
use std::marker::PhantomData;

/// What the run view's cell owns: the owned context and the run arena.
pub(crate) struct Owner<'p> {
    /// The message's context, the source of truth at every checkpoint.
    pub(crate) ctx: OwnedDataValue,
    /// The run arena the view's spines live in.
    pub(crate) arena: Bump,
    _program: PhantomData<&'p ()>,
}

/// The view's root. Covariant, so a reader can shorten it.
pub(crate) type Root<'q> = &'q DataValue<'q>;

self_cell!(
    /// The owner and the view built over it.
    pub(crate) struct RunView<'p> {
        owner: Owner<'p>,

        #[covariant]
        dependent: Root,
    }
);

/// The run view's tuning knobs. `Default` holds the measured starting values.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tuning {
    /// Whether an owned write larger than the view checkpoints instead of
    /// being copied in.
    pub(crate) invalidate: bool,
    /// An owned write is large when it passes `max(invalidate_floor, live)`.
    pub(crate) invalidate_floor: usize,
    /// The garbage trigger is `max(garbage_floor, live)`.
    pub(crate) garbage_floor: usize,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            invalidate: true,
            invalidate_floor: 64 << 10,
            garbage_floor: 64 << 10,
        }
    }
}

/// What a run did, counted rather than timed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Stats {
    /// Views built.
    pub(crate) views: u32,
    /// Checkpoints taken.
    pub(crate) checkpoints: u32,
    /// Large owned writes that checkpointed instead of being copied in.
    pub(crate) invalidations: u32,
    /// Marked paths the checkpoint converted from the view.
    pub(crate) converted: u32,
    /// The largest `live` seen: the view's spine bytes right after a build.
    pub(crate) live_max: usize,
    /// The largest run-arena bytes in use seen at a tick, a checkpoint or the
    /// finish.
    pub(crate) peak_arena: usize,
}

/// Where the context lives.
enum State<'p> {
    /// Only the owned tree exists.
    Owned(Owner<'p>),
    /// The owned tree and a view of it, with writes pending in the log.
    Viewed(RunView<'p>),
    /// Between states, and after the context went back to its slot.
    Taken,
}

/// The shared references a host call holds across its `.await`: never a
/// `&RunView`, whose `Bump` makes it `!Sync`.
#[derive(Clone, Copy)]
pub(crate) struct Reader<'r> {
    /// The view's root.
    pub(crate) root: &'r DataValue<'r>,
    /// The owned tree, current as of the last checkpoint.
    pub(crate) owned: &'r OwnedDataValue,
    /// What changed since.
    pub(crate) log: &'r Writes,
}

impl<'r> Reader<'r> {
    /// The value at `path`: borrowed from the owned tree unless the log says
    /// it changed, else converted from the view.
    pub(crate) fn get(self, path: &Path) -> Option<Cow<'r, OwnedDataValue>> {
        self.find(path).map(Filled::into_cow)
    }

    /// [`Self::get`] for a path already resolved against the view, as a
    /// host call's first write resolves it.
    pub(crate) fn get_resolved(self, path: &Path, r: Resolved) -> Option<Cow<'r, OwnedDataValue>> {
        self.find_resolved(path, r).map(Filled::into_cow)
    }

    /// Where the value at `path` is, lent from wherever it is: the owned
    /// tree unless the log says it changed, else the log's owned value, else
    /// the view, which `render` serializes as it is.
    pub(crate) fn find(self, path: &Path) -> Option<Filled<'r>> {
        if self.log.is_empty() {
            return path.get(self.owned).map(Filled::lent);
        }
        let r = splice::resolve(*self.root, path.segs())?;
        self.find_resolved(path, r)
    }

    /// [`Self::find`] for a path already resolved against the view.
    fn find_resolved(self, path: &Path, r: Resolved) -> Option<Filled<'r>> {
        if self.log.is_empty() {
            return path.get(self.owned).map(Filled::lent);
        }
        match self.log.lookup(At::new(path.segs(), r.kinds)) {
            Lookup::Clean => path.get(self.owned).map(Filled::lent),
            Lookup::Owned(v) => v.map(Filled::lent),
            Lookup::Dirty => splice::get(*self.root, path.segs()).map(Filled::View),
        }
    }
}

/// The run context: the only writer of a run. It owns the message's
/// context for the run and puts it back when the run finishes or is dropped.
pub(crate) struct RunContext<'m, 'p> {
    /// Where the context goes back to: the message's.
    slot: &'m mut OwnedDataValue,
    /// The context, owned or viewed.
    state: State<'p>,
    /// Writes since the view was built.
    log: Writes,
    /// Arena bytes in use right after the view was built: its spines.
    live: usize,
    /// The knobs.
    tuning: Tuning,
    /// What the run did so far.
    stats: Stats,
}

/// A write against the view, inside the cell. `'q` is the view's lifetime;
/// anything borrowed for `'p` may go into it.
pub(crate) struct Edit<'e, 'q, 'p> {
    /// The cell's owner.
    owner: &'q Owner<'p>,
    /// The view's root, replaced by each write.
    root: &'e mut Root<'q>,
    /// Where each write is marked.
    log: &'e mut Writes,
}

impl<'q, 'p> Edit<'_, 'q, 'p> {
    /// Mark a write that resolved to `hops` against `before`, creating from
    /// hop `created` on, with `leaf` the owned value it put at the end of the
    /// path, when the writer had one.
    ///
    /// - Nothing created: the path itself.
    /// - Created from a hop past an array's end: the array, as appended from
    ///   its old length, since the padding `null`s are new elements too.
    /// - Created otherwise: the first created node, so a later removal below
    ///   it cannot leave the created containers out of the owned tree.
    fn mark_new(
        &mut self,
        before: DataValue<'q>,
        at: At<'_>,
        created: Option<usize>,
        leaf: Option<OwnedDataValue>,
    ) {
        let Some(i) = created else {
            return self.log.mark(at, Mark::Set(leaf));
        };
        let below = |v| path::chain(&at.segs()[i + 1..], v);
        if let Some(j) = at.index(i)
            && let Some(DataValue::Array(items)) = splice::get(before, &at.segs()[..i])
            && j >= items.len()
        {
            let from = items.len();
            let owned = leaf.map(|v| {
                let mut tail = vec![OwnedDataValue::Null; j - from];
                tail.push(below(v));
                tail
            });
            return self.log.mark(at.prefix(i), Mark::Appended { from, owned });
        }
        self.log.mark(at.prefix(i + 1), Mark::Set(leaf.map(below)));
    }

    /// The cell's owner, for strings the `Program` owns.
    pub(crate) fn owner(&self) -> &'q Owner<'p> {
        self.owner
    }

    /// The run arena.
    pub(crate) fn arena(&self) -> &'q Bump {
        &self.owner.arena
    }

    /// The view's root by value.
    pub(crate) fn root(&self) -> DataValue<'q> {
        **self.root
    }

    /// The root by reference, as `evaluate` takes it.
    pub(crate) fn root_ref(&self) -> &'q DataValue<'q> {
        self.root
    }

    /// The view's value at `path`, writes so far included.
    pub(crate) fn get(&self, path: &Path) -> Option<DataValue<'q>> {
        splice::get(self.root(), path.segs())
    }

    /// Write `value` at `path`. `owned` is the same value as an owned tree,
    /// when the writer built one anyway, so the checkpoint need not convert
    /// it again. Returns whether it wrote.
    pub(crate) fn set(
        &mut self,
        path: &Path,
        value: DataValue<'q>,
        owned: Option<OwnedDataValue>,
    ) -> bool {
        self.set_segs(path.segs(), value, owned)
    }

    /// [`Self::set`] at segments.
    pub(crate) fn set_segs(
        &mut self,
        segs: &[Seg],
        value: DataValue<'q>,
        owned: Option<OwnedDataValue>,
    ) -> bool {
        let before = self.root();
        let Some((after, r)) = splice::set_resolved(self.arena(), before, segs, value) else {
            return false;
        };
        *self.root = self.arena().alloc(after);
        self.mark_new(before, At::new(segs, r.kinds), r.created, owned);
        true
    }

    /// Overwrites values of the small object at `object` where they are, so
    /// it keeps its keys and their order, as an owned in-place update keeps
    /// them. `values` gets the object's pairs and gives one entry per pair,
    /// in the object's order: the path of the pair's key (`object` and that
    /// key) and its new value, or `None` to keep the value there. `false`,
    /// writing nothing, when `object` holds no object or `values` declines
    /// it.
    ///
    /// One new value is marked at its own key's path, so the checkpoint
    /// assigns that key alone, where it is, and never converts the object.
    /// Several are marked as one write of the object, which the checkpoint
    /// converts: a walk of the log per changed key, repeated at every write,
    /// costs more than that conversion once a run writes the object several
    /// times before its checkpoint, as the multi-workflow port does. A key
    /// that comes twice is marked as the object too, since the owned tree
    /// assigns a key's first pair only.
    pub(crate) fn overwrite<'v, const N: usize>(
        &mut self,
        object: &Path,
        values: impl FnOnce(
            &'q [(&'q str, DataValue<'q>)],
        ) -> Option<[(&'v Path, Option<DataValue<'q>>); N]>,
    ) -> bool {
        let before = self.root();
        let Some(DataValue::Object(pairs)) = splice::get(before, object.segs()) else {
            return false;
        };
        let Some(values) = values(pairs) else {
            return false;
        };
        debug_assert!(
            pairs.len() == N
                && pairs.iter().zip(&values).all(|((k, _), (leaf, _))| {
                    matches!(leaf.segs().split_last(),
                        Some((last, parent)) if parent == object.segs() && *last.key == **k)
                }),
            "`values` names the object's keys, in its order"
        );
        if values.iter().all(|(_, value)| value.is_none()) {
            return true;
        }
        let arena = self.arena();
        let updated = DataValue::Object(
            arena.alloc_slice_fill_with(N, |i| (pairs[i].0, values[i].1.unwrap_or(pairs[i].1))),
        );
        let Some((after, r)) = splice::set_resolved(arena, before, object.segs(), updated) else {
            unreachable!("an object in the view takes a write")
        };
        debug_assert_eq!(r.created, None, "the object was there");
        *self.root = arena.alloc(after);
        let mut changed = values.iter().filter(|(_, value)| value.is_some());
        let leaf = match (changed.next(), changed.next()) {
            (Some((leaf, _)), None) => Some(leaf),
            _ => None,
        };
        let each_once = || (1..N).all(|i| pairs[..i].iter().all(|(k, _)| *k != pairs[i].0));
        let at = match leaf {
            Some(leaf) if each_once() => leaf.segs(),
            _ => object.segs(),
        };
        self.log.mark(At::new(at, r.kinds), Mark::Set(None));
        true
    }

    /// Remove `path`. Returns whether something was there.
    pub(crate) fn remove(&mut self, path: &Path) -> bool {
        self.remove_segs(path.segs())
    }

    /// [`Self::remove`] at segments.
    pub(crate) fn remove_segs(&mut self, segs: &[Seg]) -> bool {
        let before = self.root();
        let Some((after, kinds)) = splice::remove_resolved(self.arena(), before, segs) else {
            return false;
        };
        let at = At::new(segs, kinds);
        *self.root = self.arena().alloc(after);
        // A key that comes back later moves to the end of its object, which
        // the checkpoint's in-place assignment would not do.
        if !at.is_index(at.len() - 1) {
            self.log.reorder(at.prefix(at.len() - 1));
        }
        self.log.mark(at, Mark::Removed);
        true
    }

    /// `map`'s append and extend. `Ok(false)` when the path is unreachable,
    /// which v3 treats as success with nothing written.
    pub(crate) fn append(
        &mut self,
        path: &Path,
        items: &[DataValue<'q>],
        owned: Option<Vec<OwnedDataValue>>,
    ) -> Result<bool, &'static str> {
        let before = self.root();
        let existing = splice::get(before, path.segs());
        let Some((after, r)) = splice::append(self.arena(), before, path.segs(), existing, items)?
        else {
            return Ok(false);
        };
        let at = At::new(path.segs(), r.kinds);
        *self.root = self.arena().alloc(after);
        match existing {
            Some(DataValue::Array(old)) => self.log.mark(
                at,
                Mark::Appended {
                    from: old.len(),
                    owned,
                },
            ),
            // A new array, over a missing or `null` target.
            _ => self.mark_new(before, at, r.created, owned.map(OwnedDataValue::Array)),
        }
        Ok(true)
    }

    /// `map`'s write to a root slot: marked as one write per key merged, or
    /// one for the slot when it is replaced.
    pub(crate) fn merge_root(
        &mut self,
        key: &str,
        value: DataValue<'q>,
        owned: Option<OwnedDataValue>,
    ) {
        let before = self.root();
        let slot = before
            .as_object()
            .and_then(|slots| slots.iter().find(|(k, _)| *k == key))
            .map(|(_, v)| *v);
        *self.root = self
            .arena()
            .alloc(splice::merge_root(self.arena(), before, key, value));
        match (slot, value) {
            (Some(DataValue::Object(_)), DataValue::Object(new_pairs)) => {
                let owned: Vec<Option<OwnedDataValue>> = match owned {
                    Some(OwnedDataValue::Object(pairs)) => {
                        pairs.into_iter().map(|(_, v)| Some(v)).collect()
                    }
                    _ => vec![None; new_pairs.len()],
                };
                for ((k, _), v) in new_pairs.iter().zip(owned) {
                    let segs = [Seg::key(key), Seg::key(k)];
                    self.log.mark(At::new(&segs, 0), Mark::Set(v));
                }
            }
            _ => self
                .log
                .mark(At::new(&[Seg::key(key)], 0), Mark::Set(owned)),
        }
    }
}

impl<'m, 'p> RunContext<'m, 'p> {
    /// Take the context out of `slot` for the run; it goes back when the run
    /// finishes or is dropped.
    pub(crate) fn new(slot: &'m mut OwnedDataValue, tuning: Tuning) -> Self {
        let ctx = std::mem::take(slot);
        Self {
            slot,
            state: State::Owned(Owner {
                ctx,
                arena: arena::take(),
                _program: PhantomData,
            }),
            log: Writes::default(),
            live: 0,
            tuning,
            stats: Stats::default(),
        }
    }

    /// What the run did so far. The engine takes the stats from
    /// [`finish`](Self::finish); the tests read them mid-run.
    #[cfg(test)]
    pub(crate) fn stats(&self) -> Stats {
        self.stats
    }

    /// Whether a view is built now.
    pub(crate) fn has_view(&self) -> bool {
        matches!(self.state, State::Viewed(_))
    }

    /// The view, built now if there is none: at the first read, not at run
    /// start.
    fn view(&mut self) -> &mut RunView<'p> {
        if let State::Owned(_) = self.state {
            let State::Owned(owner) = std::mem::replace(&mut self.state, State::Taken) else {
                unreachable!()
            };
            let view = RunView::new(owner, |o| o.arena.alloc(o.ctx.view_in(&o.arena)));
            self.live = arena::used(&view.borrow_owner().arena);
            self.stats.live_max = self.stats.live_max.max(self.live);
            self.stats.views += 1;
            self.state = State::Viewed(view);
        }
        match &mut self.state {
            State::Viewed(view) => view,
            _ => unreachable!("a run context is not used after its context went back"),
        }
    }

    /// Read against the view. `f` may evaluate any `&'p` expression; what it
    /// returns cannot borrow the view.
    pub(crate) fn read<R>(&mut self, f: impl for<'q> FnOnce(&'q Owner<'p>, Root<'q>) -> R) -> R {
        self.view().with_dependent(|owner, root| f(owner, root))
    }

    /// Write against the view, building it first.
    pub(crate) fn edit<R>(&mut self, f: impl for<'e, 'q> FnOnce(&mut Edit<'e, 'q, 'p>) -> R) -> R {
        self.view();
        let State::Viewed(view) = &mut self.state else {
            unreachable!()
        };
        let log = &mut self.log;
        view.with_dependent_mut(|owner, root| f(&mut Edit { owner, root, log }))
    }

    /// An owned write: a parse result, an applied overlay, a binding. With no
    /// view it assigns the owned tree. With a view, a value over the threshold
    /// checkpoints and assigns the owned tree, and anything smaller is
    /// copied in, spliced and marked with its owned value.
    pub(crate) fn set(
        &mut self,
        path: &Path,
        value: OwnedDataValue,
        size_hint: Option<usize>,
    ) -> bool {
        self.set_segs(path.segs(), value, size_hint)
    }

    /// [`Self::set`], first recording the write's [`Change::assign`] onto
    /// `changes` when there is a list to record it on: the old value read
    /// from the context as it was, the new one cloned for the record.
    pub(crate) fn set_recorded(
        &mut self,
        path: &Path,
        value: OwnedDataValue,
        size_hint: Option<usize>,
        changes: Option<&mut Vec<Change>>,
    ) -> bool {
        if let Some(changes) = changes {
            changes.push(Change::assign(path, self.get_or_null(path), value.clone()));
        }
        self.set(path, value, size_hint)
    }

    /// [`Self::set`] at segments: how an applied overlay writes, with no
    /// dotted path to build.
    pub(crate) fn set_segs(
        &mut self,
        segs: &[Seg],
        value: OwnedDataValue,
        size_hint: Option<usize>,
    ) -> bool {
        if self.has_view() {
            let limit = self.live.max(self.tuning.invalidate_floor);
            let large = match size_hint {
                Some(n) => n > limit,
                None => size::exceeds(&value, limit),
            };
            if !(self.tuning.invalidate && large) {
                return self.edit(|e| {
                    let av = value.to_arena(e.arena());
                    e.set_segs(segs, av, Some(value))
                });
            }
            self.checkpoint();
            self.stats.invalidations += 1;
        }
        match &mut self.state {
            State::Owned(owner) => path::set(&mut owner.ctx, segs, value),
            _ => unreachable!(),
        }
    }

    /// A write the engine makes after every task, such as the progress write:
    /// built owned when there is no view, or straight into the view from
    /// strings the `Program` owns, converting nothing until a checkpoint.
    pub(crate) fn engine_set(
        &mut self,
        path: &Path,
        owned: impl FnOnce() -> OwnedDataValue,
        view: impl for<'q> FnOnce(&'q Owner<'p>) -> DataValue<'q>,
    ) -> bool {
        match &mut self.state {
            State::Owned(owner) => path.assign(&mut owner.ctx, owned()),
            _ => self.edit(|e| {
                let value = view(e.owner());
                e.set(path, value, None)
            }),
        }
    }

    /// An engine write that updates the owned value at `path` in place,
    /// reusing what it holds: with no view, `update` gets the value there
    /// and says whether it updated it. `false`, with nothing written, when a
    /// view exists, nothing is at `path` or `update` declined, and the caller
    /// writes through [`Self::engine_set`] instead.
    pub(crate) fn engine_update(
        &mut self,
        path: &Path,
        update: impl FnOnce(&mut OwnedDataValue) -> bool,
    ) -> bool {
        match &mut self.state {
            State::Owned(owner) => path::get_mut(&mut owner.ctx, path.segs()).is_some_and(update),
            _ => false,
        }
    }

    /// Removes the value at `segs`, as a host call's applied overlay does;
    /// whether something was there.
    pub(crate) fn remove_segs(&mut self, segs: &[Seg]) -> bool {
        match &mut self.state {
            State::Owned(owner) => path::remove(&mut owner.ctx, segs).is_some(),
            _ => self.edit(|e| e.remove_segs(segs)),
        }
    }

    /// The value at `path`: borrowed from the owned tree unless the log says
    /// it changed, else converted from the view.
    pub(crate) fn get(&self, path: &Path) -> Option<Cow<'_, OwnedDataValue>> {
        match &self.state {
            State::Owned(owner) => path.get(&owner.ctx).map(Cow::Borrowed),
            State::Viewed(view) => self.reader_of(view).get(path),
            State::Taken => unreachable!("a run context is not used after its context went back"),
        }
    }

    /// [`Self::get`], owned, or `null`: a write's old value, for its
    /// `Change`.
    pub(crate) fn get_or_null(&self, path: &Path) -> OwnedDataValue {
        self.get(path).map_or(OwnedDataValue::Null, Cow::into_owned)
    }

    /// What a host call holds across its `.await`. Builds the view first.
    pub(crate) fn reader(&mut self) -> Reader<'_> {
        self.view();
        let State::Viewed(view) = &self.state else {
            unreachable!()
        };
        self.reader_of(view)
    }

    /// The reader over `view`, which is this context's.
    fn reader_of<'r>(&'r self, view: &'r RunView<'p>) -> Reader<'r> {
        Reader {
            root: view.borrow_dependent(),
            owned: &view.borrow_owner().ctx,
            log: &self.log,
        }
    }

    /// At an op boundary: checkpoint once the arena's garbage passes
    /// `max(garbage_floor, live)`, so a rebuild, which costs about `live`,
    /// runs only after at least that much garbage.
    pub(crate) fn tick(&mut self) {
        if let State::Viewed(view) = &self.state {
            let used = arena::used(&view.borrow_owner().arena);
            self.stats.peak_arena = self.stats.peak_arena.max(used);
            let garbage = used.saturating_sub(self.live);
            if garbage > self.live.max(self.tuning.garbage_floor) {
                self.checkpoint();
            }
        }
    }

    /// Settle the marks against the view, drop the view, reset the arena,
    /// then apply the marks and the reorders to the owned tree. The view is
    /// rebuilt at the next read.
    ///
    /// The context is never out of `self.state` while code that may panic
    /// runs: `settle` runs with the view still in place, and the owner is put
    /// back before `apply`. A panic in either leaves an owner that
    /// `give_back` returns to the slot: after a panic in `settle`, the
    /// context of the last checkpoint; after one in `apply`, that context
    /// with the marks applied so far.
    pub(crate) fn checkpoint(&mut self) {
        let State::Viewed(view) = &self.state else {
            return;
        };
        self.stats.peak_arena = self
            .stats
            .peak_arena
            .max(arena::used(&view.borrow_owner().arena));
        let log = &mut self.log;
        let converted = view.with_dependent(|_, root| log.settle(**root));
        let State::Viewed(view) = std::mem::replace(&mut self.state, State::Taken) else {
            unreachable!("the view was in place a moment ago")
        };
        let mut owner = view.into_owner();
        owner.arena.reset();
        self.state = State::Owned(owner);
        let State::Owned(owner) = &mut self.state else {
            unreachable!("the owner was just put back")
        };
        self.log.apply(&mut owner.ctx);
        self.live = 0;
        self.stats.checkpoints += 1;
        self.stats.converted += converted;
    }

    /// The last checkpoint; the context goes back to its slot and the arena
    /// to the pool.
    pub(crate) fn finish(mut self) -> Stats {
        self.checkpoint();
        self.give_back();
        self.stats
    }

    /// Returns the context to its slot and the arena to the pool, whatever state
    /// the run is in.
    fn give_back(&mut self) {
        let owner = match std::mem::replace(&mut self.state, State::Taken) {
            State::Owned(owner) => owner,
            State::Viewed(view) => view.into_owner(),
            State::Taken => return,
        };
        *self.slot = owner.ctx;
        arena::give(owner.arena);
    }
}

impl Drop for RunContext<'_, '_> {
    /// A run dropped early, by a host's `select!` or a dropped request, still
    /// leaves every write made so far in the message. Not while panicking:
    /// a checkpoint runs the same code the panic may have come from, and a
    /// second panic during unwinding aborts the process. The message
    /// then keeps the last checkpoint.
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.checkpoint();
        }
        self.give_back();
    }
}
