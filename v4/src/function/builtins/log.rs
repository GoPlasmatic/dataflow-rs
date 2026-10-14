//! `log`'s config: a log line and its structured fields.

use crate::base::Result;
use crate::expr::Expr;
use crate::function::{FunctionConfig, Outcome, SyncCtx, SyncFunction};
use datavalue::OwnedDataValue;
use serde::Deserialize;
use std::collections::BTreeMap;

/// `log`'s input.
#[derive(Debug, FunctionConfig)]
#[non_exhaustive]
pub struct LogConfig {
    /// The level the line is emitted at: `info` by default.
    #[config(serde, default = "info")]
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

/// The target `log` lines are emitted under, as v3's.
const TARGET: &str = "dataflow::log";

impl LogLevel {
    /// The `log` crate's level.
    fn level(self) -> ::log::Level {
        match self {
            Self::Trace => ::log::Level::Trace,
            Self::Debug => ::log::Level::Debug,
            Self::Info => ::log::Level::Info,
            Self::Warn => ::log::Level::Warn,
            Self::Error => ::log::Level::Error,
        }
    }
}

/// One expression's text: a string as it is, anything else as compact JSON,
/// and `<eval error>` when it fails.
fn stringify<'p>(cx: &mut SyncCtx<'_, '_, 'p>, expr: &'p Expr) -> String {
    match cx.eval(expr) {
        Ok(value) => match &*value {
            OwnedDataValue::String(s) => s.clone(),
            other => other.to_json_string(),
        },
        Err(e) => {
            ::log::error!("Log: Failed to evaluate expression: {e:?}");
            "<eval error>".to_owned()
        }
    }
}

impl SyncFunction for LogConfig {
    /// Emits the line, `message [key=value, ...]`, at the level. A level the
    /// logger filters out evaluates nothing, and the call is `Ok` either way.
    fn call<'p>(&'p self, cx: &mut SyncCtx<'_, '_, 'p>) -> Result<Outcome> {
        let level = self.level.level();
        if !::log::log_enabled!(target: TARGET, level) {
            return Ok(Outcome::Ok);
        }
        let mut line = stringify(cx, &self.message);
        if !self.fields.is_empty() {
            let parts: Vec<String> = self
                .fields
                .iter()
                .map(|(key, expr)| format!("{key}={}", stringify(cx, expr)))
                .collect();
            line = format!("{line} [{}]", parts.join(", "));
        }
        ::log::log!(target: TARGET, level, "{line}");
        Ok(Outcome::Ok)
    }
}

#[cfg(test)]
mod tests {
    use crate::function::builtins::differential::logs::both;
    use ::log::Level;
    use serde_json::json;

    fn context() -> serde_json::Value {
        json!({"data": {"a": 5, "b": 1, "c": "three", "list": [1, "x"], "f": 1.5, "zero": 0}})
    }

    #[test]
    fn a_task_below_the_loggers_level_evaluates_nothing_and_is_ok() {
        // The message would fail; at `info` under a `warn` logger it is
        // never evaluated, so nothing is logged and nothing fails.
        let input = json!({"level": "info", "message": {"/": [1, {"var": "data.zero"}]}});
        let (v3, v4) = both(Level::Warn, &input, &context());
        assert_eq!(v4, v3);
        assert_eq!(v4.0.status, Some(200));
        assert!(v4.1.is_empty(), "{:?}", v4.1);
    }

    #[test]
    fn a_task_at_the_loggers_level_emits_the_message_and_its_fields_in_key_order() {
        let input = json!({
            "level": "warn",
            "message": "hello",
            "fields": {
                "c": {"var": "data.c"},
                "a": {"var": "data.a"},
                "b": {"var": "data.b"},
            }
        });
        let (v3, v4) = both(Level::Warn, &input, &context());
        assert_eq!(v4, v3);
        assert_eq!(v4.1, ["WARN: hello [a=5, b=1, c=three]"]);
    }

    #[test]
    fn values_that_are_not_strings_are_written_as_json() {
        let input = json!({
            "level": "error",
            "message": {"var": "data.list"},
            "fields": {"f": {"var": "data.f"}, "n": {"var": "data.missing"}, "t": true}
        });
        let (v3, v4) = both(Level::Trace, &input, &context());
        assert_eq!(v4, v3);
        assert_eq!(v4.1.len(), 1);
    }

    #[test]
    fn a_failing_message_or_field_is_written_as_eval_error() {
        let broken = json!({"/": [1, {"var": "data.zero"}]});
        let input = json!({"level": "warn", "message": broken, "fields": {"x": broken}});
        let (v3, v4) = both(Level::Warn, &input, &context());
        assert_eq!(v4, v3);
        assert_eq!(
            v4.1,
            [
                "ERROR: Log: Failed to evaluate expression",
                "ERROR: Log: Failed to evaluate expression",
                "WARN: <eval error> [x=<eval error>]"
            ]
        );
        assert_eq!(v4.0.status, Some(200));
    }

    #[test]
    fn every_level_agrees_with_v3_under_every_logger_level() {
        for task in ["trace", "debug", "info", "warn", "error"] {
            for logger in [
                Level::Error,
                Level::Warn,
                Level::Info,
                Level::Debug,
                Level::Trace,
            ] {
                let input = json!({"level": task, "message": {"cat": ["m ", {"var": "data.a"}]}});
                let (v3, v4) = both(logger, &input, &context());
                assert_eq!(v4, v3, "task {task} under {logger}");
            }
        }
        // The default level is `info`.
        let (v3, v4) = both(Level::Info, &json!({"message": "m"}), &context());
        assert_eq!(v4, v3);
        assert_eq!(v4.1, ["INFO: m"]);
    }
}
