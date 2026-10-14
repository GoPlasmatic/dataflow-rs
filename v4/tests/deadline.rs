extern crate dataflow_rs_v4 as dataflow_rs;

// The run's deadline and its cancellation token (I92), end to end, under a
// paused clock: nothing here sleeps for real. Tokio advances the clock to
// the next timer whenever every task waits, so a call that sleeps ten
// seconds past a one-second deadline costs no wall time.

use dataflow_rs::run::CancelToken;
use std::time::Duration;

/// Long enough for anything that should resolve, on the paused clock: a
/// future that never resolves fails its `timeout` at once instead of hanging.
const PATIENCE: Duration = Duration::from_secs(3600);

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
