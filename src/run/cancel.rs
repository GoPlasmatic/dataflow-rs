//! The run's limits: its deadline and its cancellation token, and the
//! one rule that checks them.
//!
//! The interpreter checks them at every host call and every fan-out
//! element's start, at every sweep, and every
//! [`CHECK_EVERY`](crate::run::CHECK_EVERY) ops, and only when one is
//! set: a run with neither never reads the clock. The token is checked
//! first. A stop between ops or at a sweep records nothing on the message;
//! the [`Run`](super::Run) says why. Natively a host call also [`race`]s
//! them, one timer per call; on wasm only the checks run. A host call that
//! returns `Err` is checked again once it has, natively and on wasm alike:
//! a limit that has passed by then is why it failed.

use crate::base::{CancelToken, Error, ErrorKind, Instant};
use crate::observe::status::RunStatus;

/// The run's deadline and token, as its [`RunOptions`](super::RunOptions)
/// set them.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Limits<'r> {
    /// When the run must end.
    pub(crate) deadline: Option<Instant>,
    /// The token that cancels the run.
    pub(crate) cancel: Option<&'r CancelToken>,
    /// Whether this run's runtime can build a timer: probed once at the
    /// run's start, so every call of the run races the same way.
    pub(crate) timers: bool,
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
    /// The run's status when this stop ends it.
    pub(crate) fn status(self) -> RunStatus {
        match self {
            Self::Cancelled => RunStatus::Cancelled,
            Self::TimedOut => RunStatus::TimedOut,
        }
    }

    /// The error a call to `function` this stop cut short ends with, recorded
    /// on its task.
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

    /// `Err(Stop)` when the token tripped or the deadline passed.
    pub(crate) fn check(&self) -> std::result::Result<(), Stop> {
        self.stop().map_or(Ok(()), Err)
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
/// A call that wins with an `Err` is still the limit's: its caller checks
/// the limits again.
///
/// A timer needs a Tokio runtime with its time driver enabled. Without one
/// (`futures::executor::block_on`, smol, a runtime built without
/// `enable_time`), the deadline falls back to the checks around each op and
/// call, the mode wasm always uses, and warns once. A token still races,
/// since it needs no timer.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn race<F: Future>(
    call: F,
    limits: Limits<'_>,
) -> std::result::Result<F::Output, Stop> {
    let timer = match limits.deadline {
        Some(deadline) if limits.timers => Some(deadline),
        Some(_) => {
            timers::warn_once();
            None
        }
        None => None,
    };
    // A deadline with no timer and no token has nothing to race: the
    // checks around the call are the whole limit.
    if timer.is_none() && limits.cancel.is_none() {
        return Ok(call.await);
    }
    let cancelled = async {
        match limits.cancel {
            Some(token) => token.cancelled().await,
            None => std::future::pending().await,
        }
    };
    let timer = async {
        match timer {
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

/// Whether a timer can be built on this thread.
#[cfg(not(target_arch = "wasm32"))]
mod timers {
    use std::cell::Cell;

    thread_local! {
        /// This thread's answer, once probed. A thread does not change the
        /// runtime context it polls in, so the answer is stable there; a
        /// process-wide cache would be wrong when a host runs one runtime
        /// with timers and another without.
        static AVAILABLE: Cell<Option<bool>> = const { Cell::new(None) };
    }

    /// Whether `tokio::time::sleep_until` can be built here: a Tokio runtime
    /// is current and its time driver is enabled.
    ///
    /// Tokio exposes neither question publicly. A `Sleep` fetches the driver
    /// in its constructor, panicking when it is missing, so the probe builds
    /// one and catches that panic. A missing runtime is cheaper to see and
    /// panics nothing.
    pub(super) fn available() -> bool {
        AVAILABLE.with(|cell| match cell.get() {
            Some(known) => known,
            None => {
                let known = probe();
                cell.set(Some(known));
                known
            }
        })
    }

    fn probe() -> bool {
        if tokio::runtime::Handle::try_current().is_err() {
            return false;
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drop(tokio::time::sleep_until(tokio::time::Instant::now()));
        }))
        .is_ok()
    }

    /// Says once that a deadline is checked, not raced.
    pub(super) fn warn_once() {
        static WARNED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        WARNED.get_or_init(|| {
            ::log::warn!(
                "a run deadline cannot use a timer: no Tokio runtime with \
                 `enable_time` is current. Limits are checked between ops and \
                 calls instead, and a call is not cut short mid-flight."
            );
        });
    }
}

/// Whether a run started on this thread can race a deadline with a timer.
/// Probed once at the run's start; whatever it answers holds for the run.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn timers_available() -> bool {
    timers::available()
}

/// On wasm a run never races a deadline.
#[cfg(target_arch = "wasm32")]
pub(crate) fn timers_available() -> bool {
    false
}

/// The time a deadline is compared with, on the run's shared clock
/// ([`super::record::now`]).
fn now() -> Instant {
    super::record::now()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::RunOptions;
    use crate::run::record::clock_reads as reads;
    use crate::{Engine, Message};
    use serde_json::json;
    use std::time::Duration;

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
            timers: false,
        };
        assert!(passed.is_set());
        assert_eq!(passed.stop(), Some(Stop::TimedOut));
        token.cancel();
        assert_eq!(passed.check(), Err(Stop::Cancelled));
        let e = Stop::Cancelled.during("f");
        assert_eq!(
            (e.code(), e.message()),
            ("CANCELLED", "the run was cancelled during `f`")
        );
        assert!(!e.is_retryable());

        let far = Limits {
            deadline: Some(now() + Duration::from_secs(3600)),
            cancel: None,
            timers: false,
        };
        assert!(far.is_set() && far.check().is_ok());
        let past = Limits {
            deadline: Some(now()),
            cancel: None,
            timers: false,
        };
        assert_eq!(past.check(), Err(Stop::TimedOut));
        let e = Stop::TimedOut.during("f");
        assert_eq!(
            (e.code(), e.message()),
            ("TIMEOUT_ERROR", "the run's deadline passed during `f`")
        );
        assert!(e.is_retryable());
    }

    /// Returns `Ok`, or an `Err` of its own.
    struct Answer(bool);

    impl crate::Function for Answer {
        type Config = ();

        async fn call(&self, _: &mut crate::Ctx<'_>, _: &()) -> crate::Result<crate::Outcome> {
            if self.0 {
                Ok(crate::Outcome::Ok)
            } else {
                Err(Error::service("BOOM", "it failed"))
            }
        }
    }

    /// How many times one run of a single `answer` call reads the clock.
    async fn reads_of(ok: bool, opts: &RunOptions<'_>) -> u32 {
        let engine = Engine::builder()
            .register("answer", Answer(ok))
            .workflow_json(&json!({"id": "w", "tasks": [{"id": "a",
                "on_failure": "continue", "function": {"name": "answer", "input": {}}}]}))
            .build()
            .unwrap();
        let before = reads();
        engine
            .run_with(&mut Message::from_json(&json!({})), opts)
            .await;
        reads() - before
    }

    #[tokio::test]
    async fn only_a_host_calls_err_checks_the_limits_after_it() {
        // An `Err` reads the clock once more than an `Ok`, after the call,
        // to see whether the deadline had passed by then.
        let far = now() + Duration::from_secs(3600);
        let opts = RunOptions::default().deadline(far);
        let ok = reads_of(true, &opts).await;
        assert!(ok >= 1, "the check before the call reads it");
        assert_eq!(reads_of(false, &opts).await, ok + 1);

        // Neither reads it with no deadline, a token alone included.
        let token = CancelToken::new();
        for opts in [RunOptions::default(), RunOptions::default().cancel(&token)] {
            assert_eq!(reads_of(true, &opts).await, 0);
            assert_eq!(reads_of(false, &opts).await, 0);
        }
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

    #[test]
    fn a_deadline_without_a_timer_falls_back_to_the_checks() {
        // `futures::executor::block_on` has no Tokio runtime, so there is no
        // time driver to race: the checks between ops and calls are the whole
        // limit, and the call is not cut short mid-flight. Before DX-2 the
        // first host call panicked here.
        let engine = Engine::builder()
            .register("answer", Answer(true))
            .workflow_json(&json!({"id": "w", "tasks": [{"id": "a",
                "function": {"name": "answer", "input": {}}}]}))
            .build()
            .unwrap();
        let opts = RunOptions::default().deadline(now() + Duration::from_secs(3600));
        let run = futures::executor::block_on(
            engine.run_with(&mut Message::from_json(&json!({})), &opts),
        );
        assert!(
            matches!(run.status(), crate::run::RunStatus::Completed),
            "{:?}",
            run.status()
        );
    }
}
