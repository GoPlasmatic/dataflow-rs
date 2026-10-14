//! `Ctx`: a function's view of one call, and the static facts about it.

use super::overlay::Overlay;
use crate::base::{CancelToken, Change, ErrorRecord, ProgramId, Result, TaskIx, WorkflowIx};
use crate::expr::arena::with_arena;
use crate::expr::path;
use crate::expr::{Expr, ExprEngine, FromValue, Output, Path, Seg};
use crate::state::{Lookup, Reader, splice};
use bumpalo::Bump;
use datavalue::{DataValue, OwnedDataValue};
use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

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
/// [`CallLayer`](super::CallLayer).
pub struct Ctx<'r> {
    /// The run's view, the owned tree and the run's log.
    reader: Reader<'r>,
    /// A fan-out element's bindings, under the call's own writes.
    binding: Option<&'r Binding<'r>>,
    payload: &'r OwnedDataValue,
    exprs: &'r ExprEngine,
    info: CallInfo<'r>,
    /// The callee's primary output.
    #[expect(dead_code, reason = "Task 3's `emit_default` writes through it")]
    primary: Option<&'r Output>,
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

/// What the runtime hands a new [`Ctx`].
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
    /// The callee's primary output.
    pub(crate) primary: Option<&'r Output>,
    /// When the run must end.
    pub(crate) deadline: Option<Instant>,
    /// The run's cancellation token.
    pub(crate) cancel: Option<&'r CancelToken>,
    /// Whether the call records its `Change`s.
    pub(crate) capture: bool,
}

/// A fan-out element's bindings: `temp_data.<as>` holds the element and
/// `temp_data.<as>_index` its index. They sit between the run's context and
/// the call's own writes. The runtime builds none until Task 4's fan-out.
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
    /// A context for one call.
    pub(crate) fn new(start: CallStart<'r>) -> Self {
        Self {
            reader: start.reader,
            binding: start.binding,
            payload: start.payload,
            exprs: start.exprs,
            info: start.info,
            primary: start.primary,
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

    /// When the run must end, if it has a deadline.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// The run's cancellation token, if it has one: a child of it cancels a
    /// run this function starts along with this one (R60).
    pub fn cancel(&self) -> Option<&CancelToken> {
        self.cancel
    }

    /// The value at `path` as this call sees it: its own writes, over the
    /// run's context. Borrowed unless only the run's view or this call's
    /// writes below `path` know it, when it is converted.
    pub fn get(&self, path: &Path) -> Option<Cow<'_, OwnedDataValue>> {
        if !self.overlay.is_empty() {
            let written = with_base(self.reader, self.binding, |base| {
                match self.overlay.lookup(base, path) {
                    Lookup::Clean => None,
                    Lookup::Owned(v) => Some(v.map(Cow::Borrowed)),
                    // Written below: read off the root `eval` sees.
                    Lookup::Dirty => Some(with_arena(|arena| {
                        splice::get(*self.root_in(arena), path.segs())
                            .map(|v| Cow::Owned(v.to_owned()))
                    })),
                }
            });
            if let Some(found) = written {
                return found;
            }
        }
        if let Some(found) = self.binding.and_then(|b| b.get(path, *self.reader.root)) {
            return found;
        }
        self.reader.get(path)
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
    /// one path leaves two `Change`s and one value.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`](crate::base::ErrorKind::Config) when `path` is a
    /// root and `value` is not an object (R52).
    pub fn set(&mut self, path: &Path, value: OwnedDataValue) -> Result<()> {
        path.check_set(&value)?;
        if self.capture {
            let old_value = self.get(path).map_or(OwnedDataValue::Null, Cow::into_owned);
            self.changes.push(Change::write(
                Arc::clone(path.dotted()),
                old_value,
                value.clone(),
            ));
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

    /// Removes the value at `path`, recording the removal under capture.
    /// Nothing there is nothing to do, and records nothing.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`](crate::base::ErrorKind::Config) when `path` is a
    /// root (R52).
    pub fn remove(&mut self, path: &Path) -> Result<()> {
        path.check_remove()?;
        let old_value = match self.get(path) {
            None => return Ok(()),
            Some(old) => self.capture.then(|| old.into_owned()),
        };
        if let Some(old_value) = old_value {
            self.changes
                .push(Change::removal(Arc::clone(path.dotted()), old_value));
        }
        let Self {
            reader,
            binding,
            overlay,
            ..
        } = self;
        with_base(*reader, *binding, |base| overlay.remove(base, path));
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
    use crate::state::{RunContext, Tuning};

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
        f(Ctx::new(CallStart {
            reader,
            binding,
            payload: &payload,
            exprs: &exprs,
            info,
            primary: None,
            deadline: None,
            cancel: None,
            capture: true,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::testing::with_ctx;
    use super::*;
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
        assert!(overlay.is_empty() && errors.is_empty());
        assert_eq!(
            serde_json::to_value(&changes).unwrap(),
            json!([{"path": "data.s.below", "old_value": null, "new_value": 1}])
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
