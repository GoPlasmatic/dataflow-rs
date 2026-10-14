//! The run's limits (I92): its deadline and its cancellation token, and the
//! one rule that checks them.
//!
//! The interpreter checks them at every host call and every fan-out
//! element's start, at every sweep, and every 64 ops, and only when one is
//! set: a run with neither never reads the clock. The token is checked
//! first. A stop between ops or at a sweep records nothing on the message;
//! the [`Run`](super::Run) says why. Natively a host call also [`race`]s
//! them, one timer per call; on wasm only the checks run.

use crate::base::{CancelToken, Error, ErrorKind, Instant, Result};

/// The run's deadline and token, as its [`RunOptions`](super::RunOptions)
/// set them.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Limits<'r> {
    /// When the run must end.
    pub(crate) deadline: Option<Instant>,
    /// The token that cancels the run.
    pub(crate) cancel: Option<&'r CancelToken>,
}

/// Why a run must stop short.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    /// The token, or one of its ancestors, was cancelled.
    Cancelled,
    /// The deadline passed.
    TimedOut,
}

impl Stop {
    /// The error a stop between ops or at a sweep ends the run with. Nothing
    /// records it; its kind is the run's status.
    fn error(self) -> Error {
        match self {
            Self::Cancelled => Error::new(ErrorKind::Cancelled, "the run was cancelled"),
            Self::TimedOut => Error::new(ErrorKind::TimedOut, "the run's deadline passed"),
        }
    }

    /// The error of a call to `function` this stop cut short, recorded on
    /// its task.
    pub(crate) fn during(self, function: &str) -> Error {
        match self {
            Self::Cancelled => Error::new(
                ErrorKind::Cancelled,
                format!("the run was cancelled during `{function}`"),
            ),
            Self::TimedOut => Error::new(
                ErrorKind::TimedOut,
                format!("the run's deadline passed during `{function}`"),
            ),
        }
    }
}

impl Limits<'_> {
    /// Whether the run has a deadline or a token: without either, nothing
    /// is checked and nothing races.
    pub(crate) fn is_set(&self) -> bool {
        self.deadline.is_some() || self.cancel.is_some()
    }

    /// Why the run must stop now, if it must: the token first, then the
    /// deadline. The clock is read only when there is a deadline.
    pub(crate) fn stop(&self) -> Option<Stop> {
        if self.cancel.is_some_and(CancelToken::is_cancelled) {
            return Some(Stop::Cancelled);
        }
        match self.deadline {
            Some(deadline) if now() >= deadline => Some(Stop::TimedOut),
            _ => None,
        }
    }

    /// `Err` of kind `Cancelled` when the token tripped, else of kind
    /// `TimedOut` when the deadline passed.
    pub(crate) fn check(&self) -> Result<()> {
        self.stop().map_or(Ok(()), |stop| Err(stop.error()))
    }
}

/// `call` run to its end, unless the run's token or deadline ends it
/// first, when `call` is dropped and the stop says which.
///
/// Natively, with a limit set, `call` races the token's
/// [`cancelled`](CancelToken::cancelled) and a timer at the deadline, one
/// timer per call, created only once `call` is pending. The race is
/// `biased`: `call` is polled first, so a call that finishes in the poll a
/// limit fires in wins; then the token, as [`Limits::stop`] reads it first.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn race<F: Future>(
    call: F,
    limits: Limits<'_>,
) -> std::result::Result<F::Output, Stop> {
    if !limits.is_set() {
        return Ok(call.await);
    }
    let cancelled = async {
        match limits.cancel {
            Some(token) => token.cancelled().await,
            None => std::future::pending().await,
        }
    };
    let timer = async {
        match limits.deadline {
            Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        biased;
        output = call => Ok(output),
        () = cancelled => Err(Stop::Cancelled),
        () = timer => Err(Stop::TimedOut),
    }
}

/// On wasm nothing races: `call` runs to its end, and the checks around it
/// are the only ones.
#[cfg(target_arch = "wasm32")]
pub(crate) async fn race<F: Future>(
    call: F,
    _limits: Limits<'_>,
) -> std::result::Result<F::Output, Stop> {
    Ok(call.await)
}

/// The time a deadline is compared with.
fn now() -> Instant {
    #[cfg(test)]
    tests::CLOCK_READS.with(|reads| reads.set(reads.get() + 1));
    clock()
}

/// Natively, tokio's clock, the one a host call's timer runs on: the system
/// clock, unless a test paused it, when the checks see the time the timers
/// see.
#[cfg(not(target_arch = "wasm32"))]
fn clock() -> Instant {
    tokio::time::Instant::now().into_std()
}

/// On wasm, the system clock.
#[cfg(target_arch = "wasm32")]
fn clock() -> Instant {
    Instant::now()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::RunOptions;
    use crate::{Engine, Message};
    use serde_json::json;
    use std::cell::Cell;
    use std::time::Duration;

    thread_local! {
        /// How many times this thread read the limits' clock.
        pub(super) static CLOCK_READS: Cell<u32> = const { Cell::new(0) };
    }

    fn reads() -> u32 {
        CLOCK_READS.with(Cell::get)
    }

    #[test]
    fn the_token_is_checked_before_the_deadline_and_neither_when_unset() {
        let unset = Limits::default();
        assert!(!unset.is_set());
        let before = reads();
        assert!(unset.check().is_ok());
        assert_eq!(reads(), before, "no deadline, no clock");

        let token = CancelToken::new();
        let passed = Limits {
            deadline: Some(now()),
            cancel: Some(&token),
        };
        assert!(passed.is_set());
        assert_eq!(passed.stop(), Some(Stop::TimedOut));
        token.cancel();
        let e = passed.check().unwrap_err();
        assert_eq!(
            (e.code(), e.message()),
            ("CANCELLED", "the run was cancelled")
        );
        assert!(!e.is_retryable());

        let far = Limits {
            deadline: Some(now() + Duration::from_secs(3600)),
            cancel: None,
        };
        assert!(far.is_set() && far.check().is_ok());
        let past = Limits {
            deadline: Some(now()),
            cancel: None,
        };
        let e = past.check().unwrap_err();
        assert_eq!(
            (e.code(), e.message()),
            ("TIMEOUT_ERROR", "the run's deadline passed")
        );
        assert!(e.is_retryable());
    }

    #[tokio::test]
    async fn without_a_deadline_or_a_token_no_clock_is_read() {
        // A loop of sweeps and more than 64 ops: every place a limit is
        // checked, skipped when none is set.
        let maps: Vec<_> = (0..70)
            .map(|i| {
                json!({"id": format!("m{i}"), "function": {"name": "map", "input": {
                "mappings": [{"path": format!("data.m{i}"), "logic": i}]}}})
            })
            .collect();
        let engine = Engine::builder()
            .workflow_json(&json!({"id": "w", "loop": {"counter": "i", "max": 3}, "tasks": maps}))
            .build()
            .unwrap();
        let before = reads();
        engine.run(&mut Message::from_json(&json!({}))).await;
        assert_eq!(reads(), before);

        // A token alone is checked there too, and reads no clock either.
        let token = CancelToken::new();
        let opts = RunOptions::default().cancel(&token);
        engine
            .run_with(&mut Message::from_json(&json!({})), &opts)
            .await;
        assert_eq!(reads(), before);

        // The same run with a far deadline reads it at each of them.
        let far = now() + Duration::from_secs(3600);
        let before = reads();
        let opts = RunOptions::default().deadline(far);
        engine
            .run_with(&mut Message::from_json(&json!({})), &opts)
            .await;
        assert!(reads() > before + 3, "{} reads", reads() - before);
    }
}
