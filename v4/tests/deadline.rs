extern crate dataflow_rs_v4 as dataflow_rs;

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
use dataflow_rs::{Ctx, Engine, Expr, Function, Message, Outcome, Result, Run, RunOptions};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
async fn a_fan_out_cut_short_folds_nothing() {
    // Cut by an element's race: of three elements in flight, two finish and
    // the second outlasts the deadline. Nothing is folded, the finished
    // elements' writes, records and results included. The task is recorded
    // once, with no element, as a call cut short is, and the run stops.
    let raced = [workflow(
        "w",
        vec![
            fan(json!([100, 10_000, 100]), 3),
            map("after", "data.after", json!(true)),
        ],
    )];
    // Cut at an element's start: one at a time, the first sleeps to the
    // deadline and finishes; the second's check cuts it before it runs.
    let checked = [workflow("w", vec![fan(json!([1000, 0]), 1)])];
    for workflows in [&raced[..], &checked[..]] {
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
        assert_eq!(errors(&m), [timed_out("t")]);
        assert_eq!(data(&m), json!({"input": 1}));
        assert_eq!(considered(&run), [("w", 1, Some(WorkflowExit::Stopped))]);
    }
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
