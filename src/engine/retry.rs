//! Retrying an operation that failed for a reason worth retrying.
//!
//! The crate has carried a complete retryability *classification* with no
//! *mechanism*: [`DataflowError::retryable`] sorts every variant, and
//! [`ErrorInfo`](crate::ErrorInfo) has `retry_attempted` / `retry_count` fields,
//! but no engine code path acts on any of it. Every host has written the same
//! loop.
//!
//! This module supplies the loop. It is deliberately **not** engine-level
//! automatic retry: the engine cannot know which handlers are idempotent — an
//! SMTP send that times out after `DATA` is indistinguishable from one that
//! succeeded, and retrying duplicates the mail. Whether to retry stays a
//! per-handler, per-call-site decision; the crate just supplies the correct
//! loop for those that opt in.
//!
//! # Not available on wasm32
//!
//! Backoff needs a timer, and tokio's time driver does not run on
//! `wasm32-unknown-unknown`. The whole module is `cfg`-gated off that target.

use crate::engine::error::{DataflowError, Result};
use std::future::Future;
use std::time::Duration;
use tokio::time::Instant;

/// The longest a single backoff sleep may grow to, however many attempts have
/// failed. Without a ceiling, doubling reaches minutes and a caller waiting on
/// the result has no idea why.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// How hard to retry, and for how long overall.
///
/// ```
/// use dataflow_rs::RetryPolicy;
/// use std::time::Duration;
///
/// // Three retries, 100ms doubling, but never more than 5s in total.
/// let policy = RetryPolicy {
///     max_retries: 3,
///     retry_delay_ms: 100,
///     deadline: Some(Duration::from_secs(5)),
/// };
/// assert_eq!(policy.max_retries, 3);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries *after* the first attempt. `0` means try once and give up.
    pub max_retries: u32,
    /// Base delay. Doubles per attempt, capped at 60s.
    pub retry_delay_ms: u64,
    /// Wall-clock ceiling for the **whole** loop, sleeps included.
    ///
    /// Without this, a call with a 30s per-attempt timeout and capped backoff
    /// can run to roughly 127s under a 30s caller budget — the per-attempt
    /// bound says nothing about the total.
    pub deadline: Option<Duration>,
}

impl Default for RetryPolicy {
    /// Three retries, 100ms base delay, no deadline.
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_delay_ms: 100,
            deadline: None,
        }
    }
}

impl RetryPolicy {
    /// A policy that never retries. Useful as an explicit opt-out at a call
    /// site that takes a policy.
    pub fn none() -> Self {
        Self {
            max_retries: 0,
            retry_delay_ms: 0,
            deadline: None,
        }
    }

    /// The backoff before retry number `retry` (1-based), capped.
    fn backoff(&self, retry: u32) -> Duration {
        let doubled = self
            .retry_delay_ms
            .checked_mul(1u64 << retry.min(32).saturating_sub(1))
            .map_or(MAX_BACKOFF, Duration::from_millis);
        doubled.min(MAX_BACKOFF)
    }
}

/// Run `operation`, retrying while it fails retryably and budget remains.
///
/// Retries only when [`DataflowError::retryable`] says so — a validation error
/// fails once and returns immediately, because trying it again cannot help.
///
/// ```no_run
/// use dataflow_rs::{RetryPolicy, retry_with_policy, DataflowError, Result};
/// # async fn demo() -> Result<String> {
/// # async fn call_the_service() -> Result<String> { Ok(String::new()) }
/// let body = retry_with_policy(RetryPolicy::default(), "user_service", || async {
///     call_the_service().await
/// })
/// .await?;
/// # Ok(body)
/// # }
/// ```
///
/// # Deadline
///
/// The deadline covers the whole loop, sleeps included. A backoff that would
/// cross it is **skipped** rather than slept: sleeping only to fail afterwards
/// spends latency the caller is already waiting on. The loop then ends with the
/// last error.
///
/// Timing uses [`tokio::time::Instant`], so the deadline stays coherent with
/// the sleeps under `tokio::time::pause()`.
pub async fn retry_with_policy<T, F, Fut>(
    policy: RetryPolicy,
    label: &str,
    operation: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    retry_with_attempts(policy, label, operation).await.0
}

/// As [`retry_with_policy`], also reporting how many attempts were made.
///
/// The count is what fills [`ErrorInfo::retry_attempted`](crate::ErrorInfo) and
/// `retry_count` — the two fields the crate has always carried with nothing to
/// populate them:
///
/// ```no_run
/// use dataflow_rs::{ErrorInfo, RetryPolicy, retry_with_attempts};
/// # async fn demo() {
/// # async fn call() -> dataflow_rs::Result<()> { Ok(()) }
/// let (result, attempts) =
///     retry_with_attempts(RetryPolicy::default(), "svc", || async { call().await }).await;
///
/// if let Err(err) = result {
///     let mut info = ErrorInfo::simple_ref("SVC_FAILED", &err.to_string(), None);
///     info.retry_attempted = Some(attempts > 1);
///     info.retry_count = Some(attempts.saturating_sub(1));
/// }
/// # }
/// ```
pub async fn retry_with_attempts<T, F, Fut>(
    policy: RetryPolicy,
    label: &str,
    mut operation: F,
) -> (Result<T>, u32)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let started = Instant::now();
    let mut attempts = 0u32;
    let mut last: DataflowError;

    loop {
        attempts += 1;
        match operation().await {
            Ok(value) => return (Ok(value), attempts),
            Err(err) => last = err,
        }

        if !last.retryable() {
            log::debug!("{label}: not retryable after {attempts} attempt(s): {last}");
            return (Err(last), attempts);
        }

        let retry = attempts; // the retry we are about to consider, 1-based
        if retry > policy.max_retries {
            log::debug!("{label}: giving up after {attempts} attempt(s): {last}");
            return (Err(last), attempts);
        }

        let backoff = policy.backoff(retry);

        // The deadline covers the sleep too. Sleeping and *then* failing spends
        // latency the caller is already waiting on, so a backoff that would
        // cross the line ends the loop instead.
        if let Some(deadline) = policy.deadline {
            let elapsed = started.elapsed();
            if elapsed + backoff >= deadline {
                log::debug!(
                    "{label}: deadline {deadline:?} leaves no room for a {backoff:?} backoff \
                     after {elapsed:?}; stopping at {attempts} attempt(s)"
                );
                return (Err(last), attempts);
            }
        }

        log::debug!("{label}: retry {retry} in {backoff:?} after: {last}");
        tokio::time::sleep(backoff).await;
    }
}
