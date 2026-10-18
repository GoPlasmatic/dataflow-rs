//! One host call: a [`Ctx`] over the run's view,
//! the call through the layers, and the settle.
//!
//! The reader is taken in its own statement and the `Ctx` holds only it, so
//! no `&RunView` is held across the call's `.await`. Once the call returns,
//! an `Ok`, whatever its `Outcome`, applies the overlay through the run
//! context in order and keeps the call's `Change`s; an `Err` drops both,
//! and the records the call added are kept either way. Then the
//! recorder runs as for a built-in.
//!
//! The run's limits are checked before each call and, natively, raced
//! against it. A call they cut short is dropped and settles as an
//! `Err` of kind `TimedOut` or `Cancelled` does, its writes dropped and its
//! records kept; then the run stops, whatever the task's policies. A call
//! that returns `Err` once one of them has passed is decided as one they
//! cut short: a child run handed the run's limits stops in the wake they
//! fire in, and its call returns before the race can cut it short, or on
//! wasm, where nothing races.
//!
//! A fan-out's elements are host calls too: [`invoke`] is the one call, and
//! [`fan_out`](super::fan_out::fan_out) settles and records each element.

use super::cancel::Limits;
#[cfg(not(target_arch = "wasm32"))]
use super::cancel::race;
use super::interp::RunEnv;
use super::policy::{self, Flow};
use super::record::{self, Completed, Stamp};
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
    let start = CallStart {
        reader,
        binding: None,
        payload: env.payload,
        exprs: env.exprs,
        info: env.call_info(wf, site.task, Stamp::sweep(env)),
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

impl<'r> RunEnv<'r, '_> {
    /// The [`CallInfo`] of `wf`'s task at row `task`, at `stamp`: what the
    /// call is handed, and what its `TaskFinished` tells.
    pub(crate) fn call_info<'c>(
        &self,
        wf: &'c CompiledWorkflow,
        task: u32,
        stamp: Stamp,
    ) -> CallInfo<'c>
    where
        'r: 'c,
    {
        let meta = &wf.tasks[task as usize];
        CallInfo {
            program: self.program,
            workflow: self.workflow_ix,
            task: TaskIx::new(self.task_base + task),
            function: &meta.function,
            workflow_id: &wf.id,
            task_id: &meta.id,
            channel: self.channel,
            routing_bucket: self.routing_bucket,
            loop_counter: stamp.loop_counter,
            element_index: stamp.element_index,
        }
    }
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
    /// element's result.
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
    timers: bool,
) -> impl Future<Output = Invoked> {
    let limits = Limits {
        deadline: start.deadline,
        cancel: start.cancel,
        timers,
    };
    let function = start.info.function;
    let mut ctx = Ctx::new(start, bound);
    async move {
        let chain = Next::new(layers.run, layers.registry, ctx.callee());
        // Natively a limit is raced on a timer, boxed so the run's future
        // carries neither the timer's state nor a second copy of the call's
        // start. On wasm nothing races, so the call runs to its end and the
        // checks around it are the whole limit: no box, no race.
        let ran = match limits.stop() {
            Some(stop) => Err(stop),
            #[cfg(not(target_arch = "wasm32"))]
            None if limits.is_set() => Box::pin(race(chain.run(&mut ctx), limits)).await,
            None => Ok(chain.run(&mut ctx).await),
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

/// The records a call added, onto the message, each stamped
/// ([`ErrorRecord::stamp`]) with the task's ids, the run's timestamp and in
/// a fan-out the element's index, where it carries none of its own.
pub(crate) fn keep_records(
    env: &mut RunEnv<'_, '_>,
    task: u32,
    records: Vec<ErrorRecord>,
    element_index: Option<usize>,
) {
    let ids = env.ids(task);
    for mut record in records {
        record.stamp(ids.workflow, Some(ids.task), env.now_text, element_index);
        env.errors.push(record);
    }
}

/// One `Call` op on a host callee: the call, timed in a timed run, the
/// settle, the decision, the recorder; the flow.
///
/// The records the call added go to the message first, stamped with the
/// task's ids and the run's timestamp where they carry none; the
/// recorder's own follows.
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
        invoke(start, bound, layers, None, env.limits.timers).await
    };
    let elapsed = started.map(record::since);
    // Read after the call, so the run's future keeps neither across its
    // `.await`: nothing pushes to the message's records while it runs.
    let task_id = env.ids(site.task).task;
    let errors_before = env.errors.len();
    if result.is_ok() {
        overlay.apply(rc);
        env.changes = changes;
    }
    let marked = replaces_a_root(&env.changes);
    keep_records(env, site.task, errors, None);
    let decision = match result {
        Err(e) if cut_short => policy::cut_short(task_id, e),
        result => policy::decide(site.policy, task_id, result),
    };
    let done = Completed {
        task: site.task,
        stamp: Stamp::sweep(env),
        elapsed,
        errors_before,
        marked,
    };
    record::record(rc, env, wf, done, decision)
}

/// Whether a host call's `changes` replace a root: the recorder then
/// clears the flag on the audit's copy, which keeps v3's bytes.
pub(crate) fn replaces_a_root(changes: &[Change]) -> bool {
    changes.iter().any(|c| c.replaced)
}
