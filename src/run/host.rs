//! One host call (05, "A host `Function`"): a [`Ctx`] over the run's view,
//! the call through the layers, and the settle.
//!
//! The reader is taken in its own statement and the `Ctx` holds only it, so
//! no `&RunView` is held across the call's `.await`. Once the call returns,
//! an `Ok`, whatever its `Outcome`, applies the overlay through the run
//! context in order and keeps the call's `Change`s; an `Err` drops both
//! (break 1). The records the call added are kept either way. Then the
//! recorder runs as for a built-in.
//!
//! The run's limits are checked before each call and, natively, raced
//! against it (I92). A call they cut short is dropped and settles as an
//! `Err` of kind `TimedOut` or `Cancelled` does, its writes dropped and its
//! records kept; then the run stops, whatever the task's policies. A call
//! that returns `Err` once one of them has passed is decided as one they
//! cut short: a child run handed the run's limits stops in the wake they
//! fire in, and its call returns before the race can cut it short, or on
//! wasm, where nothing races.
//!
//! A fan-out's elements are host calls too: [`invoke`] is the one call, and
//! [`fan_out`](super::fan_out::fan_out) settles and records each element.

use super::cancel::{Limits, race};
use super::interp::RunEnv;
use super::policy::{self, Flow};
use super::record::{self, Completed, Stamp, TaskIds};
use crate::base::{Change, ErrorRecord, Result, TaskIx};
use crate::expr::Path;
use crate::function::{BoundCall, CallInfo, CallLayer, CallStart, Ctx, Next, Outcome, Overlay};
use crate::program::{CallSite, CompiledWorkflow};
use crate::state::{Reader, RunContext};
use datavalue::OwnedDataValue;
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

/// The layers around a call: the run's, outermost, then the registry's.
#[derive(Clone, Copy)]
pub(crate) struct Layers<'a> {
    /// The run's layers, outermost first.
    pub(crate) run: &'a [Arc<dyn CallLayer>],
    /// The registry's layers, outermost first.
    pub(crate) registry: &'a [Arc<dyn CallLayer>],
}

/// What a call at `site` starts from, over `reader`: its [`CallStart`],
/// with no binding and no element, and its [`Layers`]. A fan-out copies
/// both for each element. Nothing returned borrows `env`.
pub(crate) fn call_start<'c, 'r: 'c, 'p: 'c>(
    env: &RunEnv<'r, 'p>,
    reader: Reader<'c>,
    wf: &'c CompiledWorkflow,
    site: &CallSite,
) -> (CallStart<'c>, Layers<'c>) {
    let task = &wf.tasks[site.task as usize];
    let start = CallStart {
        reader,
        binding: None,
        payload: env.payload,
        exprs: env.exprs,
        info: CallInfo {
            program: env.program,
            workflow: env.workflow_ix,
            task: TaskIx::new(env.task_base + site.task),
            function: &task.function,
            workflow_id: &wf.id,
            task_id: &task.id,
            channel: env.channel,
            loop_counter: env.frame.loop_counter,
            element_index: None,
        },
        deadline: env.limits.deadline,
        cancel: env.limits.cancel,
        capture: env.capture,
    };
    let layers = Layers {
        run: env.layers,
        registry: wf.registry.layers(),
    };
    (start, layers)
}

/// What one call left: its result, and what it wrote and recorded.
pub(crate) struct Invoked {
    /// What the chain returned.
    pub(crate) result: Result<Outcome>,
    /// The call's writes, in order.
    pub(crate) overlay: Overlay,
    /// The call's `Change`s, in write order.
    pub(crate) changes: Vec<Change>,
    /// The records the call added.
    pub(crate) errors: Vec<ErrorRecord>,
    /// What the call saw at `collect` when it returned `Ok`: a fan-out
    /// element's result (I91).
    pub(crate) collected: Option<OwnedDataValue>,
    /// Whether the run's deadline or token cut the call short, or had
    /// passed when the call returned `Err`: `result` is then the `Err` that
    /// stops the run.
    pub(crate) cut_short: bool,
    /// How long a fan-out element's call ran, in a timed run: set by its
    /// caller, which reads the clock around the call.
    pub(crate) elapsed: Option<Duration>,
}

/// One call of `bound` through the layers: the `Ctx` built from `start`,
/// the run's limits checked, the chain run and, natively, raced against
/// them, the value at `collect` read as the call sees it once it returned
/// `Ok`, the `Ctx` settled. A call the limits cut short, before it starts or while it
/// runs, gives `Err` of kind `TimedOut` or `Cancelled`, naming the function;
/// so does a call that returns `Err` once a limit has passed, whatever its
/// own error, which costs one check of the limits on an `Err` and none on
/// an `Ok`.
///
/// The future holds the `Ctx`, which holds the callee, and not `start` or
/// `bound` beside it, and the race is boxed, made only when a limit is set:
/// the run's future, which a host spawns once per message, carries neither
/// the timer's state nor a second copy of the call's start.
pub(crate) fn invoke<'r>(
    start: CallStart<'r>,
    bound: &'r dyn BoundCall,
    layers: Layers<'r>,
    collect: Option<&Path>,
) -> impl Future<Output = Invoked> {
    let limits = Limits {
        deadline: start.deadline,
        cancel: start.cancel,
    };
    let function = start.info.function;
    let mut ctx = Ctx::new(start, bound);
    async move {
        let chain = Next::new(layers.run, layers.registry, ctx.callee());
        let ran = match limits.stop() {
            Some(stop) => Err(stop),
            None if !limits.is_set() => Ok(chain.run(&mut ctx).await),
            None => Box::pin(race(chain.run(&mut ctx), limits)).await,
        };
        let (result, cut_short) = match ran {
            // An `Err` once the run's own limit has passed is the limit's:
            // the call returned in the wake the limit fired in, before the
            // race could cut it short, or on wasm, where nothing races. A
            // child run handed the run's limits ends that way. Only an
            // `Err` pays for the check.
            Ok(Err(e)) => match limits.stop() {
                Some(stop) => (Err(stop.during(function)), true),
                None => (Err(e), false),
            },
            Ok(result) => (result, false),
            Err(stop) => (Err(stop.during(function)), true),
        };
        let collected = match collect {
            Some(path) if result.is_ok() => ctx.get(path).map(Cow::into_owned),
            _ => None,
        };
        let (overlay, changes, errors) = ctx.settle();
        Invoked {
            result,
            overlay,
            changes,
            errors,
            collected,
            cut_short,
            elapsed: None,
        }
    }
}

/// A record a call added, as given (I96): with the run's timestamp,
/// `now_text`, when it carries none, and in a fan-out the element's index
/// when it carries none.
pub(crate) fn stamp_record(
    mut record: ErrorRecord,
    now_text: &str,
    element_index: Option<usize>,
) -> ErrorRecord {
    if record.timestamp.is_none() {
        record.timestamp = Some(now_text.to_owned());
    }
    if record.element_index.is_none() {
        record.element_index = element_index;
    }
    record
}

/// The records a call added, onto the message, each stamped by
/// [`stamp_record`].
pub(crate) fn keep_records(
    env: &mut RunEnv<'_, '_>,
    records: Vec<ErrorRecord>,
    element_index: Option<usize>,
) {
    for record in records {
        let record = stamp_record(record, env.now_text, element_index);
        env.errors.push(record);
    }
}

/// One `Call` op on a host callee: the call, timed in a timed run, the
/// settle, the decision, the recorder; the flow.
///
/// The records the call added go to the message first, as given, with the
/// run's timestamp when they carry none (I96); the recorder's own follows.
/// A call the run's limits cut short is recorded as an `Err`, and then
/// stops the run.
pub(crate) async fn call_host<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    wf: &'p CompiledWorkflow,
    site: &'p CallSite,
    bound: &'p dyn BoundCall,
) -> Flow {
    let started = env.watch.timed.then(record::now);
    let Invoked {
        result,
        overlay,
        changes,
        errors,
        cut_short,
        ..
    } = {
        let reader = rc.reader();
        let (start, layers) = call_start(env, reader, wf, site);
        invoke(start, bound, layers, None).await
    };
    let elapsed = started.map(record::since);
    // Read after the call, so the run's future keeps neither across its
    // `.await`: nothing pushes to the message's records while it runs.
    let ids = TaskIds {
        workflow: env.workflow_id,
        task: &env.task_ids[site.task as usize],
    };
    let errors_before = env.errors.len();
    if result.is_ok() {
        overlay.apply(rc);
        env.changes = changes;
    }
    let marked = replaces_a_root(&env.changes);
    keep_records(env, errors, None);
    let decision = match result {
        Err(e) if cut_short => policy::cut_short(ids.task, e),
        result => policy::decide(site.policy(), ids.task, result),
    };
    let done = Completed {
        ids,
        task: site.task,
        stamp: Stamp::sweep(env),
        status: decision.status,
        record: decision.record,
        failed: matches!(decision.flow, Flow::Fail(_) | Flow::Stop(_)),
        elapsed,
        errors_before,
        marked,
    };
    record::record(rc, env, wf, done);
    decision.flow
}

/// Whether a host call's `changes` replace a root (I106): the recorder then
/// clears the flag on the audit's copy, which keeps v3's bytes.
pub(crate) fn replaces_a_root(changes: &[Change]) -> bool {
    changes.iter().any(|c| c.replaced)
}
