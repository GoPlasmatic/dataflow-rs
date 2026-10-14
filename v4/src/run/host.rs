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
//! A fan-out's elements are host calls too: [`invoke`] is the one call, and
//! [`fan_out`](super::fan_out::fan_out) settles and records each element.

use super::interp::RunEnv;
use super::policy::{self, Flow};
use super::record::{self, Stamp, TaskIds};
use crate::base::{Change, ErrorRecord, Result, TaskIx};
use crate::expr::Path;
use crate::function::{BoundCall, CallInfo, CallLayer, CallStart, Ctx, Next, Outcome, Overlay};
use crate::program::{CallSite, CompiledWorkflow};
use crate::state::RunContext;
use datavalue::OwnedDataValue;
use std::borrow::Cow;
use std::sync::Arc;

/// The layers around a call: the run's, outermost, then the registry's.
#[derive(Clone, Copy)]
pub(crate) struct Layers<'a> {
    /// The run's layers, outermost first.
    pub(crate) run: &'a [Arc<dyn CallLayer>],
    /// The registry's layers, outermost first.
    pub(crate) registry: &'a [Arc<dyn CallLayer>],
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
}

/// One call through the layers: the `Ctx` built from `start`, the chain
/// run, the value at `collect` read as the call sees it once it returned
/// `Ok`, the `Ctx` settled.
pub(crate) async fn invoke<'r>(
    start: CallStart<'r>,
    bound: &'r dyn BoundCall,
    layers: Layers<'r>,
    collect: Option<&Path>,
) -> Invoked {
    let mut ctx = Ctx::new(start);
    let result = Next::new(layers.run, layers.registry, bound)
        .run(&mut ctx)
        .await;
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
    }
}

/// The records a call added, onto the message, as given (I96): with the
/// run's timestamp when they carry none, and in a fan-out the element's
/// index when they carry none.
pub(crate) fn keep_records(
    env: &mut RunEnv<'_, '_>,
    records: Vec<ErrorRecord>,
    element_index: Option<usize>,
) {
    for mut record in records {
        if record.timestamp.is_none() {
            record.timestamp = Some(env.now_text.to_owned());
        }
        if record.element_index.is_none() {
            record.element_index = element_index;
        }
        env.errors.push(record);
    }
}

/// One `Call` op on a host callee: the call, the settle, the decision, the
/// recorder; the flow.
///
/// The records the call added go to the message first, as given, with the
/// run's timestamp when they carry none (I96); the recorder's own follows.
pub(crate) async fn call_host<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    wf: &'p CompiledWorkflow,
    site: &'p CallSite,
    bound: &'p dyn BoundCall,
) -> Flow {
    let task = &wf.tasks[site.task as usize];
    let ids = TaskIds {
        workflow: env.workflow_id,
        task: &env.task_ids[site.task as usize],
    };
    let Invoked {
        result,
        overlay,
        changes,
        errors,
        ..
    } = {
        let reader = rc.reader();
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
            primary: bound.primary_output(),
            deadline: None,
            cancel: None,
            capture: env.capture,
        };
        let layers = Layers {
            run: env.layers,
            registry: wf.registry.layers(),
        };
        invoke(start, bound, layers, None).await
    };
    if result.is_ok() {
        overlay.apply(rc);
        env.changes = changes;
    }
    keep_records(env, errors, None);
    let decision = policy::decide(site.policy(), ids.task, result);
    let stamp = Stamp::sweep(env);
    record::record(rc, env, ids, stamp, decision.status, decision.record);
    decision.flow
}
