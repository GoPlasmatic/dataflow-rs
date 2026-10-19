//! One fan-out: a task's host function called once per
//! element of an array, then folded into the run in element order.
//!
//! `over` is evaluated once, to an owned array, by a loop's rule. Each
//! element's call is a host call ([`invoke`]) over the run's one view, with
//! the element and its index bound beneath the call's own writes
//! ([`Binding`]). Nothing reaches the run until every call has finished, so
//! no call sees another's writes, however many run at once. The calls run
//! under `FuturesUnordered`, at most `max_concurrency` at once and with no
//! spawn, so they borrow the view; no new call starts once a finished one
//! halts or fails the task, and the calls in flight finish.
//!
//! The fold then goes in element order over the elements that finished, as
//! v3's `run_for_each` folds: the records the call added, its writes (an
//! `Ok`'s, whatever its outcome), its result in `into`, then the recorder,
//! stamped with the element. It stops after the first element that fails
//! the task or halts, so the elements after it contribute nothing, finished
//! or not: what makes `max_concurrency` a timing knob only. Each element is
//! decided under [`Policy::per_element`], which cannot halt; the task's own
//! `terminal`, `on_rejection: halt` and `on_failure: halt` apply once, after
//! the fold.
//!
//! Each element's call checks the run's limits as it starts and, natively,
//! races them; one that returns `Err` once a limit has passed is cut short
//! too, as a single call is. Once one is cut short, the calls in flight are
//! dropped and nothing is folded, the finished elements included. The
//! records each call that returned added are kept, in element order and
//! stamped with their element, as a single call cut short keeps its own;
//! then the task is recorded once, with no element, as a call cut short is,
//! and the run stops.
//!
//! In a timed run each element's call is timed as it runs. The task's one
//! record of a fan-out cut short takes the time the call cut short ran, and
//! of an `over` with nothing to call the time from its evaluation to its
//! record, as v3's.

use super::host::{CallResult, Invoked, Layers, Settled, call_start, invoke, keep_records, settle};
use super::interp::{Over, RunEnv, elements};
use super::policy::{self, Decision, Flow, Policy, decide};
use super::record::{self, Completed};
use crate::base::{Change, Error, ErrorKind};
use crate::expr::{MAX_ARRAY_INDEX, Path, Seg};
use crate::function::{Binding, BoundCall, CallInfo, CallStart, Outcome};
use crate::program::{CallSite, CompiledWorkflow, FanSite};
use crate::state::RunContext;
use datavalue::OwnedDataValue;
use futures_util::stream::{FuturesUnordered, StreamExt};
use std::sync::Arc;

/// What one element's call left once its `Ctx` was settled: all the fold
/// needs, owned, so no element holds the view.
struct Element {
    index: usize,
    invoked: Invoked,
    /// The call's decision under the per-element policy.
    decision: Decision,
}

impl Element {
    /// Element `index`'s call, once it returned without being cut short,
    /// decided under `policy`.
    fn new(index: usize, mut invoked: Invoked, policy: Policy, task_id: &str) -> Self {
        let result = match &mut invoked.result {
            CallResult::Returned(Ok(outcome)) => Ok(*outcome),
            CallResult::Returned(Err(e)) => Err(std::mem::replace(e, Error::service("", ""))),
            CallResult::CutShort(..) => unreachable!("a call cut short breaks out before Element"),
        };
        let decision = decide(policy, task_id, result);
        Self {
            index,
            invoked,
            decision,
        }
    }

    /// Whether the element fails the task or halts the workflow, under the
    /// per-element policy: no new call starts after it, and the fold stops
    /// after it.
    fn stops(&self) -> bool {
        matches!(self.decision.flow, Flow::Fail(_) | Flow::Halt)
    }
}

/// One element's call: the task's `start` with the element bound and its
/// index in the call's info, timed when `timed`.
#[allow(
    clippy::too_many_arguments,
    reason = "one call site's per-element facts, passed straight to `invoke`"
)]
async fn call_element<'r>(
    start: CallStart<'r>,
    fan: &'r FanSite,
    index: usize,
    item: &'r OwnedDataValue,
    bound: &'r dyn BoundCall,
    layers: Layers<'r>,
    timed: bool,
    timers: bool,
) -> (usize, Invoked) {
    let binding = Binding {
        item: &fan.r#as,
        value: item,
        index: &fan.index,
        index_value: OwnedDataValue::from(index),
    };
    let start = CallStart {
        binding: Some(&binding),
        info: CallInfo {
            element_index: Some(index),
            ..start.info
        },
        ..start
    };
    let started = timed.then(record::now);
    let mut invoked = invoke(start, bound, layers, fan.collect.as_ref(), timers).await;
    invoked.elapsed = started.map(record::since);
    (index, invoked)
}

/// `items`, unless `into` has no slot for one of them: an index past
/// [`MAX_ARRAY_INDEX`] is a key, so its result's path would name no element
/// and the trace's replay would put it nowhere.
fn fits_into<'e>(
    fan: &FanSite,
    task: &str,
    items: std::borrow::Cow<'e, [OwnedDataValue]>,
) -> Result<std::borrow::Cow<'e, [OwnedDataValue]>, Error> {
    if fan.into.is_none() || items.len() <= MAX_ARRAY_INDEX + 1 {
        return Ok(items);
    }
    Err(Error::new(
        ErrorKind::Eval,
        format!(
            "{} has {} elements, and into holds at most {}, one result per index up to {MAX_ARRAY_INDEX}",
            Over::FanOut(task),
            items.len(),
            MAX_ARRAY_INDEX + 1
        ),
    ))
}

/// `into`'s segments and one more for an element's slot, which each
/// element's fold sets to its index: no path is built per element.
fn slot_segs(into: &Path) -> Vec<Seg> {
    debug_assert!(
        into.index_child(0).is_some(),
        "the compiler leaves room below `into` for an index"
    );
    let mut segs = Vec::with_capacity(into.segs().len() + 1);
    segs.extend_from_slice(into.segs());
    segs.push(Seg::index(0));
    segs
}

/// One `FanOut` op: `over`, the calls, `into` pre-filled, the fold in
/// element order, then the task's own policy; the flow.
///
/// An `over` that is not an array is one `Err` of the task, decided under
/// the task's policy and recorded with no element, and so is one with more
/// elements than `into` has slots for. An empty one writes
/// `into = []` with one `Change`, whose old value is `null` as v3's, and
/// records the task once, with no element.
pub(crate) async fn fan_out<'p>(
    rc: &mut RunContext<'_, 'p>,
    env: &mut RunEnv<'_, 'p>,
    wf: &'p CompiledWorkflow,
    site: &'p CallSite,
    fan: &'p FanSite,
    bound: &'p dyn BoundCall,
) -> Flow {
    let task = &wf.tasks[site.task as usize];
    let task_id = env.ids(site.task).task;
    let policy = site.policy;
    let over = elements(rc, env.exprs, &fan.over, Over::FanOut(&task.id))
        .and_then(|items| fits_into(fan, &task.id, items));
    let items = match over {
        Ok(items) if !items.is_empty() => items,
        // Nothing to call: an empty array completes the task, anything else
        // fails it.
        no_calls => {
            let errors_before = env.errors.len();
            let started = env.watch.timed.then(record::now);
            let result = no_calls.map(|_| {
                if let Some(into) = &fan.into {
                    rc.set(into, OwnedDataValue::Array(Vec::new()), None);
                    if env.capture.captures() {
                        env.changes.push(Change::write(
                            Arc::clone(into.dotted()),
                            OwnedDataValue::Null,
                            OwnedDataValue::Array(Vec::new()),
                        ));
                    }
                }
                Outcome::Ok
            });
            let decision = decide(policy, task_id, result);
            let done = Completed::new(
                env,
                site.task,
                None,
                started.map(record::since),
                errors_before,
                false,
            );
            return record::record(rc, env, wf, done, decision);
        }
    };

    let n = items.len();
    let per_element = policy.per_element();
    let mut finished: Vec<Option<Element>> = (0..n).map(|_| None).collect();
    // The first call the run's limits cut short: its index, the records it
    // added, its error and how long it ran. The fan-out folds nothing then.
    let mut cut = None;
    {
        // The reader is taken in its own statement and every call holds a
        // copy: nothing writes to the run until the last call has finished.
        let reader = rc.reader();
        let (start, layers) = call_start(env, reader, wf, site);
        let mut pending = items.iter().enumerate();
        let mut in_flight = FuturesUnordered::new();
        let mut stop = false;
        loop {
            while !stop && in_flight.len() < fan.max_concurrency {
                let Some((index, item)) = pending.next() else {
                    break;
                };
                in_flight.push(call_element(
                    start,
                    fan,
                    index,
                    item,
                    bound,
                    layers,
                    env.watch.timed,
                    env.limits.timers,
                ));
            }
            let Some((index, invoked)) = in_flight.next().await else {
                break;
            };
            if let CallResult::CutShort(stop, error) = invoked.result {
                // Dropping `in_flight` drops the calls still running.
                cut = Some((index, invoked.errors, stop, error, invoked.elapsed));
                break;
            }
            let element = Element::new(index, invoked, per_element, task_id);
            stop |= element.stops();
            finished[index] = Some(element);
        }
    }
    drop(items);
    if let Some((index, errors, stop, e, elapsed)) = cut {
        let errors_before = env.errors.len();
        let ids = env.ids(site.task);
        let mut kept: Vec<_> = finished
            .into_iter()
            .flatten()
            .map(|element| (element.index, element.invoked.errors))
            .collect();
        kept.push((index, errors));
        kept.sort_unstable_by_key(|&(index, _)| index);
        for (index, errors) in kept {
            keep_records(ids, env.now_text, env.errors, errors, Some(index));
        }
        let decision = policy::cut_short(task_id, stop, &e);
        let done = Completed::new(env, site.task, None, elapsed, errors_before, false);
        return record::record(rc, env, wf, done, decision);
    }

    // One slot per element, so an element that failed or never ran is a
    // `null` at its own index. Written by the engine: no `Change` for the
    // audit, and in a trace that keeps changes the first change of the first
    // folded element's step.
    if let Some(into) = &fan.into {
        if let Some(trace) = env.watch.trace.as_deref_mut().filter(|t| t.changes()) {
            trace.push_prelude(Change::write(
                Arc::clone(into.dotted()),
                rc.get_or_null(into),
                OwnedDataValue::Array(vec![OwnedDataValue::Null; n]),
            ));
        }
        rc.set(
            into,
            OwnedDataValue::Array(vec![OwnedDataValue::Null; n]),
            None,
        );
    }
    let mut slot = fan.into.as_ref().map(|into| (into, slot_segs(into)));
    let mut halted = false;
    let mut rejected = false;
    let mut failed = false;
    for element in finished.into_iter().flatten() {
        let Element {
            index,
            invoked,
            decision,
        } = element;
        let Settled {
            result,
            collected,
            elapsed,
            errors_before,
            marked,
            ..
        } = settle(rc, env, site.task, invoked, Some(index));
        let ok = result.is_ok();
        let (rejected_here, failed_here) = (decision.rejected(), decision.failed());
        if let Some((into, segs)) = slot.as_mut() {
            if let Some(last) = segs.last_mut() {
                *last = Seg::index(index);
            }
            let value = match collected {
                Some(value) if !rejected_here && !failed_here => value,
                _ => OwnedDataValue::Null,
            };
            // An `Err` records no change, its result's included, as v3's.
            if ok && env.capture.captures() {
                // The dotted path `into.index_child(index)` would have.
                env.changes.push(Change::write(
                    Arc::from(format!("{}.{index}", into.dotted())),
                    OwnedDataValue::Null,
                    value.clone(),
                ));
            }
            rc.set_segs(segs, value, None);
        }
        rejected |= rejected_here;
        failed |= failed_here;
        let done = Completed::new(env, site.task, Some(index), elapsed, errors_before, marked);
        let flow = record::record(rc, env, wf, done, decision);
        rc.tick();
        match flow {
            flow @ (Flow::Fail(_) | Flow::Stop(_)) => return flow,
            Flow::Halt => {
                halted = true;
                break;
            }
            Flow::Next | Flow::Jump(_) => {}
        }
    }
    policy.after_fan_out(halted, rejected, failed)
}
