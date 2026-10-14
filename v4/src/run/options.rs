//! The options of one run.

use super::message::AuditMode;
use crate::function::CallLayer;
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
    /// Layers around every host call of this run, outside the registry's;
    /// the first is the outermost. None by default.
    pub layers: &'a [Arc<dyn CallLayer>],
}

impl Default for RunOptions<'_> {
    fn default() -> Self {
        Self {
            channel: None,
            routing_bucket: None,
            capture_changes: true,
            audit: AuditMode::Full,
            layers: &[],
        }
    }
}

impl fmt::Debug for RunOptions<'_> {
    /// Every option, the layers as a count.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunOptions")
            .field("channel", &self.channel)
            .field("routing_bucket", &self.routing_bucket)
            .field("capture_changes", &self.capture_changes)
            .field("audit", &self.audit)
            .field("layers", &self.layers.len())
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

    /// Wraps every host call of the run in `layers`, outside the registry's
    /// layers; the first is the outermost.
    #[must_use]
    pub fn layers(mut self, layers: &'a [Arc<dyn CallLayer>]) -> Self {
        self.layers = layers;
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
        assert!(o.layers.is_empty());
        let layers: Vec<Arc<dyn CallLayer>> = vec![Arc::new(Pass)];
        let o = o
            .channel("c")
            .routing_bucket(7)
            .capture_changes(false)
            .audit(AuditMode::Off)
            .layers(&layers);
        assert_eq!((o.channel, o.routing_bucket), (Some("c"), Some(7)));
        assert!(!o.capture_changes);
        assert_eq!(o.audit, AuditMode::Off);
        assert_eq!(o.layers.len(), 1);
        assert_eq!(
            format!("{o:?}"),
            r#"RunOptions { channel: Some("c"), routing_bucket: Some(7), capture_changes: false, audit: Off, layers: 1 }"#
        );
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
