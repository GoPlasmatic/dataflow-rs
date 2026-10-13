//! `log`'s config: a log line and its structured fields.

use crate::expr::Expr;
use crate::function::FunctionConfig;
use serde::Deserialize;
use std::collections::BTreeMap;

/// `log`'s input.
#[derive(Debug, FunctionConfig)]
#[non_exhaustive]
pub struct LogConfig {
    /// The level the line is emitted at: `info` by default.
    #[config(serde, default = "info")]
    #[cfg_attr(not(test), expect(dead_code, reason = "read by the phase 4 runtime"))]
    pub level: LogLevel,
    /// JSONLogic computing the line.
    #[config(records)]
    pub message: Expr,
    /// Structured fields, each JSONLogic, emitted in key order.
    #[config(records, default_json = "{}")]
    pub fields: BTreeMap<String, Expr>,
}

/// The level a [`LogConfig`] line is emitted at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum LogLevel {
    /// `trace`.
    Trace,
    /// `debug`.
    Debug,
    /// `info`, the default.
    #[default]
    Info,
    /// `warn`.
    Warn,
    /// `error`.
    Error,
}
