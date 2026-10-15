//! The options of one run.

use super::message::AuditMode;
use crate::base::{CancelToken, Instant};
use crate::function::CallLayer;
use crate::observe::Observer;
use std::fmt;
use std::sync::Arc;

/// What a caller chooses about one run.
#[derive(Clone)]
#[non_exhaustive]
pub struct RunOptions<'a> {
    /// Run the Active workflows of this channel; `None` runs every Active one.
    pub channel: Option<&'a str>,
    /// The bucket a rollout admits; `None` is admitted by every rollout.
    pub routing_bucket: Option<u8>,
    /// Record each write's `Change`. `true` by default (#63).
    pub capture_changes: bool,
    /// Which audit entries the message keeps. `Full` by default.
    pub audit: AuditMode,
    /// When the run must end; `None` by default. It is checked at every
    /// host call, every fan-out element's start, every sweep and every 64
    /// ops. Natively each host call also races it, on a Tokio timer, so a
    /// run with a deadline is polled inside a Tokio runtime with its time
    /// driver enabled. A call it cuts short is recorded as `TIMEOUT_ERROR`
    /// on its task; either way the run ends [`TimedOut`].
    ///
    /// [`TimedOut`]: crate::observe::RunStatus::TimedOut
    pub deadline: Option<Instant>,
    /// A token that cancels the run when it, or an ancestor, is cancelled;
    /// `None` by default. It is checked and raced where the deadline is,
    /// and first. A call it cuts short is recorded as `CANCELLED` on its
    /// task; either way the run ends [`Cancelled`].
    ///
    /// [`Cancelled`]: crate::observe::RunStatus::Cancelled
    pub cancel: Option<&'a CancelToken>,
    /// Layers around every host call of this run, outside the registry's;
    /// the first is the outermost. None by default.
    pub layers: &'a [Arc<dyn CallLayer>],
    /// An observer of this run, beside the engine's
    /// ([`EngineConfig::observer`]); `None` by default.
    ///
    /// [`EngineConfig::observer`]: crate::run::EngineConfig::observer()
    pub observer: Option<&'a dyn Observer>,
}

impl Default for RunOptions<'_> {
    fn default() -> Self {
        Self {
            channel: None,
            routing_bucket: None,
            capture_changes: true,
            audit: AuditMode::Full,
            deadline: None,
            cancel: None,
            layers: &[],
            observer: None,
        }
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
        f.debug_struct("RunOptions")
            .field("channel", &self.channel)
            .field("routing_bucket", &self.routing_bucket)
            .field("capture_changes", &self.capture_changes)
            .field("audit", &self.audit)
            .field("deadline", &self.deadline)
            .field("cancel", &self.cancel)
            .field("layers", &self.layers.len())
            .field("observer", &format_args!("{observer}"))
            .finish()
    }
}

impl<'a> RunOptions<'a> {
    /// Runs the Active workflows of channel `c`.
    #[must_use]
    pub fn channel(mut self, c: &'a str) -> Self {
        self.channel = Some(c);
        self
    }

    /// Sets the routing bucket rollouts are tested against.
    #[must_use]
    pub fn routing_bucket(mut self, b: u8) -> Self {
        self.routing_bucket = Some(b);
        self
    }

    /// Turns change capture on or off.
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
            r#"RunOptions { channel: Some("c"), routing_bucket: Some(7), capture_changes: false, audit: Off, deadline: None, cancel: Some(CancelToken { cancelled: false, .. }), layers: 1, observer: absent }"#
        );
        let watcher = Quiet;
        let o = o.observer(&watcher);
        assert!(format!("{o:?}").ends_with("layers: 1, observer: present }"));
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
