//! `validation`'s config: rules, each a condition and the error recorded when
//! it fails.

use crate::expr::Expr;
use crate::function::FunctionConfig;

/// `validation`'s input.
#[derive(Debug, FunctionConfig)]
#[non_exhaustive]
pub struct ValidationConfig {
    /// The rules, checked in order.
    pub rules: Vec<Rule>,
}

/// One rule of a [`ValidationConfig`].
#[derive(Debug, FunctionConfig)]
#[non_exhaustive]
pub struct Rule {
    /// JSONLogic that passes the rule only when it evaluates to `true`.
    pub logic: Expr,
    /// The error recorded when the rule fails, evaluated only then.
    #[config(records)]
    pub message: Expr<String>,
}
