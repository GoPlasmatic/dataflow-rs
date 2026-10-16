// The run's deadline and its cancellation token (I92), end to end, under a
// paused clock: nothing here sleeps for real. Tokio advances the clock to
// the next timer whenever every task waits, so a call that sleeps ten
// seconds past a one-second deadline costs no wall time.
//
// One function, `slow`, drives the host calls: it writes `data.began` and
// records `NOTED`, sleeps `ms` on tokio's clock, then writes `data.slept`.
// A call cut short shows which of those stand.

use dataflow_rs::function::FunctionConfig;
use dataflow_rs::run::{CancelToken, ErrorRecord, Instant, RunStatus, WorkflowExit};
use dataflow_rs::{Ctx, Engine, Error, Expr, Function, Message, Outcome, Result, Run, RunOptions};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Compiles only while `run::Instant` is `std::time::Instant` natively (I86).
fn same(i: std::time::Instant) -> dataflow_rs::run::Instant {
    i
}

#[test]
fn run_instant_is_the_std_instant_natively() {
    let _ = same(std::time::Instant::now());
}

/// Long enough for anything that should resolve, on the paused clock: a
/// future that never resolves fails its `timeout` at once instead of hanging.
const PATIENCE: Duration = Duration::from_secs(3600);

/// `slow`'s config.
#[derive(FunctionConfig)]
struct SlowConfig {
    /// How long the call sleeps, in milliseconds.
    ms: Expr<u64>,
}

/// Writes `data.began` and records `NOTED`, sleeps, then writes
/// `data.slept`.
struct Slow;

impl Function for Slow {
    type Config = SlowConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &SlowConfig) -> Result<Outcome> {
        let ms = ctx.eval(&cfg.ms)?;
        ctx.set(&"data.began".parse()?, OwnedDataValue::from(&json!(true)))?;
        ctx.record_error(ErrorRecord::new("NOTED", "began"));
        tokio::time::sleep(Duration::from_millis(ms)).await;
        ctx.set(&"data.slept".parse()?, OwnedDataValue::from(&json!(ms)))?;
        Ok(Outcome::Ok)
    }
}

/// What `parent` saw: its `Ctx`'s deadline, whether it had a token, and
/// how its child run ended.
type Saw = (Option<Instant>, bool, String);

/// R60's child run: `nested` runs with the parent's deadline and a child of
/// its token, so the child reports how it ended instead of being dropped.
struct Parent {
    nested: Engine,
    saw: Arc<Mutex<Vec<Saw>>>,
}

impl Function for Parent {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _: &()) -> Result<Outcome> {
        let token = ctx.cancel().map(CancelToken::child);
        let mut opts = RunOptions::default();
        if let Some(deadline) = ctx.deadline() {
            opts = opts.deadline(deadline);
        }
        if let Some(token) = &token {
            opts = opts.cancel(token);
        }
        let child = self.nested.run_with(&mut message(), &opts).await;
        let saw = (
            ctx.deadline(),
            ctx.cancel().is_some(),
            format!("{:?}", child.status()),
        );
        self.saw.lock().unwrap().push(saw);
        Ok(Outcome::Ok)
    }
}

/// An engine over `workflows`, `slow` registered.
fn engine(workflows: &[Value]) -> Engine {
    workflows
        .iter()
        .fold(Engine::builder().register("slow", Slow), |b, w| {
            b.workflow_json(w)
        })
        .build()
        .unwrap_or_else(|issues| panic!("the engine builds: {issues:?}"))
}

/// A `slow` task sleeping `ms`.
fn slow(id: &str, ms: u64) -> Value {
    json!({"id": id, "function": {"name": "slow", "input": {"ms": ms}}})
}

/// The fan-out task `t`: `slow` once per element of `over`, each sleeping
/// its element's `ms`, at most `max` at once, collecting `data.slept` into
/// `data.outs`.
fn fan(over: Value, max: usize) -> Value {
    json!({"id": "t", "for_each": {"over": over, "as": "p", "max_concurrency": max,
            "collect": "data.slept", "into": "data.outs"},
           "function": {"name": "slow", "input": {"ms": {"var": "temp_data.p"}}}})
}

/// A message whose `data` is `{"input": 1}`.
fn message() -> Message {
    Message::from_parts(
        OwnedDataValue::Null,
        OwnedDataValue::from(&json!({"input": 1})),
        OwnedDataValue::Null,
    )
    .with_id("m")
}

/// A `map` task writing `logic` at `path`.
fn map(id: &str, path: &str, logic: Value) -> Value {
    json!({"id": id, "function": {"name": "map", "input": {
        "mappings": [{"path": path, "logic": logic}]}}})
}

/// A workflow `id` holding `tasks`.
fn workflow(id: &str, tasks: Vec<Value>) -> Value {
    json!({"id": id, "tasks": tasks})
}

/// A deadline `ms` from now, on the paused clock.
fn after(ms: u64) -> Instant {
    (tokio::time::Instant::now() + Duration::from_millis(ms)).into_std()
}

fn data(m: &Message) -> Value {
    m.to_json()["context"]["data"].clone()
}

fn progress(m: &Message) -> Value {
    m.to_json()["context"]["metadata"]["progress"].clone()
}

/// `(task, status, element)` of each audit entry.
fn audit(m: &Message) -> Vec<(String, usize, Option<usize>)> {
    m.audit()
        .iter()
        .map(|e| (e.task_id.to_string(), e.status, e.element_index))
        .collect()
}

/// `(code, message)` of each error record.
fn errors(m: &Message) -> Vec<(String, String)> {
    m.errors()
        .iter()
        .map(|e| (e.code.clone(), e.message.clone()))
        .collect()
}

fn error(code: &str, message: &str) -> (String, String) {
    (code.to_owned(), message.to_owned())
}

/// The record of a call to `slow` the deadline cut short, on task `task`.
fn timed_out(task: &str) -> (String, String) {
    error(
        "TIMEOUT_ERROR",
        &format!("Task {task} error: Timeout error: the run's deadline passed during `slow`"),
    )
}

/// The record of a call to `slow` the token cut short, on task `task`.
fn cancelled(task: &str) -> (String, String) {
    error(
        "CANCELLED",
        &format!("Task {task} error: the run was cancelled during `slow`"),
    )
}

/// Cancels a clone of `token` once `ms` have passed, from another task.
fn cancel_after(token: &CancelToken, ms: u64) {
    let token = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        token.cancel();
    });
}

/// `(id, sweeps, exit)` of each workflow the run considered.
fn considered(run: &Run) -> Vec<(&str, u32, Option<WorkflowExit>)> {
    run.workflows().map(|w| (w.id, w.sweeps, w.exit)).collect()
}

/// Whether `token.cancelled()` resolves, waiting on the paused clock.
async fn resolves(token: &CancelToken) -> bool {
    tokio::time::timeout(PATIENCE, token.cancelled())
        .await
        .is_ok()
}

#[tokio::test(start_paused = true)]
async fn a_child_token_trips_with_its_parent_and_on_its_own() {
    // On its own: a task waiting on the child is woken, and the parent stays
    // as it was.
    let parent = CancelToken::new();
    let child = parent.child();
    let waiting = tokio::spawn({
        let child = child.clone();
        async move { child.cancelled().await }
    });
    // The waiter registers before anything is cancelled.
    tokio::task::yield_now().await;
    child.cancel();
    tokio::time::timeout(PATIENCE, waiting)
        .await
        .expect("the child's waiter is woken")
        .unwrap();
    assert!(!parent.is_cancelled());
    assert!(
        !resolves(&parent).await,
        "cancelling a child leaves its parent"
    );

    // With its parent: a child and a grandchild waiting are both woken.
    let other = parent.child();
    let deep = other.child();
    let waiters = [other.clone(), deep.clone()]
        .map(|token| tokio::spawn(async move { token.cancelled().await }));
    tokio::task::yield_now().await;
    assert!(!other.is_cancelled() && !deep.is_cancelled());
    parent.cancel();
    for waiter in waiters {
        tokio::time::timeout(PATIENCE, waiter)
            .await
            .expect("a descendant's waiter is woken")
            .unwrap();
    }
    assert!(other.is_cancelled() && deep.is_cancelled());

    // A token already cancelled, or made from one that is, resolves at once.
    assert!(resolves(&parent).await);
    assert!(resolves(&parent.child()).await);
}

#[tokio::test(start_paused = true)]
async fn a_deadline_is_checked_between_ops_and_at_every_sweep() {
    let passed = RunOptions::default().deadline(after(0));

    // At every sweep: a loop of 10,000 `map` sweeps stops as its first sweep
    // starts, before its bindings. Nothing runs and nothing is recorded on
    // the message; the `Run` says why.
    let e = engine(&[
        json!({"id": "w", "loop": {"counter": "i", "max": 10_000}, "tasks": [
            map("m", "data.n", json!({"var": "temp_data.i"}))]}),
        workflow("next", vec![map("x", "data.x", json!(1))]),
    ]);
    let mut m = message();
    let run = e.run_with(&mut m, &passed).await;
    assert!(
        matches!(run.status(), RunStatus::TimedOut),
        "{:?}",
        run.status()
    );
    assert!(run.retryable() && run.partial());
    assert!(m.audit().is_empty() && m.errors().is_empty());
    assert_eq!(data(&m), json!({"input": 1}));
    assert_eq!(considered(&run), [("w", 0, Some(WorkflowExit::Stopped))]);

    // Between ops: with no loop and no host call, the clock is read every
    // 64 ops, so of 100 `map`s the first 63 run. The stop records nothing.
    let maps = (0..100)
        .map(|i| map(&format!("m{i}"), &format!("data.m{i}"), json!(i)))
        .collect();
    let e = engine(&[workflow("w", maps)]);
    let mut m = message();
    let run = e.run_with(&mut m, &passed).await;
    assert!(matches!(run.status(), RunStatus::TimedOut));
    assert_eq!(m.audit().len(), 63);
    assert!(m.errors().is_empty());
    assert_eq!(considered(&run), [("w", 1, Some(WorkflowExit::Stopped))]);

    // Without the deadline the same run completes.
    let mut m = message();
    let run = e.run(&mut m).await;
    assert!(matches!(run.status(), RunStatus::Completed));
    assert_eq!(m.audit().len(), 100);
}

#[tokio::test(start_paused = true)]
async fn a_host_call_past_the_deadline_is_cut_short_and_recorded() {
    // `on_failure: continue` on the task and on the workflow changes
    // nothing: a call cut short stops the run whatever the policies.
    let mut t = slow("t", 10_000);
    t["on_failure"] = json!("continue");
    let mut w = workflow("w", vec![t, map("after", "data.after", json!(true))]);
    w["on_failure"] = json!("continue");
    let e = engine(&[w, workflow("next", vec![map("x", "data.x", json!(1))])]);
    let mut m = message();
    let start = tokio::time::Instant::now();
    let run = e
        .run_with(&mut m, &RunOptions::default().deadline(after(1000)))
        .await;
    // Cut at the deadline, not after its ten seconds.
    let took = start.elapsed();
    assert!(
        took >= Duration::from_secs(1) && took < Duration::from_secs(2),
        "{took:?}"
    );
    assert!(
        matches!(run.status(), RunStatus::TimedOut),
        "{:?}",
        run.status()
    );
    assert!(run.retryable() && run.partial());
    // Recorded as an `Err` from the function is: a 500, the progress, the
    // record; the records it added before the cut are kept.
    assert_eq!(audit(&m), [("t".to_owned(), 500, None)]);
    assert!(m.audit()[0].changes.is_empty());
    assert_eq!(
        progress(&m),
        json!({"workflow_id": "w", "task_id": "t", "status_code": 500})
    );
    assert_eq!(errors(&m), [error("NOTED", "began"), timed_out("t")]);
    // Its writes were dropped with it, and nothing after it ran.
    assert_eq!(data(&m), json!({"input": 1}));
    assert_eq!(considered(&run), [("w", 1, Some(WorkflowExit::Stopped))]);
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_token_stops_the_run_at_the_next_check() {
    // Tripped from another task during a host call: the call's race against
    // the token cuts it short.
    let e = engine(&[workflow("w", vec![slow("t", 10_000), slow("u", 0)])]);
    let token = CancelToken::new();
    cancel_after(&token, 1000);
    let mut m = message();
    let run = e
        .run_with(&mut m, &RunOptions::default().cancel(&token))
        .await;
    assert!(
        matches!(run.status(), RunStatus::Cancelled),
        "{:?}",
        run.status()
    );
    assert!(!run.retryable() && run.partial());
    assert_eq!(audit(&m), [("t".to_owned(), 500, None)]);
    assert_eq!(errors(&m), [error("NOTED", "began"), cancelled("t")]);
    assert_eq!(data(&m), json!({"input": 1}));
    assert_eq!(considered(&run), [("w", 1, Some(WorkflowExit::Stopped))]);

    // Tripped before the run: built-ins run until a check, and the first
    // host call's check cuts it before the function runs.
    let e = engine(&[workflow(
        "w",
        vec![map("m", "data.m", json!(1)), slow("t", 0)],
    )]);
    let mut m = message();
    let run = e
        .run_with(&mut m, &RunOptions::default().cancel(&token))
        .await;
    assert!(matches!(run.status(), RunStatus::Cancelled));
    assert_eq!(
        audit(&m),
        [("m".to_owned(), 200, None), ("t".to_owned(), 500, None)]
    );
    assert_eq!(errors(&m), [cancelled("t")]);
    assert_eq!(data(&m), json!({"input": 1, "m": 1}));
}

#[tokio::test(start_paused = true)]
async fn a_finished_call_wins_over_a_simultaneous_deadline() {
    // `t` sleeps exactly to the deadline, so its timer and the deadline's
    // fire together. The race is biased: the call, polled first, finishes
    // and lands. `u` then starts past the deadline, and its check cuts it
    // before it runs.
    let e = engine(&[workflow("w", vec![slow("t", 1000), slow("u", 0)])]);
    let mut m = message();
    let run = e
        .run_with(&mut m, &RunOptions::default().deadline(after(1000)))
        .await;
    assert!(matches!(run.status(), RunStatus::TimedOut));
    assert_eq!(
        audit(&m),
        [("t".to_owned(), 200, None), ("u".to_owned(), 500, None)]
    );
    assert_eq!(errors(&m), [error("NOTED", "began"), timed_out("u")]);
    assert_eq!(data(&m), json!({"input": 1, "began": true, "slept": 1000}));
}

#[tokio::test(start_paused = true)]
async fn a_fan_out_cut_short_folds_nothing_and_keeps_its_calls_records() {
    // Cut by an element's race: of three elements in flight, two finish and
    // the second outlasts the deadline. Nothing is folded, the finished
    // elements' writes and results included; the records every call that
    // returned added are kept, in element order, each stamped with its
    // element, as a single call cut short keeps its own (I96). The task is
    // recorded once, with no element, as a call cut short is, and the run
    // stops.
    let raced = [workflow(
        "w",
        vec![
            fan(json!([100, 10_000, 100]), 3),
            map("after", "data.after", json!(true)),
        ],
    )];
    // Cut at an element's start: one at a time, the first sleeps to the
    // deadline and finishes; the second's check cuts it before it runs, so
    // it added nothing.
    let checked = [workflow("w", vec![fan(json!([1000, 0]), 1)])];
    for (workflows, noted) in [(&raced[..], &[0, 1, 2][..]), (&checked[..], &[0][..])] {
        let e = engine(workflows);
        let mut m = message();
        let run = e
            .run_with(&mut m, &RunOptions::default().deadline(after(1000)))
            .await;
        assert!(
            matches!(run.status(), RunStatus::TimedOut),
            "{:?}",
            run.status()
        );
        assert_eq!(audit(&m), [("t".to_owned(), 500, None)]);
        let mut expected: Vec<_> = noted
            .iter()
            .map(|&i| (error("NOTED", "began"), Some(i)))
            .collect();
        expected.push((timed_out("t"), None));
        assert_eq!(elements(&m), expected);
        assert_eq!(data(&m), json!({"input": 1}));
        assert_eq!(considered(&run), [("w", 1, Some(WorkflowExit::Stopped))]);
    }
}

/// `((code, message), element)` of each error record.
fn elements(m: &Message) -> Vec<((String, String), Option<usize>)> {
    m.errors()
        .iter()
        .map(|e| ((e.code.clone(), e.message.clone()), e.element_index))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn ctx_exposes_the_deadline_and_the_token() {
    let saw = Arc::new(Mutex::new(Vec::new()));
    let parent = Parent {
        nested: engine(&[workflow("child", vec![slow("c", 10_000)])]),
        saw: Arc::clone(&saw),
    };
    let e = Engine::builder()
        .register("parent", parent)
        .workflow_json(&workflow(
            "w",
            vec![json!({"id": "p", "function": {"name": "parent", "input": {}}})],
        ))
        .build()
        .unwrap();

    // Neither: the function sees none, and its child runs to its end
    // (`slow`'s `NOTED` makes it `CompletedWithErrors`).
    let run = e.run(&mut message()).await;
    assert!(matches!(run.status(), RunStatus::Completed));

    // The deadline: the child inherits it, and its timer fires with the
    // parent's. The parent's call, polled first, finishes with what its
    // child reported.
    let deadline = after(1000);
    let mut m = message();
    let run = e
        .run_with(&mut m, &RunOptions::default().deadline(deadline))
        .await;
    assert!(matches!(run.status(), RunStatus::Completed));
    assert_eq!(audit(&m), [("p".to_owned(), 200, None)]);

    // The token: a child of it trips with it.
    let token = CancelToken::new();
    cancel_after(&token, 1000);
    e.run_with(&mut message(), &RunOptions::default().cancel(&token))
        .await;

    let saw = saw.lock().unwrap();
    assert_eq!(
        *saw,
        [
            (None, false, "CompletedWithErrors".to_owned()),
            (Some(deadline), false, "TimedOut".to_owned()),
            (None, true, "Cancelled".to_owned()),
        ]
    );
}

/// How [`Propagate`]'s child run is stopped short: never by the parent's
/// own limits.
#[derive(Clone, Copy)]
enum ChildStop {
    /// The child's deadline, `ms` from the call's start, earlier than any
    /// the parent has.
    Deadline(u64),
    /// The child's own token, cancelled before the child runs; the parent's
    /// is not.
    Cancelled,
}

/// R60's child run, propagated: `nested` runs under a limit of its own, and
/// the call returns its `into_result()?`, so the child's stop is the
/// call's `Err`.
struct Propagate {
    nested: Engine,
    stop: ChildStop,
}

impl Function for Propagate {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _: &()) -> Result<Outcome> {
        let token = ctx
            .cancel()
            .map_or_else(CancelToken::new, CancelToken::child);
        let mut opts = RunOptions::default().cancel(&token);
        match self.stop {
            ChildStop::Deadline(ms) => opts = opts.deadline(after(ms)),
            ChildStop::Cancelled => token.cancel(),
        }
        self.nested
            .run_with(&mut message(), &opts)
            .await
            .into_result()?;
        Ok(Outcome::Ok)
    }
}

#[tokio::test(start_paused = true)]
async fn a_child_runs_stop_is_the_calls_error_not_the_runs_limit() {
    // The child's own deadline passes, or its own token trips, and the call
    // returns that as an `Err` under the default policies: the parent's
    // limits never fired, so the parent is `Stopped` with the task's error,
    // neither `TimedOut` nor `Cancelled`.
    for (stop, code, text) in [
        (
            ChildStop::Deadline(1000),
            "TIMEOUT_ERROR",
            "Task p error: Timeout error: the run timed out",
        ),
        (
            ChildStop::Cancelled,
            "CANCELLED",
            "Task p error: the run was cancelled",
        ),
    ] {
        let e = Engine::builder()
            .register(
                "propagate",
                Propagate {
                    nested: engine(&[workflow("child", vec![slow("c", 10_000)])]),
                    stop,
                },
            )
            .workflow_json(&workflow(
                "w",
                vec![
                    json!({"id": "p", "function": {"name": "propagate", "input": {}}}),
                    map("after", "data.after", json!(true)),
                ],
            ))
            .build()
            .unwrap();
        let token = CancelToken::new();
        let opts = RunOptions::default().deadline(after(5000)).cancel(&token);
        let mut m = message();
        let run = e.run_with(&mut m, &opts).await;
        match run.status() {
            RunStatus::Stopped(e) => assert_eq!(e.code(), code),
            other => panic!("{other:?}"),
        }
        assert!(!token.is_cancelled());
        assert_eq!(audit(&m), [("p".to_owned(), 500, None)]);
        assert_eq!(errors(&m), [error(code, text)]);
        assert_eq!(data(&m), json!({"input": 1}));
        assert_eq!(considered(&run), [("w", 1, Some(WorkflowExit::Stopped))]);
    }
}

// -- A call's `Err` once its run's own limit has passed --
//
// Found by Orion's port of `channel_call` (phase 7c, Task O10): its tests
// `the_runs_deadline_during_the_child_ends_the_run_timed_out` and
// `the_runs_token_tripped_during_the_child_ends_the_run_cancelled`, in
// `crates/orion-server/src/engine/functions/channel_call.rs`, ended the
// parent run `Stopped` until the function awaited `pending()` to let the
// race cut it short. A call that hands its child run the run's deadline and
// a child of its token sees the child stop in the wake the limit fires in,
// and returns the child's `Err` before the race, polled call first, can cut
// it short; on wasm nothing races at all. A call's `Err` once its run's own
// limit has passed is now the limit's, so the run ends on the limit.

/// R60's child run as Orion's `channel_call` makes it: `nested` runs under
/// the run's deadline, or the call's own `own` ms when that is strictly
/// earlier (a tie is the run's), and a child of the run's token; the call
/// returns its `into_result()?`, so the child's stop is the call's `Err`.
struct Inherit {
    nested: Engine,
    own: Option<u64>,
}

impl Function for Inherit {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _: &()) -> Result<Outcome> {
        let token = ctx.cancel().map(CancelToken::child);
        let deadline = match (self.own.map(after), ctx.deadline()) {
            (Some(own), Some(run)) if own < run => Some(own),
            (own, run) => run.or(own),
        };
        let mut opts = RunOptions::default();
        if let Some(deadline) = deadline {
            opts = opts.deadline(deadline);
        }
        if let Some(token) = &token {
            opts = opts.cancel(token);
        }
        self.nested
            .run_with(&mut message(), &opts)
            .await
            .into_result()?;
        Ok(Outcome::Ok)
    }
}

/// Cancels its token, then sleeps ten seconds: a call its run's token cuts
/// short in the poll it trips it in.
struct Trip(CancelToken);

impl Function for Trip {
    type Config = ();

    async fn call(&self, _: &mut Ctx<'_>, _: &()) -> Result<Outcome> {
        self.0.cancel();
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(Outcome::Ok)
    }
}

/// `fail`'s config.
#[derive(FunctionConfig)]
struct FailConfig {
    /// How long the call sleeps before it fails, in milliseconds.
    ms: Expr<u64>,
}

/// Records `NOTED`, sleeps, then fails with `BOOM`: an `Err` of its own.
struct Fail;

impl Function for Fail {
    type Config = FailConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &FailConfig) -> Result<Outcome> {
        let ms = ctx.eval(&cfg.ms)?;
        ctx.record_error(ErrorRecord::new("NOTED", "began"));
        tokio::time::sleep(Duration::from_millis(ms)).await;
        Err(Error::service("BOOM", "it failed"))
    }
}

/// A child engine whose one task sleeps ten seconds.
fn sleeping_child() -> Engine {
    engine(&[workflow("child", vec![slow("c", 10_000)])])
}

/// An engine whose workflow `w` holds `first`, then `after`, and then the
/// workflow `next`; `inherit` calls `f` and `fail` is registered.
fn calling(f: Inherit, first: Value) -> Engine {
    Engine::builder()
        .register("inherit", f)
        .register("fail", Fail)
        .workflow_json(&workflow(
            "w",
            vec![first, map("after", "data.after", json!(true))],
        ))
        .workflow_json(&workflow("next", vec![map("x", "data.x", json!(1))]))
        .build()
        .unwrap_or_else(|issues| panic!("the engine builds: {issues:?}"))
}

/// The task `p` calling `inherit`, under `on_failure`.
fn inherit(on_failure: &str) -> Value {
    json!({"id": "p", "on_failure": on_failure,
           "function": {"name": "inherit", "input": {}}})
}

/// The record of a call to `function` on `task` the run's deadline ended.
fn deadline_during(task: &str, function: &str) -> (String, String) {
    error(
        "TIMEOUT_ERROR",
        &format!("Task {task} error: Timeout error: the run's deadline passed during `{function}`"),
    )
}

#[tokio::test(start_paused = true)]
async fn a_childs_timeout_once_the_runs_deadline_passed_ends_the_run_timed_out() {
    // The child's deadline is the run's: inherited, or the call's own when
    // that is later or tied. Its timer fires with the race's, and in that
    // wake the call, polled first, returns the child's `TimedOut`. The run's
    // deadline has passed by then, so the call is the deadline's: the run
    // ends `TimedOut`, whatever the task's policies, and its task records
    // what a call the deadline cut short records.
    for own in [None, Some(1000), Some(5000)] {
        let e = calling(
            Inherit {
                nested: sleeping_child(),
                own,
            },
            inherit("continue"),
        );
        let mut m = message();
        let start = tokio::time::Instant::now();
        let run = e
            .run_with(&mut m, &RunOptions::default().deadline(after(1000)))
            .await;
        assert_eq!(start.elapsed(), Duration::from_secs(1), "{own:?}");
        assert!(
            matches!(run.status(), RunStatus::TimedOut),
            "{own:?}: {:?}",
            run.status()
        );
        assert!(run.retryable() && run.partial());
        assert_eq!(audit(&m), [("p".to_owned(), 500, None)]);
        assert_eq!(errors(&m), [deadline_during("p", "inherit")]);
        assert_eq!(data(&m), json!({"input": 1}));
        assert_eq!(considered(&run), [("w", 1, Some(WorkflowExit::Stopped))]);
    }
}

#[tokio::test(start_paused = true)]
async fn a_childs_cancel_once_the_runs_token_tripped_ends_the_run_cancelled() {
    // Tripped from another task, which wakes the call: the child's own race
    // sees its token first and ends the child `Cancelled`. Tripped inside
    // the child, by its task: the call returns in its first poll, with no
    // timer and no wake, as every call returns on wasm. Either way the run's
    // token is cancelled when the call returns the child's `Cancelled`, so
    // the run ends `Cancelled`.
    for inside in [false, true] {
        let token = CancelToken::new();
        let nested = if inside {
            Engine::builder()
                .register("trip", Trip(token.clone()))
                .workflow_json(&workflow(
                    "child",
                    vec![json!({"id": "c", "function": {"name": "trip", "input": {}}})],
                ))
                .build()
                .unwrap()
        } else {
            cancel_after(&token, 1000);
            sleeping_child()
        };
        let e = calling(Inherit { nested, own: None }, inherit("continue"));
        let opts = RunOptions::default().deadline(after(5000)).cancel(&token);
        let mut m = message();
        let run = e.run_with(&mut m, &opts).await;
        assert!(
            matches!(run.status(), RunStatus::Cancelled),
            "inside {inside}: {:?}",
            run.status()
        );
        assert!(!run.retryable() && run.partial());
        assert_eq!(audit(&m), [("p".to_owned(), 500, None)]);
        assert_eq!(
            errors(&m),
            [error(
                "CANCELLED",
                "Task p error: the run was cancelled during `inherit`"
            )]
        );
        assert_eq!(data(&m), json!({"input": 1}));
        assert_eq!(considered(&run), [("w", 1, Some(WorkflowExit::Stopped))]);
    }
}

#[tokio::test(start_paused = true)]
async fn a_childs_own_earlier_deadline_is_the_calls_failure_under_its_policy() {
    // The call's own deadline, a second in, is strictly before the run's:
    // the child ends `TimedOut` on it, and the call returns that while the
    // run's limits hold. The call keeps its own error, and its task's
    // policy decides: `fail` stops the run with it, `continue` goes on.
    let token = CancelToken::new();
    let opts = RunOptions::default().deadline(after(5000)).cancel(&token);
    let child_timed_out = error(
        "TIMEOUT_ERROR",
        "Task p error: Timeout error: the run timed out",
    );
    let own = || Inherit {
        nested: sleeping_child(),
        own: Some(1000),
    };

    let mut m = message();
    let run = calling(own(), inherit("fail"))
        .run_with(&mut m, &opts)
        .await;
    match run.status() {
        RunStatus::Stopped(e) => assert_eq!(
            (e.code(), e.message()),
            ("TIMEOUT_ERROR", "the run timed out")
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(audit(&m), [("p".to_owned(), 500, None)]);
    assert_eq!(errors(&m), std::slice::from_ref(&child_timed_out));
    assert_eq!(data(&m), json!({"input": 1}));
    assert_eq!(considered(&run), [("w", 1, Some(WorkflowExit::Stopped))]);

    let mut m = message();
    let run = calling(own(), inherit("continue"))
        .run_with(&mut m, &opts)
        .await;
    assert!(
        matches!(run.status(), RunStatus::CompletedWithErrors),
        "{:?}",
        run.status()
    );
    assert_eq!(
        audit(&m),
        [
            ("p".to_owned(), 500, None),
            ("after".to_owned(), 200, None),
            ("x".to_owned(), 200, None),
        ]
    );
    assert_eq!(errors(&m), [child_timed_out]);
    assert_eq!(data(&m), json!({"input": 1, "after": true, "x": 1}));
    assert!(!token.is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn an_err_before_the_runs_deadline_keeps_its_policy_and_one_at_it_is_the_deadlines() {
    let fail = |ms: u64, on_failure: &str| {
        json!({"id": "f", "on_failure": on_failure,
               "function": {"name": "fail", "input": {"ms": ms}}})
    };
    let unused = || Inherit {
        nested: sleeping_child(),
        own: None,
    };
    let boom = error("BOOM", "Task f error: it failed");

    // Half a second before the deadline: the call's own error, under its
    // policy. `continue` goes on to the end; `fail` stops the run with it.
    let mut m = message();
    let run = calling(unused(), fail(500, "continue"))
        .run_with(&mut m, &RunOptions::default().deadline(after(1000)))
        .await;
    assert!(
        matches!(run.status(), RunStatus::CompletedWithErrors),
        "{:?}",
        run.status()
    );
    assert_eq!(
        audit(&m),
        [
            ("f".to_owned(), 500, None),
            ("after".to_owned(), 200, None),
            ("x".to_owned(), 200, None),
        ]
    );
    assert_eq!(errors(&m), [error("NOTED", "began"), boom.clone()]);

    let mut m = message();
    let run = calling(unused(), fail(500, "fail"))
        .run_with(&mut m, &RunOptions::default().deadline(after(1000)))
        .await;
    match run.status() {
        RunStatus::Stopped(e) => assert_eq!(e.code(), "BOOM"),
        other => panic!("{other:?}"),
    }
    assert_eq!(errors(&m), [error("NOTED", "began"), boom]);

    // At the deadline itself, the instant counts as passed: the call is the
    // deadline's, as the race would have made it had it been polled first.
    // Its own records are kept, as a call cut short keeps them.
    let mut m = message();
    let run = calling(unused(), fail(1000, "continue"))
        .run_with(&mut m, &RunOptions::default().deadline(after(1000)))
        .await;
    assert!(
        matches!(run.status(), RunStatus::TimedOut),
        "{:?}",
        run.status()
    );
    assert_eq!(audit(&m), [("f".to_owned(), 500, None)]);
    assert_eq!(
        errors(&m),
        [error("NOTED", "began"), deadline_during("f", "fail")]
    );
    assert_eq!(data(&m), json!({"input": 1}));
}

#[tokio::test(start_paused = true)]
async fn a_fan_out_elements_err_once_the_runs_deadline_passed_cuts_the_fan_out_short() {
    // Each element's call hands its child the run's deadline. The first to
    // return, in the wake the deadline fires in, returns its child's
    // `TimedOut`: the fan-out is cut short as an element's race cuts it,
    // nothing folded and the task recorded once with no element, and the
    // run ends `TimedOut`.
    let task = json!({"id": "t", "on_failure": "continue",
        "for_each": {"over": [1, 2, 3], "as": "p", "max_concurrency": 3},
        "function": {"name": "inherit", "input": {}}});
    let e = calling(
        Inherit {
            nested: sleeping_child(),
            own: None,
        },
        task,
    );
    let mut m = message();
    let run = e
        .run_with(&mut m, &RunOptions::default().deadline(after(1000)))
        .await;
    assert!(
        matches!(run.status(), RunStatus::TimedOut),
        "{:?}",
        run.status()
    );
    assert_eq!(audit(&m), [("t".to_owned(), 500, None)]);
    assert_eq!(elements(&m), [(deadline_during("t", "inherit"), None)]);
    assert_eq!(data(&m), json!({"input": 1}));
    assert_eq!(considered(&run), [("w", 1, Some(WorkflowExit::Stopped))]);
}
