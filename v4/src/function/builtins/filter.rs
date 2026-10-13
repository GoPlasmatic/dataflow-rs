//! `filter`'s config: a gate on the rest of the workflow.

use crate::expr::Expr;
use crate::function::FunctionConfig;
use serde::Deserialize;

/// `filter`'s input.
#[derive(Debug, FunctionConfig)]
#[non_exhaustive]
pub struct FilterConfig {
    /// JSONLogic that lets the message through only when it evaluates to
    /// `true`.
    pub condition: Expr,
    /// What a rejected message does: `halt` (the default) or `skip`.
    #[config(serde, default = "halt")]
    pub on_reject: RejectAction,
}

/// What a [`FilterConfig`] does when its condition is not met.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum RejectAction {
    /// Stop the workflow: no later task in it runs.
    #[default]
    Halt,
    /// Skip this task only: the next one runs.
    Skip,
}
