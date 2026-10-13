//! The options of one run.

use super::message::AuditMode;

/// What a caller chooses about one run.
#[derive(Clone, Debug)]
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
}

impl Default for RunOptions<'_> {
    fn default() -> Self {
        Self {
            channel: None,
            routing_bucket: None,
            capture_changes: true,
            audit: AuditMode::Full,
        }
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
        let o = o
            .channel("c")
            .routing_bucket(7)
            .capture_changes(false)
            .audit(AuditMode::Off);
        assert_eq!((o.channel, o.routing_bucket), (Some("c"), Some(7)));
        assert!(!o.capture_changes);
        assert_eq!(o.audit, AuditMode::Off);
    }
}
