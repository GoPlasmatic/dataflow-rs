//! `Ctx`: a function's view of one call, and the static facts about it.

use super::erased::BoundCall;
use super::overlay::Overlay;
use crate::base::{
    AuthoredPath, CancelToken, Change, Error, ErrorKind, ErrorRecord, Instant, PathSeg, ProgramId,
    Result, TaskIx, WorkflowIx,
};
use crate::expr::arena::with_arena;
use crate::expr::path;
use crate::expr::{
    Expr, ExprEngine, ExprVisitor, Filled, FromValue, Interpolated, Output, Path, Rendered,
    ResolveExpr, Resolver, Seg, secret_refs,
};
use crate::state::{Lookup, Reader, splice};
use bumpalo::Bump;
use datavalue::{DataValue, OwnedDataValue};
use serde_json::Value;
use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

/// Static facts about one call: where it sits in the program, and which
/// iteration it is.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct CallInfo<'a> {
    /// The program the call belongs to. It tells the indices of one build
    /// from another's across a reload.
    pub program: ProgramId,
    /// The workflow's position in the program.
    pub workflow: WorkflowIx,
    /// The task's position in the program.
    pub task: TaskIx,
    /// The name the function was called by.
    pub function: &'a str,
    /// The workflow's id.
    pub workflow_id: &'a str,
    /// The task's id.
    pub task_id: &'a str,
    /// The channel the run was started on; `None` when the run takes every
    /// active workflow.
    pub channel: Option<&'a str>,
    /// The loop counter, inside a workflow's loop.
    pub loop_counter: Option<i64>,
    /// The element's index, inside a `for_each` fan-out.
    pub element_index: Option<usize>,
}

/// A [`Function`](super::Function)'s view of one call.
///
/// It reads the run's single view of the context, so no view is built per
/// call (R38), and keeps the call's writes in an overlay of its own: a read
/// sees them over the run's context, and they reach the message only when
/// the call returns `Ok` (whatever the [`Outcome`](super::Outcome)). An `Err`
/// drops them, and their `Change`s, and keeps the records the call added.
///
/// Once the call has written, each evaluation splices the overlay onto the
/// view in an arena borrowed from a pool for that evaluation only. `Ctx`
/// holds no arena, so it is `Send` and `Sync`. A host never builds one: the
/// engine hands it to [`Function::call`](super::Function::call) and to each
/// [`CallLayer`](super::CallLayer), and [`testing::call`](super::testing::call)
/// to one call outside a run:
///
/// ```
/// # extern crate dataflow_rs_v4 as dataflow_rs;
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// use dataflow_rs::datavalue::OwnedDataValue;
/// use dataflow_rs::function::testing::{self, Write};
/// use dataflow_rs::run::Change;
/// use dataflow_rs::{Ctx, Function, Message, Outcome, Path, Result};
/// use serde_json::json;
///
/// /// Counts visits at `data.visits`, and drops the stale `data.cache`.
/// struct Visit;
///
/// impl Function for Visit {
///     type Config = ();
///
///     async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
///         let visits: Path = "data.visits".parse()?;
///         let seen = ctx.get(&visits).and_then(|v| v.as_i64()).unwrap_or(0);
///         ctx.set(&visits, OwnedDataValue::from(&json!(seen + 1)))?;
///         // The call reads its own write at once; the message, once the
///         // call returns `Ok`.
///         assert_eq!(ctx.get(&visits).and_then(|v| v.as_i64()), Some(seen + 1));
///         ctx.remove(&"data.cache".parse()?)?;
///         Ok(Outcome::Ok)
///     }
/// }
///
/// let dv = |v: serde_json::Value| OwnedDataValue::from(&v);
/// let data = dv(json!({"visits": 1, "cache": "stale"}));
/// let called = testing::call(Visit, json!({}))
///     .message(Message::from_parts(OwnedDataValue::Null, data, OwnedDataValue::Null))
///     .await;
/// let visits: Path = "data.visits".parse().unwrap();
/// assert_eq!(called.writes[0], (visits, Write::Set(dv(json!(2)))));
/// assert_eq!(called.writes[1].1, Write::Removed);
/// assert_eq!(
///     called.changes[0],
///     Change::write("data.visits", dv(json!(1)), dv(json!(2)))
/// );
/// assert_eq!(called.message.context().data(), &dv(json!({"visits": 2})));
/// # });
/// ```
pub struct Ctx<'r> {
    /// The run's view, the owned tree and the run's log.
    reader: Reader<'r>,
    /// A fan-out element's bindings, under the call's own writes.
    binding: Option<&'r Binding<'r>>,
    payload: &'r OwnedDataValue,
    exprs: &'r ExprEngine,
    info: CallInfo<'r>,
    /// The function and its config: `emit_default` writes through its
    /// primary output, and `input` resolves its config.
    callee: &'r dyn BoundCall,
    deadline: Option<Instant>,
    cancel: Option<&'r CancelToken>,
    /// Whether `set` and `remove` record `Change`s (I67).
    capture: bool,
    /// The call's writes, in order.
    overlay: Overlay,
    /// The call's `Change`s, in write order.
    changes: Vec<Change>,
    /// The records the call added.
    errors: Vec<ErrorRecord>,
}

/// What the runtime hands a new [`Ctx`]. A fan-out builds one for the
/// task and copies it for each element, with the element's binding and
/// index.
#[derive(Clone, Copy)]
pub(crate) struct CallStart<'r> {
    /// The run's reader, taken in its own statement: never a `&RunView`.
    pub(crate) reader: Reader<'r>,
    /// A fan-out element's bindings.
    pub(crate) binding: Option<&'r Binding<'r>>,
    /// The message's payload.
    pub(crate) payload: &'r OwnedDataValue,
    /// The engine the program was compiled with.
    pub(crate) exprs: &'r ExprEngine,
    /// The call's static facts.
    pub(crate) info: CallInfo<'r>,
    /// When the run must end.
    pub(crate) deadline: Option<Instant>,
    /// The run's cancellation token.
    pub(crate) cancel: Option<&'r CancelToken>,
    /// Whether the call records its `Change`s.
    pub(crate) capture: bool,
}

/// A callee's primary output, as [`Ctx::emit_default`] writes through it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Primary<'r> {
    /// The config declares none.
    Undeclared,
    /// The config's primary output, the field named, which the input left
    /// unset: an `Option<Output>`.
    Unset(&'r str),
    /// The config's primary output, as the call site set it.
    Set(&'r Output),
}

/// A fan-out element's bindings: `temp_data.<as>` holds the element and
/// `temp_data.<as>_index` its index. They sit between the run's context and
/// the call's own writes, and are never written to the run: the settle
/// replays the call's writes at their paths, where the binding is not.
pub(crate) struct Binding<'r> {
    /// `temp_data.<as>`.
    pub(crate) item: &'r Path,
    /// The element.
    pub(crate) value: &'r OwnedDataValue,
    /// `temp_data.<as>_index`.
    pub(crate) index: &'r Path,
    /// The element's index.
    pub(crate) index_value: OwnedDataValue,
}

impl Binding<'_> {
    /// `root` with both bindings written, in `arena`.
    fn splice_onto<'a>(&'a self, root: DataValue<'a>, arena: &'a Bump) -> DataValue<'a> {
        let root =
            splice::set(arena, root, self.item.segs(), self.value.view_in(arena)).unwrap_or(root);
        splice::set(
            arena,
            root,
            self.index.segs(),
            self.index_value.view_in(arena),
        )
        .unwrap_or(root)
    }

    /// What the bindings say `path` holds, over the run's `root`: `Some` for
    /// a path at, below or above one of them, `None` for any other.
    fn get<'s>(
        &'s self,
        path: &Path,
        root: DataValue<'_>,
    ) -> Option<Option<Cow<'s, OwnedDataValue>>> {
        for (at, value) in [(self.item, self.value), (self.index, &self.index_value)] {
            if let Some(rest) = below(path, at) {
                return Some(path::get(value, rest).map(Cow::Borrowed));
            }
        }
        if [self.item, self.index]
            .iter()
            .any(|at| below(at, path).is_some())
        {
            return Some(with_arena(|arena| {
                splice::get(self.splice_onto(root, arena), path.segs())
                    .map(|v| Cow::Owned(v.to_owned()))
            }));
        }
        None
    }
}

/// The segments of `path` below `prefix`, when `path` is `prefix` or below
/// it. A binding's path is a root and a key, so segments compare by key.
fn below<'p>(path: &'p Path, prefix: &Path) -> Option<&'p [Seg]> {
    let (path, prefix) = (path.segs(), prefix.segs());
    (path.len() >= prefix.len() && path.iter().zip(prefix).all(|(a, b)| a.key == b.key))
        .then(|| &path[prefix.len()..])
}

/// The root a call's writes sit on: the run's view, with the binding
/// written in `arena` when there is one.
fn base_in<'a>(
    reader: Reader<'a>,
    binding: Option<&'a Binding<'a>>,
    arena: &'a Bump,
) -> DataValue<'a> {
    match binding {
        None => *reader.root,
        Some(binding) => binding.splice_onto(*reader.root, arena),
    }
}

/// `f` with the root a call's writes sit on, taking an arena only when there
/// is a binding to write.
fn with_base<R>(
    reader: Reader<'_>,
    binding: Option<&Binding<'_>>,
    f: impl FnOnce(DataValue<'_>) -> R,
) -> R {
    match binding {
        None => f(*reader.root),
        Some(_) => with_arena(|arena| f(base_in(reader, binding, arena))),
    }
}

impl<'r> Ctx<'r> {
    /// A context for one call of `callee`.
    pub(crate) fn new(start: CallStart<'r>, callee: &'r dyn BoundCall) -> Self {
        Self {
            reader: start.reader,
            binding: start.binding,
            payload: start.payload,
            exprs: start.exprs,
            info: start.info,
            callee,
            deadline: start.deadline,
            cancel: start.cancel,
            capture: start.capture,
            overlay: Overlay::default(),
            changes: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// The static facts about this call.
    pub fn info(&self) -> &CallInfo<'r> {
        &self.info
    }

    /// The function this call reaches and its config, bound at compile.
    pub(crate) fn callee(&self) -> &'r dyn BoundCall {
        self.callee
    }

    /// The callee's config resolved for this call (I130): every field the
    /// input set, keyed by its authored key, in declaration order, as
    /// [`FunctionConfig::resolve`](super::FunctionConfig::resolve) gives it
    /// over the context as this call sees it, a fan-out element's binding
    /// included. A hand-written config resolves to `null` unless it says
    /// otherwise.
    ///
    /// A field whose expression reads a secret is `null`, the one redaction
    /// the engine writes, as [`TraceOptions::redact`](crate::observe::TraceOptions::redact)
    /// writes a redacted path's value, and it is never evaluated, so not
    /// even its error can carry the value: a logging layer or a dry run
    /// reads the input without reading a secret (R37). The function itself
    /// reads its own fields, secrets included, through [`Self::eval`].
    ///
    /// # Errors
    ///
    /// The first field's that fails: [`ErrorKind::Budget`] when the
    /// operation ceiling is exceeded, [`ErrorKind::Eval`] for any other
    /// evaluation failure.
    pub fn input(&self) -> Result<OwnedDataValue> {
        let mut input = self.callee.resolve(&mut Redacted(self))?;
        // The walk the secret checks trust: whatever a field's resolve did,
        // the value where its secret-reading expression sits is `null`.
        self.callee.visit(&mut Redact(&mut input));
        Ok(input)
    }

    /// `cfg` resolved for this call, unredacted:
    /// [`FunctionConfig::resolve`](super::FunctionConfig::resolve) over the
    /// context as this call sees it, a fan-out element's binding included,
    /// with every expression evaluated, a secret-reading one too. It is
    /// [`Self::input`]'s counterpart: `input` is what a layer or a dry run
    /// reads, and this is the function's own view of its config, which it
    /// may hand on whole.
    ///
    /// # Errors
    ///
    /// The first field's that fails: [`ErrorKind::Budget`] when the
    /// operation ceiling is exceeded, [`ErrorKind::Eval`] for any other
    /// evaluation failure, [`ErrorKind::Config`] for a serde field that does
    /// not serialize.
    pub fn resolve<C: super::FunctionConfig>(&self, cfg: &C) -> Result<OwnedDataValue> {
        cfg.resolve(&mut &*self)
    }

    /// The message's payload, which no expression reads.
    pub fn payload(&self) -> &OwnedDataValue {
        self.payload
    }

    /// The engine the program was compiled with, for an evaluation over a
    /// root of the function's own.
    pub fn exprs(&self) -> &ExprEngine {
        self.exprs
    }

    /// The secret at the dotted path `name`, from the engine's store. What a
    /// function does with it is the function's: the engine never records it.
    pub fn secret(&self, name: &str) -> Option<&OwnedDataValue> {
        self.exprs.secrets().get(name)
    }

    /// When the run must end, if it has a deadline: a run this function
    /// starts passes it on, or an earlier one (R60).
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// The run's cancellation token, if it has one: a child of it cancels a
    /// run this function starts along with this one (R60).
    pub fn cancel(&self) -> Option<&CancelToken> {
        self.cancel
    }

    /// The value at `path` as this call sees it: its own writes, over a
    /// fan-out element's bindings, over the run's context. Borrowed unless
    /// it must be put together, when it is converted: a value only the run's
    /// view knows, or one with this call's writes or a binding below `path`,
    /// such as `temp_data` while `temp_data.p` is bound.
    pub fn get(&self, path: &Path) -> Option<Cow<'_, OwnedDataValue>> {
        self.find(path).map(Filled::into_cow)
    }

    /// Where the value at `path` is, as this call sees it, lent from
    /// wherever it is: the call's writes, the binding, then the run's log,
    /// owned tree or view. Only a value the call wrote below `path` is
    /// converted, off the root `eval` sees.
    fn find(&self, path: &Path) -> Option<Filled<'_>> {
        match self.overlay.lookup(path) {
            Lookup::Clean => {}
            Lookup::Owned(v) => return v.map(Filled::lent),
            Lookup::Dirty => {
                return with_arena(|arena| {
                    splice::get(*self.root_in(arena), path.segs())
                        .map(|v| Filled::Owned(Cow::Owned(v.to_owned())))
                });
            }
        }
        if let Some(found) = self.binding.and_then(|b| b.get(path, *self.reader.root)) {
            return found.map(Filled::Owned);
        }
        self.reader.find(path)
    }

    /// `doc` with each hole looked up once, as [`Self::get`] would, and
    /// lent where it was found; a hole that finds nothing yields its
    /// default. No tree is built (R54): the [`Rendered`] serializes the
    /// document's skeleton with each value in place.
    pub fn render<'s>(&'s self, doc: &'s Interpolated) -> Rendered<'s> {
        doc.render_with(|hole| {
            hole.path
                .as_ref()
                .and_then(|path| self.find(path))
                .unwrap_or(Filled::Default(&hole.default))
        })
    }

    /// Whether this call sees a value at `path`, converting none.
    fn holds(&self, path: &Path) -> bool {
        match self.overlay.lookup(path) {
            Lookup::Owned(v) => v.is_some(),
            Lookup::Dirty => {
                with_arena(|arena| splice::get(*self.root_in(arena), path.segs()).is_some())
            }
            Lookup::Clean => with_base(self.reader, self.binding, |base| {
                splice::get(base, path.segs()).is_some()
            }),
        }
    }

    /// Evaluates `expr` against the context as this call sees it. A folded
    /// constant is lent from `expr` and takes no arena; anything else is
    /// evaluated in a pooled arena and converted to `T` before the arena goes
    /// back.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Budget`](crate::base::ErrorKind::Budget) when the
    /// operation ceiling is exceeded; [`ErrorKind::Eval`](crate::base::ErrorKind::Eval)
    /// for any other failure, including a result that is not a `T`.
    pub fn eval<'e, T: FromValue>(&self, expr: &'e Expr<T>) -> Result<T::Out<'e>> {
        if let Some(c) = expr.constant() {
            return Ok(T::lend(c));
        }
        with_arena(|arena| {
            let root = self.root_in(arena);
            let view = self.exprs.eval_in(expr.logic(), root, arena)?;
            expr.convert(view).map(T::give)
        })
    }

    /// `expr` evaluated as [`Self::eval`] evaluates it, as an owned value: a
    /// folded constant converted, anything else evaluated in a pooled arena
    /// and converted before the arena goes back.
    fn eval_owned(&self, expr: &dyn ResolveExpr) -> Result<OwnedDataValue> {
        if let Some(constant) = expr.constant_value() {
            return Ok(constant);
        }
        with_arena(|arena| {
            let root = self.root_in(arena);
            let view = self.exprs.eval_in(expr.logic(), root, arena)?;
            expr.value_of(view)
        })
    }

    /// The root this call's evaluations see: the run's view, with the binding
    /// and then the call's writes spliced on in `arena` when there are any.
    fn root_in<'a>(&'a self, arena: &'a Bump) -> &'a DataValue<'a> {
        if self.binding.is_none() && self.overlay.is_empty() {
            return self.reader.root;
        }
        let base = base_in(self.reader, self.binding, arena);
        if self.overlay.is_empty() {
            return arena.alloc(base);
        }
        arena.alloc(self.overlay.splice_onto(base, arena))
    }

    /// Assigns `value` at `path`, by `Path::set`'s rules: a hop that cannot
    /// continue writes nothing. The `Change` is recorded first, under
    /// capture, with the value this call sees there now, so a second write to
    /// one path leaves two `Change`s and one value. At a root the write
    /// replaces the root, and its `Change` says so
    /// ([`replaced`](Change::replaced), I106).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`](crate::base::ErrorKind::Config) when `path` is a
    /// root and `value` is not an object (R52).
    pub fn set(&mut self, path: &Path, value: OwnedDataValue) -> Result<()> {
        path.check_set(&value)?;
        // A first write with no binding is resolved against the run's view
        // once: the walk says whether the write lands, and whether anything
        // is there to record as the old value.
        let first = (self.overlay.is_empty() && self.binding.is_none())
            .then(|| splice::resolve(*self.reader.root, path.segs()));
        if self.capture {
            let old_value = match first {
                // Below a scalar, or creating the node at the path or above
                // it: nothing is there, or a `null` element, read as `null`.
                Some(None) => None,
                Some(Some(r)) if r.created.is_some() => None,
                Some(Some(r)) => self.reader.get_resolved(path, r),
                None => self.get(path),
            };
            let mut change = Change::write(
                Arc::clone(path.dotted()),
                old_value.map_or(OwnedDataValue::Null, Cow::into_owned),
                value.clone(),
            );
            change.replaced = path.is_root();
            self.changes.push(change);
        }
        if let Some(resolved) = first {
            self.overlay.push(path, value, resolved.is_some());
            return Ok(());
        }
        let Self {
            reader,
            binding,
            overlay,
            ..
        } = self;
        with_base(*reader, *binding, |base| overlay.set(base, path, value));
        Ok(())
    }

    /// Writes `value` through `out`, as [`Self::set`] does: at its path
    /// when it is literal, else at the path it evaluates to now, against the
    /// context as this call sees it.
    ///
    /// # Errors
    ///
    /// [`Self::eval`]'s, when `out` is computed and does not give a path;
    /// [`Self::set`]'s.
    pub fn emit(&mut self, out: &Output, value: OwnedDataValue) -> Result<()> {
        let path = match out.constant() {
            Some(path) => Cow::Borrowed(path),
            None => self.eval(out.path())?,
        };
        self.set(&path, value)
    }

    /// The callee's primary output, the one [`Self::emit_default`] writes
    /// through, as the call site set it: `None` when the config declares
    /// none, or leaves an optional one unset. A layer reads it to know
    /// whether, and where, an answer would be written.
    pub fn primary_output(&self) -> Option<&Output> {
        match self.callee.primary() {
            Primary::Set(out) => Some(out),
            Primary::Unset(_) | Primary::Undeclared => None,
        }
    }

    /// [`Self::emit`] through the function's primary output, the one its
    /// config's [`FunctionConfig::primary_output`](super::FunctionConfig::primary_output)
    /// names.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] when the config has no primary output, or
    /// leaves an optional one unset, which the error names; [`Self::emit`]'s.
    pub fn emit_default(&mut self, value: OwnedDataValue) -> Result<()> {
        let function = self.info.function;
        let callee = self.callee;
        match callee.primary() {
            Primary::Set(out) => self.emit(out, value),
            Primary::Unset(field) => Err(Error::new(
                ErrorKind::Config,
                format!("`{function}`'s primary output `{field}` is not set"),
            )),
            Primary::Undeclared => Err(Error::new(
                ErrorKind::Config,
                format!("`{function}` has no primary output"),
            )),
        }
    }

    /// Removes the value at `path`, recording the removal under capture.
    /// Nothing there is nothing to do, and records nothing.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`](crate::base::ErrorKind::Config) when `path` is a
    /// root (R52).
    pub fn remove(&mut self, path: &Path) -> Result<()> {
        path.check_remove()?;
        if self.capture {
            let Some(old_value) = self.get(path).map(Cow::into_owned) else {
                return Ok(());
            };
            self.changes
                .push(Change::removal(Arc::clone(path.dotted()), old_value));
        } else if !self.holds(path) {
            return Ok(());
        }
        self.overlay.remove(path);
        Ok(())
    }

    /// Adds a record to the message's errors, whatever the call returns. One
    /// with no timestamp gets the run's.
    pub fn record_error(&mut self, e: ErrorRecord) {
        self.errors.push(e);
    }

    /// The call's writes, its `Change`s and the records it added.
    pub(crate) fn settle(self) -> (Overlay, Vec<Change>, Vec<ErrorRecord>) {
        (self.overlay, self.changes, self.errors)
    }
}

/// A call's context resolves a config's fields against the context as the
/// call sees it, as [`Ctx::eval`] and [`Ctx::render`] do, secrets read: a
/// host's [`ConfigField::resolve`](crate::expr::ConfigField::resolve) calls
/// it. A function resolves its whole config with [`Ctx::resolve`].
impl Resolver for &Ctx<'_> {
    fn eval(&mut self, expr: &dyn ResolveExpr) -> Result<OwnedDataValue> {
        Ctx::eval_owned(self, expr)
    }

    fn render(&mut self, doc: &Interpolated) -> Result<OwnedDataValue> {
        Ok(Ctx::render(self, doc).to_owned())
    }
}

/// Whether `raw` reads a secret anywhere, a computed name included.
fn reads_secret(raw: &Value) -> bool {
    secret_refs(raw).next().is_some()
}

/// The resolver behind [`Ctx::input`]: an expression that reads a secret
/// is `null`, and is never evaluated; anything else as the call's context
/// resolves it.
struct Redacted<'c, 'r>(&'c Ctx<'r>);

impl Resolver for Redacted<'_, '_> {
    fn eval(&mut self, expr: &dyn ResolveExpr) -> Result<OwnedDataValue> {
        if reads_secret(expr.raw()) {
            return Ok(OwnedDataValue::Null);
        }
        self.0.eval_owned(expr)
    }

    fn render(&mut self, doc: &Interpolated) -> Result<OwnedDataValue> {
        Ok(self.0.render(doc).to_owned())
    }
}

/// Sets the value of every expression that reads a secret to `null`, at the
/// path the config's walk reports it, in a resolved config.
struct Redact<'v>(&'v mut OwnedDataValue);

impl ExprVisitor for Redact<'_> {
    fn expr(&mut self, at: &AuthoredPath, raw: &Value, _records: bool) {
        if reads_secret(raw)
            && let Some(slot) = slot_at(self.0, at)
        {
            *slot = OwnedDataValue::Null;
        }
    }
}

/// The value at `at` in `value`, a key naming an object's entry and an
/// index an array's element; `None` when nothing is there.
fn slot_at<'v>(value: &'v mut OwnedDataValue, at: &AuthoredPath) -> Option<&'v mut OwnedDataValue> {
    at.segs()
        .iter()
        .try_fold(value, |value, seg| match (value, seg) {
            (OwnedDataValue::Object(entries), PathSeg::Key(key)) => entries
                .iter_mut()
                .find(|(k, _)| **k == **key)
                .map(|(_, v)| v),
            (OwnedDataValue::Array(items), PathSeg::Index(i)) => items.get_mut(*i),
            _ => None,
        })
}

impl fmt::Debug for Ctx<'_> {
    /// The call's facts and whether it has written; never the context.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field("info", &self.info)
            .field("overlay_empty", &self.overlay.is_empty())
            .finish_non_exhaustive()
    }
}

/// A `Ctx` over a context of the tests' choosing.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::function::{BoxFuture, Outcome};
    use crate::state::{RunContext, Tuning};

    /// A callee with no config: no primary output, and nothing to resolve.
    pub(crate) struct NoCallee;

    impl BoundCall for NoCallee {
        fn call<'a>(&'a self, _ctx: &'a mut Ctx<'_>) -> BoxFuture<'a, Result<Outcome>> {
            Box::pin(std::future::ready(Ok(Outcome::Ok)))
        }

        fn visit(&self, _v: &mut dyn ExprVisitor) {}

        fn primary(&self) -> Primary<'_> {
            Primary::Undeclared
        }

        fn resolve(&self, _r: &mut dyn Resolver) -> Result<OwnedDataValue> {
            Ok(OwnedDataValue::Null)
        }
    }

    /// Runs `f` with a call context over `context`, the call described by
    /// `info`, with `binding` if any.
    pub(crate) fn with_ctx<R>(
        context: &serde_json::Value,
        info: CallInfo<'_>,
        binding: Option<&Binding<'_>>,
        f: impl FnOnce(Ctx<'_>) -> R,
    ) -> R {
        let exprs = ExprEngine::default();
        let payload = OwnedDataValue::Null;
        let mut slot = OwnedDataValue::from(context);
        let mut rc = RunContext::new(&mut slot, Tuning::default());
        let reader = rc.reader();
        f(Ctx::new(
            CallStart {
                reader,
                binding,
                payload: &payload,
                exprs: &exprs,
                info,
                deadline: None,
                cancel: None,
                capture: true,
            },
            &NoCallee,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{NoCallee, with_ctx};
    use super::*;
    use crate::state::{RunContext, Tuning};
    use serde_json::{Value, json};

    fn send_sync<T: Send + Sync>() {}

    #[test]
    fn ctx_is_send_and_sync() {
        send_sync::<Ctx<'static>>();
    }

    fn info() -> CallInfo<'static> {
        CallInfo {
            program: ProgramId::next(),
            workflow: WorkflowIx::new(0),
            task: TaskIx::new(0),
            function: "f",
            workflow_id: "w",
            task_id: "t",
            channel: None,
            loop_counter: None,
            element_index: None,
        }
    }

    fn parse(dotted: &str) -> Path {
        dotted.parse().unwrap()
    }

    fn get(ctx: &Ctx<'_>, dotted: &str) -> Option<Value> {
        ctx.get(&parse(dotted)).map(|v| Value::from(&*v))
    }

    fn eval(ctx: &Ctx<'_>, raw: Value) -> Value {
        let expr: Expr = ExprEngine::default().compile(&raw).unwrap();
        Value::from(&*ctx.eval(&expr).unwrap())
    }

    #[test]
    fn the_binding_sits_under_the_calls_writes() {
        let item = parse("temp_data.item");
        let index = parse("temp_data.item_index");
        let value = OwnedDataValue::from(&json!({"sku": "A", "qty": 2}));
        let binding = Binding {
            item: &item,
            value: &value,
            index: &index,
            index_value: OwnedDataValue::from(3),
        };
        let context = json!({"data": {}, "metadata": {}, "temp_data": {"kept": true}});
        with_ctx(&context, info(), Some(&binding), |mut ctx| {
            assert_eq!(get(&ctx, "temp_data.item.sku"), Some(json!("A")));
            assert_eq!(get(&ctx, "temp_data.item_index"), Some(json!(3)));
            assert_eq!(
                get(&ctx, "temp_data"),
                Some(json!({"kept": true, "item": {"sku": "A", "qty": 2}, "item_index": 3}))
            );
            assert_eq!(eval(&ctx, json!({"var": "temp_data.item.qty"})), json!(2));
            // A write below a binding is seen over it, by `get` and `eval`.
            ctx.set(&parse("temp_data.item.qty"), OwnedDataValue::from(5))
                .unwrap();
            assert_eq!(
                get(&ctx, "temp_data.item"),
                Some(json!({"sku": "A", "qty": 5}))
            );
            assert_eq!(eval(&ctx, json!({"var": "temp_data.item.qty"})), json!(5));
            assert_eq!(get(&ctx, "temp_data.kept"), Some(json!(true)));
        });
    }

    #[test]
    fn a_write_that_cannot_land_records_its_change_and_writes_nothing() {
        // v3's `TaskContext::set` records before it writes. Removing nothing
        // records nothing.
        let context = json!({"data": {"s": "scalar"}, "metadata": {}, "temp_data": {}});
        let (overlay, changes, errors) = with_ctx(&context, info(), None, |mut ctx| {
            ctx.set(&parse("data.s.below"), OwnedDataValue::from(1))
                .unwrap();
            ctx.remove(&parse("data.absent")).unwrap();
            assert_eq!(get(&ctx, "data.s"), Some(json!("scalar")));
            ctx.settle()
        });
        assert!(errors.is_empty());
        // The write is kept for the settle, which replays it where the
        // context may take it, as v3 replays a `Change`; here it cannot.
        let mut slot = OwnedDataValue::from(&context);
        let mut rc = RunContext::new(&mut slot, Tuning::default());
        overlay.apply(&mut rc);
        rc.finish();
        assert_eq!(Value::from(&slot), context);
        assert_eq!(
            serde_json::to_value(&changes).unwrap(),
            json!([{"path": "data.s.below", "old_value": null, "new_value": 1}])
        );
    }

    #[test]
    fn a_removal_without_capture_removes_what_the_call_sees_and_records_nothing() {
        let context = json!({"data": {"a": 1, "list": [1, 2]}, "metadata": {}, "temp_data": {}});
        let exprs = ExprEngine::default();
        let payload = OwnedDataValue::Null;
        let mut slot = OwnedDataValue::from(&context);
        let mut rc = RunContext::new(&mut slot, Tuning::default());
        let mut ctx = Ctx::new(
            CallStart {
                reader: rc.reader(),
                binding: None,
                payload: &payload,
                exprs: &exprs,
                info: info(),
                deadline: None,
                cancel: None,
                capture: false,
            },
            &NoCallee,
        );
        // A value of the run's, one the call wrote, nothing, and an element
        // the call's own removal moved: after it, `list.1` holds nothing.
        for (dotted, write) in [
            ("data.a", None),
            ("data.b", Some(2)),
            ("data.b", None),
            ("data.absent", None),
            ("data.list.0", None),
            ("data.list.1", None),
        ] {
            match write {
                Some(n) => ctx.set(&parse(dotted), OwnedDataValue::from(n)),
                None => ctx.remove(&parse(dotted)),
            }
            .unwrap();
        }
        assert_eq!(get(&ctx, "data"), Some(json!({"list": [2]})));
        let (overlay, changes, errors) = ctx.settle();
        assert!(changes.is_empty() && errors.is_empty());
        overlay.apply(&mut rc);
        rc.finish();
        assert_eq!(Value::from(&slot)["data"], json!({"list": [2]}));
    }

    /// What each call recorded and wrote, when each of `writes` is made by a
    /// call of its own, as its first write or, with `after_another`, after
    /// a write elsewhere: the two ways `Ctx::set` reads the old value. The
    /// run has written `data.run` into its view, so its log holds it.
    fn first_writes(writes: &[&str], after_another: bool) -> (Vec<Value>, String) {
        let context = json!({"data": {"kept": {"k": 1}, "list": [1, null], "s": "scalar"},
            "metadata": {}, "temp_data": {}});
        let exprs = ExprEngine::default();
        let payload = OwnedDataValue::Null;
        let mut slot = OwnedDataValue::from(&context);
        let mut rc = RunContext::new(&mut slot, Tuning::default());
        rc.read(|_, _| ());
        rc.set(&parse("data.run"), OwnedDataValue::from(7), None);
        let mut olds = Vec::new();
        let mut overlays = Vec::new();
        for dotted in writes {
            let mut ctx = Ctx::new(
                CallStart {
                    reader: rc.reader(),
                    binding: None,
                    payload: &payload,
                    exprs: &exprs,
                    info: info(),
                    deadline: None,
                    cancel: None,
                    capture: true,
                },
                &NoCallee,
            );
            if after_another {
                ctx.set(&parse("temp_data.other"), OwnedDataValue::from(0))
                    .unwrap();
            }
            ctx.set(&parse(dotted), OwnedDataValue::from("w")).unwrap();
            let (overlay, changes, _) = ctx.settle();
            olds.push(serde_json::to_value(&changes.last().unwrap().old_value).unwrap());
            overlays.push(overlay);
        }
        for overlay in overlays {
            overlay.apply(&mut rc);
        }
        rc.finish();
        let data = parse("data").get(&slot).map(serde_json::to_string);
        (olds, data.unwrap().unwrap())
    }

    #[test]
    fn a_first_write_reads_what_was_there_as_a_later_write_does() {
        // In the run's log, in the owned tree, missing below a missing key, a
        // `null` element and below it, and below a scalar, where nothing lands.
        // Settled in order, the write below `list.1` finds the string the
        // write before it put there, and lands nowhere.
        let writes = [
            "data.run",
            "data.kept.k",
            "data.absent.x",
            "data.list.1",
            "data.list.1.x",
            "data.s.below",
        ];
        let (first, data) = first_writes(&writes, false);
        assert_eq!(
            first,
            vec![
                json!(7),
                json!(1),
                json!(null),
                json!(null),
                json!(null),
                json!(null)
            ]
        );
        assert_eq!(first_writes(&writes, true), (first, data.clone()));
        assert_eq!(
            data,
            r#"{"kept":{"k":"w"},"list":[1,"w"],"s":"scalar","run":"w","absent":{"x":"w"}}"#
        );
    }

    /// One call's writes: `(path, Some(value))`, or `(path, None)` for a
    /// removal.
    type Call<'a> = Vec<(&'a str, Option<Value>)>;

    /// The run context over `context` after each call's writes are made
    /// through a `Ctx` of its own over the run's one view, and the overlays
    /// applied in call order, as a fan-out's elements settle. Wire text.
    fn settled(context: &Value, calls: &[Call<'_>]) -> String {
        let exprs = ExprEngine::default();
        let payload = OwnedDataValue::Null;
        let mut slot = OwnedDataValue::from(context);
        let mut rc = RunContext::new(&mut slot, Tuning::default());
        let mut overlays = Vec::new();
        for writes in calls {
            let mut ctx = Ctx::new(
                CallStart {
                    reader: rc.reader(),
                    binding: None,
                    payload: &payload,
                    exprs: &exprs,
                    info: info(),
                    deadline: None,
                    cancel: None,
                    capture: true,
                },
                &NoCallee,
            );
            for (dotted, value) in writes {
                match value {
                    Some(value) => ctx.set(&parse(dotted), OwnedDataValue::from(value)),
                    None => ctx.remove(&parse(dotted)),
                }
                .unwrap();
            }
            overlays.push(ctx.settle().0);
        }
        for overlay in overlays {
            overlay.apply(&mut rc);
        }
        rc.finish();
        serde_json::to_string(&slot).unwrap()
    }

    /// Calls planned over one base, then settled in order, end as one call
    /// making all their writes in that order would: each write lands at its
    /// own path on the writes before it. Two keys under a parent neither
    /// call's base has, then the three cases a write of a whole container
    /// would lose: padding, an element's removal, a key written after its
    /// removal.
    #[test]
    fn calls_settled_in_order_end_as_one_call_making_their_writes() {
        // The data root before, each call's writes, and the data root after
        // as wire text: key order is part of what is checked.
        let cases: [(Value, Vec<Call<'_>>, &str); 4] = [
            (
                json!({}),
                vec![
                    vec![("data.prices.a", Some(json!(1)))],
                    vec![("data.prices.b", Some(json!(2)))],
                ],
                r#"{"prices":{"a":1,"b":2}}"#,
            ),
            (
                json!({"list": [1, 2]}),
                vec![
                    vec![("data.list.4", Some(json!(5)))],
                    vec![("data.list.3", Some(json!(4)))],
                ],
                r#"{"list":[1,2,null,4,5]}"#,
            ),
            (
                json!({"list": [1, 2, 3]}),
                vec![vec![("data.list.0", None)], vec![("data.list.2", None)]],
                r#"{"list":[2,3]}"#,
            ),
            (
                json!({"a": 1, "b": 2}),
                vec![
                    vec![("data.b", Some(json!(5)))],
                    vec![("data.a", None), ("data.a", Some(json!(3)))],
                ],
                r#"{"b":5,"a":3}"#,
            ),
        ];
        for (data, calls, want) in cases {
            let context = json!({"data": data, "metadata": {}, "temp_data": {}});
            let want = format!(r#"{{"data":{want},"metadata":{{}},"temp_data":{{}}}}"#);
            let one_call = vec![calls.concat()];
            assert_eq!(settled(&context, &one_call), want, "one call: {calls:?}");
            assert_eq!(
                settled(&context, &calls),
                want,
                "settled in order: {calls:?}"
            );
        }
    }

    /// A fan-out's calls, each over the run's one view with its own
    /// element bound, each writing under its binding and under `data`,
    /// then settled in element order onto a run where no binding is: each
    /// call reads its writes over its element, and each write lands at its
    /// own path on the writes settled before it, as v3 replays an element's
    /// `Change`s onto a message that holds no binding. A write below a
    /// scalar element is blocked in its call and lands at the settle.
    #[test]
    fn calls_with_bindings_settled_in_order_replay_each_write_at_its_path() {
        let context = json!({"data": {}, "metadata": {}, "temp_data": {}});
        let item = parse("temp_data.p");
        let index = parse("temp_data.p_index");
        // Each element, its writes, what the call then sees at
        // `temp_data.p`, and the old values its `Change`s record.
        let elements: [(Value, Call<'_>, Value, [Value; 2]); 3] = [
            (
                json!({"id": "a"}),
                vec![
                    ("temp_data.p.x", Some(json!(1))),
                    ("data.a", Some(json!(1))),
                ],
                json!({"id": "a", "x": 1}),
                [json!(null), json!(null)],
            ),
            (
                json!(5),
                vec![
                    ("temp_data.p.y", Some(json!(2))),
                    ("data.b", Some(json!(2))),
                ],
                json!(5),
                [json!(null), json!(null)],
            ),
            (
                json!({"id": "c"}),
                vec![
                    ("temp_data.p.id", Some(json!("C"))),
                    ("data.a", Some(json!(3))),
                ],
                json!({"id": "C"}),
                [json!("c"), json!(null)],
            ),
        ];
        let exprs = ExprEngine::default();
        let payload = OwnedDataValue::Null;
        let mut slot = OwnedDataValue::from(&context);
        let mut rc = RunContext::new(&mut slot, Tuning::default());
        let mut overlays = Vec::new();
        let reader = rc.reader();
        for (i, (element, writes, seen, olds)) in elements.iter().enumerate() {
            let value = OwnedDataValue::from(element);
            let binding = Binding {
                item: &item,
                value: &value,
                index: &index,
                index_value: OwnedDataValue::from(i),
            };
            let mut ctx = Ctx::new(
                CallStart {
                    reader,
                    binding: Some(&binding),
                    payload: &payload,
                    exprs: &exprs,
                    info: info(),
                    deadline: None,
                    cancel: None,
                    capture: true,
                },
                &NoCallee,
            );
            for (dotted, value) in writes {
                let value = value.as_ref().map(OwnedDataValue::from).unwrap();
                ctx.set(&parse(dotted), value).unwrap();
            }
            assert_eq!(get(&ctx, "temp_data.p").as_ref(), Some(seen), "element {i}");
            assert_eq!(
                eval(&ctx, json!({"var": "temp_data.p"})),
                *seen,
                "element {i}"
            );
            assert_eq!(get(&ctx, "temp_data.p_index"), Some(json!(i)));
            let (overlay, changes, _) = ctx.settle();
            let recorded: Vec<Value> = changes.iter().map(|c| Value::from(&c.old_value)).collect();
            assert_eq!(recorded, olds, "element {i}");
            overlays.push(overlay);
        }
        for overlay in overlays {
            overlay.apply(&mut rc);
        }
        rc.finish();
        assert_eq!(
            serde_json::to_string(&slot).unwrap(),
            r#"{"data":{"a":3,"b":2},"metadata":{},"temp_data":{"p":{"x":1,"y":2,"id":"C"}}}"#
        );
    }

    #[test]
    fn render_lends_each_hole_from_where_it_is_and_builds_no_tree() {
        // R54: only a value the call wrote below a hole is converted.
        let context = json!({"data": {"order": {"customer": "C", "qty": 3}},
            "metadata": {}, "temp_data": {}});
        let exprs = ExprEngine::default();
        let payload = OwnedDataValue::Null;
        let mut slot = OwnedDataValue::from(&context);
        let mut rc = RunContext::new(&mut slot, Tuning::default());
        rc.read(|_, _| ());
        // The run writes one value with its owned copy, as `map` does under
        // capture, and one into the view only, as the progress write does.
        rc.set(
            &parse("data.order.since"),
            OwnedDataValue::from("2026"),
            None,
        );
        let progress = parse("metadata.progress");
        rc.edit(|e| {
            e.set(
                &progress,
                OwnedDataValue::from("p").to_arena(e.arena()),
                None,
            )
        });
        let mut ctx = Ctx::new(
            CallStart {
                reader: rc.reader(),
                binding: None,
                payload: &payload,
                exprs: &exprs,
                info: info(),
                deadline: None,
                cancel: None,
                capture: true,
            },
            &NoCallee,
        );
        ctx.set(&parse("temp_data.mine"), OwnedDataValue::from(1))
            .unwrap();
        ctx.set(&parse("temp_data.box.k"), OwnedDataValue::from(2))
            .unwrap();
        let doc = exprs
            .interpolated(&json!([
                {"var": "temp_data.mine"},
                {"var": "temp_data.box"},
                {"var": "data.order.since"},
                {"var": "metadata.progress"},
                {"var": "data.order"},
                {"var": "data.order.customer"},
                {"var": ["data.absent", "d"]},
                {"var": ["payload.x", "p"]}
            ]))
            .unwrap();
        let rendered = ctx.render(&doc);
        let kinds: Vec<&str> = rendered
            .filled()
            .iter()
            .map(|filled| match filled {
                Filled::Owned(Cow::Borrowed(_)) => "lent",
                Filled::Owned(Cow::Owned(_)) => "converted",
                Filled::View(_) => "view",
                Filled::Default(_) => "default",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "lent",      // the call's own write
                "converted", // written below by the call
                "lent",      // the run's log, its owned copy
                "view",      // the run's log, the view alone
                "view",      // above a write in the run's log
                "lent",      // the owned tree
                "default",   // nothing there
                "default"    // the payload, never in the context
            ]
        );
        assert_eq!(
            serde_json::to_value(&rendered).unwrap(),
            json!([1, {"k": 2}, "2026", "p",
                {"customer": "C", "qty": 3, "since": "2026"}, "C", "d", "p"])
        );
    }

    /// A callee whose resolve puts a value where its walk reports a
    /// secret-reading expression, as a field resolving its secret some other
    /// way than through the resolver would.
    struct Leaky;

    impl BoundCall for Leaky {
        fn call<'a>(
            &'a self,
            _ctx: &'a mut Ctx<'_>,
        ) -> crate::function::BoxFuture<'a, Result<crate::function::Outcome>> {
            Box::pin(std::future::ready(Ok(crate::function::Outcome::Ok)))
        }

        fn visit(&self, v: &mut dyn ExprVisitor) {
            let at = |text: &str| text.parse::<AuthoredPath>().unwrap();
            v.expr(&at("a.b[1]"), &json!({"secret": "k"}), false);
            v.expr(
                &at("a.c"),
                &json!({"cat": ["x", {"secret": {"var": "data.k"}}]}),
                false,
            );
            v.expr(&at("kept"), &json!({"var": "data.k"}), false);
            // Nothing is there: nothing is written.
            v.expr(&at("absent.x"), &json!({"secret": "k"}), false);
        }

        fn primary(&self) -> Primary<'_> {
            Primary::Undeclared
        }

        fn resolve(&self, _r: &mut dyn Resolver) -> Result<OwnedDataValue> {
            Ok(OwnedDataValue::from(&json!({
                "a": {"b": [1, "leaked"], "c": "leaked too"},
                "kept": "v",
            })))
        }
    }

    #[test]
    fn input_nulls_every_value_the_walk_reports_reading_a_secret() {
        let context = json!({"data": {"k": "v"}, "metadata": {}, "temp_data": {}});
        let exprs = ExprEngine::default();
        let payload = OwnedDataValue::Null;
        let mut slot = OwnedDataValue::from(&context);
        let mut rc = RunContext::new(&mut slot, Tuning::default());
        let ctx = Ctx::new(
            CallStart {
                reader: rc.reader(),
                binding: None,
                payload: &payload,
                exprs: &exprs,
                info: info(),
                deadline: None,
                cancel: None,
                capture: true,
            },
            &Leaky,
        );
        assert_eq!(
            ctx.input().unwrap().to_json_string(),
            r#"{"a":{"b":[1,null],"c":null},"kept":"v"}"#
        );
    }

    #[test]
    fn debug_shows_the_call_and_never_the_context() {
        let context = json!({"data": {"secret_looking": "value"}});
        with_ctx(&context, info(), None, |ctx| {
            let text = format!("{ctx:?}");
            assert!(text.contains("task_id: \"t\"") && text.contains("overlay_empty: true"));
            assert!(!text.contains("secret_looking"), "{text}");
            assert!(text.ends_with(".. }"), "{text}");
        });
    }
}
