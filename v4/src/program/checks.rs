//! The checks visitor: one hook the compile walk calls at every expression
//! site, with the expression's absolute path, its raw JSON, whether the
//! engine records its result, and the step it sits in.
//!
//! The rules (secrets, template keys, `payload` vars) arrive in Task 6, and
//! the fingerprint's classification in Task 7.

use crate::base::{AuthoredPath, Issue, StepKind};
use crate::expr::{ExprEngine, ExprVisitor};
use serde_json::Value;

/// The step an expression sits in: its kind and id. `None` for the
/// workflow's own expressions.
pub(crate) type Stamp<'s> = Option<(StepKind, &'s str)>;

/// Where an expression sits.
#[derive(Clone, Copy)]
#[cfg_attr(not(test), expect(dead_code, reason = "read by the rules from Task 6"))]
pub(crate) struct Site<'s> {
    /// The expression's absolute authored path.
    pub(crate) at: &'s AuthoredPath,
    /// Whether the engine records the expression's result.
    pub(crate) records: bool,
    /// The step it sits in.
    pub(crate) step: Stamp<'s>,
}

/// What the compile walk calls at every expression site: [`Checks`] in a
/// compile. Issues go to `issues`, in the order they are found.
pub(crate) trait Hook {
    /// One expression, at `site`, compiled from `raw`.
    fn expr(&mut self, site: Site<'_>, raw: &Value, issues: &mut Vec<Issue>);
}

/// The compile checks over one workflow or fragment.
pub(crate) struct Checks<'e> {
    /// The vocabulary and secrets the rules read.
    #[expect(dead_code, reason = "read by the rules from Task 6")]
    exprs: &'e ExprEngine,
}

impl<'e> Checks<'e> {
    /// The checks against `exprs`.
    pub(crate) fn new(exprs: &'e ExprEngine) -> Self {
        Self { exprs }
    }
}

impl Hook for Checks<'_> {
    fn expr(&mut self, _site: Site<'_>, _raw: &Value, _issues: &mut Vec<Issue>) {}
}

/// A config's expressions, forwarded to a [`Hook`]. A config's visit reports
/// paths relative to the config; this joins each onto `root`, the task's
/// `function.input`.
pub(crate) struct Below<'a> {
    pub(crate) hook: &'a mut dyn Hook,
    pub(crate) issues: &'a mut Vec<Issue>,
    pub(crate) root: &'a AuthoredPath,
    pub(crate) step: Stamp<'a>,
}

impl ExprVisitor for Below<'_> {
    fn expr(&mut self, at: &AuthoredPath, raw: &Value, records: bool) {
        let at = self.root.extend(at);
        let site = Site {
            at: &at,
            records,
            step: self.step,
        };
        self.hook.expr(site, raw, self.issues);
    }
}
