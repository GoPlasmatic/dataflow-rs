//! `retry_with_policy` — the mechanism half of the crate's retryability model.
//!
//! Every timing assertion runs under `tokio::time::pause()`, so the clock only
//! advances when a sleep asks it to. That makes the deadline arithmetic exactly
//! checkable rather than approximately: a test that "takes about 700ms" would
//! be both slow and flaky.

#![cfg(not(target_arch = "wasm32"))]

use dataflow_rs::{DataflowError, Result, RetryPolicy, retry_with_attempts, retry_with_policy};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// Counts calls and fails the first `fail_times` of them.
fn flaky(
    fail_times: u32,
    err: fn() -> DataflowError,
) -> (
    Arc<AtomicU32>,
    impl FnMut() -> futures::future::Ready<Result<u32>>,
) {
    let calls = Arc::new(AtomicU32::new(0));
    let c = Arc::clone(&calls);
    let op = move || {
        let n = c.fetch_add(1, Ordering::SeqCst) + 1;
        futures::future::ready(if n > fail_times { Ok(n) } else { Err(err()) })
    };
    (calls, op)
}

fn transient() -> DataflowError {
    DataflowError::Timeout("upstream".into())
}

fn permanent() -> DataflowError {
    DataflowError::Validation("bad input".into())
}

#[tokio::test(start_paused = true)]
async fn a_transient_failure_is_retried_until_it_succeeds() {
    let (calls, op) = flaky(2, transient);
    let out = retry_with_policy(
        RetryPolicy {
            max_retries: 3,
            retry_delay_ms: 100,
            deadline: None,
        },
        "svc",
        op,
    )
    .await;

    assert_eq!(out.unwrap(), 3);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "two failures, then success"
    );
}

#[tokio::test(start_paused = true)]
async fn a_non_retryable_error_returns_immediately() {
    let (calls, op) = flaky(u32::MAX, permanent);
    let out = retry_with_policy(
        RetryPolicy {
            max_retries: 5,
            retry_delay_ms: 100,
            deadline: None,
        },
        "svc",
        op,
    )
    .await;

    assert!(out.is_err());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "retrying a validation error cannot help, so it must not be tried again"
    );
}

#[tokio::test(start_paused = true)]
async fn retries_stop_at_max_retries() {
    let (calls, op) = flaky(u32::MAX, transient);
    let (out, attempts) = retry_with_attempts(
        RetryPolicy {
            max_retries: 3,
            retry_delay_ms: 10,
            deadline: None,
        },
        "svc",
        op,
    )
    .await;

    assert!(out.is_err());
    assert_eq!(attempts, 4, "the first attempt plus three retries");
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[tokio::test(start_paused = true)]
async fn backoff_doubles_and_then_caps() {
    // 100, 200, 400, 800 … capped at 60s. Measured on the paused clock, so
    // these are exact.
    let start = tokio::time::Instant::now();
    let (_, op) = flaky(u32::MAX, transient);
    let _ = retry_with_policy(
        RetryPolicy {
            max_retries: 3,
            retry_delay_ms: 100,
            deadline: None,
        },
        "svc",
        op,
    )
    .await;

    assert_eq!(
        start.elapsed(),
        Duration::from_millis(100 + 200 + 400),
        "three backoffs, doubling"
    );
}

#[tokio::test(start_paused = true)]
async fn backoff_is_capped_at_sixty_seconds() {
    let start = tokio::time::Instant::now();
    let (_, op) = flaky(u32::MAX, transient);
    let _ = retry_with_policy(
        // 40s, then 80s -> capped to 60s.
        RetryPolicy {
            max_retries: 2,
            retry_delay_ms: 40_000,
            deadline: None,
        },
        "svc",
        op,
    )
    .await;

    assert_eq!(
        start.elapsed(),
        Duration::from_secs(40 + 60),
        "the second backoff is clamped rather than doubling to 80s"
    );
}

#[tokio::test(start_paused = true)]
async fn a_backoff_that_would_cross_the_deadline_is_skipped_entirely() {
    // The load-bearing behaviour: sleeping and *then* failing spends latency the
    // caller is already waiting on. With a 250ms deadline the third backoff
    // (400ms) cannot fit, so the loop ends without sleeping it.
    let start = tokio::time::Instant::now();
    let (calls, op) = flaky(u32::MAX, transient);

    let (out, attempts) = retry_with_attempts(
        RetryPolicy {
            max_retries: 10,
            retry_delay_ms: 100,
            deadline: Some(Duration::from_millis(250)),
        },
        "svc",
        op,
    )
    .await;

    // t=0   attempt 1 fails; 100ms backoff fits inside 250ms, so sleep it.
    // t=100 attempt 2 fails; the next backoff is 200ms and 100+200 >= 250,
    //       so the loop ends here rather than sleeping into a failure.
    assert!(out.is_err());
    assert_eq!(attempts, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        start.elapsed(),
        Duration::from_millis(100),
        "only the first backoff was slept"
    );
    assert!(
        start.elapsed() < Duration::from_millis(250),
        "and the loop came in under its own deadline rather than sleeping to 300ms \
         and failing anyway, which is what an unguarded backoff would do"
    );
}

#[tokio::test(start_paused = true)]
async fn the_deadline_bounds_the_whole_loop_not_each_attempt() {
    // Each attempt is instant here, so any overrun is the sleeps — which is the
    // case a per-attempt timeout cannot bound.
    let start = tokio::time::Instant::now();
    let (_, op) = flaky(u32::MAX, transient);

    let _ = retry_with_policy(
        RetryPolicy {
            max_retries: 20,
            retry_delay_ms: 1_000,
            deadline: Some(Duration::from_secs(10)),
        },
        "svc",
        op,
    )
    .await;

    assert!(
        start.elapsed() < Duration::from_secs(10),
        "the loop finished inside its budget, got {:?}",
        start.elapsed()
    );
}

#[tokio::test(start_paused = true)]
async fn max_retries_zero_tries_once() {
    let (calls, op) = flaky(u32::MAX, transient);
    let (out, attempts) = retry_with_attempts(RetryPolicy::none(), "svc", op).await;

    assert!(out.is_err());
    assert_eq!(attempts, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn a_service_error_is_retried_only_when_it_says_so() {
    for (retryable, expected_calls) in [(true, 3u32), (false, 1)] {
        let calls = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&calls);
        let op = move || {
            c.fetch_add(1, Ordering::SeqCst);
            futures::future::ready(Err::<u32, _>(
                DataflowError::service("upstream", "boom")
                    .retryable(retryable)
                    .build(),
            ))
        };

        let _ = retry_with_policy(
            RetryPolicy {
                max_retries: 2,
                retry_delay_ms: 1,
                deadline: None,
            },
            "svc",
            op,
        )
        .await;

        assert_eq!(
            calls.load(Ordering::SeqCst),
            expected_calls,
            "a Service error declaring retryable={retryable} must be honoured"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn the_attempt_count_is_what_fills_the_error_info_fields() {
    use dataflow_rs::ErrorInfo;

    let (_, op) = flaky(u32::MAX, transient);
    let (result, attempts) = retry_with_attempts(
        RetryPolicy {
            max_retries: 2,
            retry_delay_ms: 1,
            deadline: None,
        },
        "svc",
        op,
    )
    .await;

    let err = result.unwrap_err();
    let mut info = ErrorInfo::simple_ref("SVC_FAILED", &err.to_string(), None);
    info.retry_attempted = Some(attempts > 1);
    info.retry_count = Some(attempts - 1);

    assert_eq!(info.retry_attempted, Some(true));
    assert_eq!(
        info.retry_count,
        Some(2),
        "two retries after the first attempt"
    );
}
