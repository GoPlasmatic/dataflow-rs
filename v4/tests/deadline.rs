extern crate dataflow_rs_v4 as dataflow_rs;

// The run's deadline and its cancellation token (I92), end to end, under a
// paused clock: nothing here sleeps for real. Tokio advances the clock to
// the next timer whenever every task waits, so a call that sleeps ten
// seconds past a one-second deadline costs no wall time.

use dataflow_rs::run::{CancelToken, Instant, RunStatus, WorkflowExit};
use dataflow_rs::{Engine, Message, Run, RunOptions};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::time::Duration;

/// Long enough for anything that should resolve, on the paused clock: a
/// future that never resolves fails its `timeout` at once instead of hanging.
const PATIENCE: Duration = Duration::from_secs(3600);

/// An engine over `workflows`.
fn engine(workflows: &[Value]) -> Engine {
    workflows
        .iter()
        .fold(Engine::builder(), |b, w| b.workflow_json(w))
        .build()
        .unwrap_or_else(|issues| panic!("the engine builds: {issues:?}"))
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
