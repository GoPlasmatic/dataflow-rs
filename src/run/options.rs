//! The options of one run.

use super::budget::{ConditionErrorPolicy, RunAccountant};
use super::message::AuditMode;
use crate::base::{CancelToken, Instant};
use crate::function::CallLayer;
use crate::observe::{Observer, TraceOptions};
use std::fmt;
use std::sync::Arc;

/// What a caller chooses about one run.
#[derive(Clone)]
#[non_exhaustive]
pub struct RunOptions<'a> {
    /// Run the Active workflows of this channel; `None` runs every Active one.
    pub channel: Option<&'a str>,
    /// The bucket a rollout admits, `0..=99`; a larger one is served as 99.
    /// `None` is admitted by every rollout.
    pub routing_bucket: Option<u8>,
    /// Record each write's `Change` in its task's audit entry. `true` by
    /// default (#63). It governs only the audit trail: a trace records its
    /// own changes whatever it says.
    pub capture_changes: bool,
    /// Which audit entries the message keeps. `Full` by default.
    pub audit: AuditMode,
    /// When the run must end; `None` by default. It is checked at every
    /// host call, every fan-out element's start, every sweep and every
    /// [`CHECK_EVERY`](crate::run::CHECK_EVERY) ops. Natively each host call also races it, on a Tokio timer, so a
    /// run with a deadline is polled inside a Tokio runtime with its time
    /// driver enabled. A call it cuts short, or that returns `Err` once it
    /// has passed, is recorded as `TIMEOUT_ERROR` on its task; either way
    /// the run ends [`TimedOut`].
    ///
    /// On wasm no timer exists, so nothing races a host call and it is
    /// never cut short mid-flight: a deadline that passes while a call runs
    /// is seen at the next check, after the call returns.
    ///
    /// [`TimedOut`]: crate::run::RunStatus::TimedOut
    pub deadline: Option<Instant>,
    /// A token that cancels the run when it, or an ancestor, is cancelled;
    /// `None` by default. It is checked and raced where the deadline is,
    /// and first. A call it cuts short, or that returns `Err` once it is
    /// cancelled, is recorded as `CANCELLED` on its task; either way the
    /// run ends [`Cancelled`].
    ///
    /// [`Cancelled`]: crate::run::RunStatus::Cancelled
    pub cancel: Option<&'a CancelToken>,
    /// Layers around every host call of this run, outside the registry's;
    /// the first is the outermost. None by default.
    pub layers: &'a [Arc<dyn CallLayer>],
    /// An observer of this run, beside the engine's
    /// ([`EngineConfig::observer`]); `None` by default.
    ///
    /// [`EngineConfig::observer`]: crate::run::EngineConfig::observer()
    pub observer: Option<&'a dyn Observer>,
    /// Trace the run, as these options say: its [`Run::trace`] is then the
    /// initial context plus each step's changes. `None` by default: no
    /// trace, and no copy of the context.
    ///
    /// [`Run::trace`]: crate::Run::trace()
    pub trace: Option<TraceOptions>,
    /// How condition evaluation errors are handled during this run; `None`
    /// inherits the engine's [`EngineConfig::condition_error_policy`](super::EngineConfig::condition_error_policy)
    /// (which defaults to [`ConditionErrorPolicy::Skip`]).
    pub condition_error_policy: Option<ConditionErrorPolicy>,
    /// Maximum number of task dispatches (inline built-ins, host calls, loop
    /// sweeps, and `for_each` elements) permitted during this run; `None` by
    /// default.
    pub task_budget: Option<u64>,
    /// Maximum cumulative datalogic operations permitted across all
    /// expressions evaluated during this run; `None` by default.
    pub expr_ops_budget: Option<u64>,
    /// Host-owned accounting and admission hook for this run; `None` by
    /// default.
    pub accountant: Option<&'a dyn RunAccountant>,
}

impl RunOptions<'static> {
    /// The defaults, as a constant: `Engine::run` borrows a static copy.
    pub(crate) const DEFAULT: Self = Self {
        channel: None,
        routing_bucket: None,
        capture_changes: true,
        audit: AuditMode::Full,
        deadline: None,
        cancel: None,
        layers: &[],
        observer: None,
        trace: None,
        condition_error_policy: None,
        task_budget: None,
        expr_ops_budget: None,
        accountant: None,
    };
}

impl Default for RunOptions<'_> {
    fn default() -> Self {
        RunOptions::DEFAULT
    }
}

impl fmt::Debug for RunOptions<'_> {
    /// Every option, the layers as a count and the observer as a presence.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let observer = if self.observer.is_some() {
            "present"
        } else {
            "absent"
        };
        let mut s = f.debug_struct("RunOptions");
        s.field("channel", &self.channel)
            .field("routing_bucket", &self.routing_bucket)
            .field("capture_changes", &self.capture_changes)
            .field("audit", &self.audit)
            .field("deadline", &self.deadline)
            .field("cancel", &self.cancel)
            .field("layers", &self.layers.len())
            .field("observer", &format_args!("{observer}"))
            .field("trace", &self.trace);
        if let Some(policy) = &self.condition_error_policy {
            s.field("condition_error_policy", policy);
        }
        if let Some(budget) = &self.task_budget {
            s.field("task_budget", budget);
        }
        if let Some(budget) = &self.expr_ops_budget {
            s.field("expr_ops_budget", budget);
        }
        if self.accountant.is_some() {
            s.field("accountant", &format_args!("present"));
        }
        s.finish()
    }
}

impl<'a> RunOptions<'a> {
    /// Runs the Active workflows of channel `c`.
    #[must_use]
    pub fn channel(mut self, c: &'a str) -> Self {
        self.channel = Some(c);
        self
    }

    /// Sets the routing bucket rollouts are tested against: `0..=99`. A
    /// larger one is served as bucket 99, as in 3.x, so derive it as a
    /// hash `% 100`, never `% 256`.
    #[must_use]
    pub fn routing_bucket(mut self, b: u8) -> Self {
        self.routing_bucket = Some(b);
        self
    }

    /// Turns the audit trail's change capture on or off.
    #[must_use]
    pub fn capture_changes(mut self, on: bool) -> Self {
        self.capture_changes = on;
        self
    }

    /// Sets which audit entries the message keeps.
    #[must_use]
    pub fn audit(mut self, mode: AuditMode) -> Self {
        self.audit = mode;
        self
    }

    /// Ends the run at `deadline`, as the `deadline` field says.
    #[must_use]
    pub fn deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Cancels the run when `token` is cancelled, as the `cancel` field
    /// says.
    #[must_use]
    pub fn cancel(mut self, token: &'a CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// Wraps every host call of the run in `layers`, outside the registry's
    /// layers; the first is the outermost.
    #[must_use]
    pub fn layers(mut self, layers: &'a [Arc<dyn CallLayer>]) -> Self {
        self.layers = layers;
        self
    }

    /// Reports this run to `o` too, beside the engine's observer.
    #[must_use]
    pub fn observer(mut self, o: &'a dyn Observer) -> Self {
        self.observer = Some(o);
        self
    }

    /// Traces the run under `opts`, as the `trace` field says.
    #[must_use]
    pub fn trace(mut self, opts: TraceOptions) -> Self {
        self.trace = Some(opts);
        self
    }

    /// Sets how condition evaluation errors are handled during this run.
    #[must_use]
    pub fn condition_error_policy(mut self, policy: ConditionErrorPolicy) -> Self {
        self.condition_error_policy = Some(policy);
        self
    }

    /// Alias for [`Self::condition_error_policy`].
    #[must_use]
    pub fn with_condition_error_policy(self, policy: ConditionErrorPolicy) -> Self {
        self.condition_error_policy(policy)
    }

    /// Sets the maximum number of task dispatches permitted during this run.
    #[must_use]
    pub fn task_budget(mut self, max_tasks: u64) -> Self {
        self.task_budget = Some(max_tasks);
        self
    }

    /// Alias for [`Self::task_budget`].
    #[must_use]
    pub fn with_task_budget(self, max_tasks: u64) -> Self {
        self.task_budget(max_tasks)
    }

    /// Sets the maximum cumulative datalogic operations permitted across all
    /// expressions evaluated during this run.
    #[must_use]
    pub fn expr_ops_budget(mut self, max_ops: u64) -> Self {
        self.expr_ops_budget = Some(max_ops);
        self
    }

    /// Alias for [`Self::expr_ops_budget`].
    #[must_use]
    pub fn with_expr_ops_budget(self, max_ops: u64) -> Self {
        self.expr_ops_budget(max_ops)
    }

    /// Attaches a host-owned [`RunAccountant`] to this run.
    #[must_use]
    pub fn accountant(mut self, accountant: &'a dyn RunAccountant) -> Self {
        self.accountant = Some(accountant);
        self
    }

    /// Alias for [`Self::accountant`].
    #[must_use]
    pub fn with_accountant(self, accountant: &'a dyn RunAccountant) -> Self {
        self.accountant(accountant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_options_default_captures_changes_and_keeps_the_full_audit() {
        let o = RunOptions::default();
        assert!(o.capture_changes);
        assert_eq!(o.audit, AuditMode::Full);
        assert_eq!(o.channel, None);
        assert_eq!(o.routing_bucket, None);
        assert!(o.deadline.is_none() && o.cancel.is_none());
        assert!(o.layers.is_empty());
        assert!(o.observer.is_none());
        assert!(o.trace.is_none());
        let layers: Vec<Arc<dyn CallLayer>> = vec![Arc::new(Pass)];
        let token = CancelToken::new();
        let o = o
            .channel("c")
            .routing_bucket(7)
            .capture_changes(false)
            .audit(AuditMode::Off)
            .cancel(&token)
            .layers(&layers);
        assert_eq!((o.channel, o.routing_bucket), (Some("c"), Some(7)));
        assert!(!o.capture_changes);
        assert_eq!(o.audit, AuditMode::Off);
        assert_eq!(o.layers.len(), 1);
        assert_eq!(
            format!("{o:?}"),
            r#"RunOptions { channel: Some("c"), routing_bucket: Some(7), capture_changes: false, audit: Off, deadline: None, cancel: Some(CancelToken { cancelled: false, .. }), layers: 1, observer: absent, trace: None }"#
        );
        let watcher = Quiet;
        let o = o.observer(&watcher);
        assert!(format!("{o:?}").ends_with("layers: 1, observer: present, trace: None }"));
        let o = o.trace(TraceOptions::default().timings(true));
        assert!(o.trace.as_ref().is_some_and(|t| t.timings && t.changes));
        let at = Instant::now();
        assert_eq!(o.deadline(at).deadline, Some(at));
    }

    /// An observer that keeps nothing.
    struct Quiet;

    impl Observer for Quiet {
        fn on(&self, _event: &crate::observe::Event<'_>) {}
    }

    /// A layer that goes straight on.
    struct Pass;

    impl CallLayer for Pass {
        fn call<'a>(
            &'a self,
            ctx: &'a mut crate::function::Ctx<'_>,
            next: crate::function::Next<'a>,
        ) -> crate::function::BoxFuture<'a, crate::base::Result<crate::function::Outcome>> {
            next.run(ctx)
        }
    }
}
