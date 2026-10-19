//! `log`'s config: a log line and its structured fields.

use crate::base::Result;
use crate::expr::Expr;
use crate::function::{FunctionConfig, Outcome, SyncCtx};
use datavalue::OwnedDataValue;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `log`'s input.
#[derive(Debug, FunctionConfig)]
pub(crate) struct LogConfig {
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LogLevel {
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

/// One expression's text onto `line`: a string as it is, anything else as
/// compact JSON, and `<eval error>` when it fails.
fn stringify<'p>(cx: &mut SyncCtx<'_, '_, 'p>, expr: &'p Expr, line: &mut String) {
    match cx.eval(expr) {
        Ok(value) => match &*value {
            OwnedDataValue::String(s) => line.push_str(s),
            other => line.push_str(&other.to_json_string()),
        },
        Err(e) => {
            ::log::error!("Log: Failed to evaluate expression: {e:?}");
            line.push_str("<eval error>");
        }
    }
}

impl LogConfig {
    /// Emits the line, `message [key=value, ...]`, at the level. A level the
    /// logger filters out evaluates nothing, and the call is `Ok` either way.
    pub(crate) fn call<'p>(&'p self, cx: &mut SyncCtx<'_, '_, 'p>) -> Result<Outcome> {
        let level = self.level.level();
        if !::log::log_enabled!(target: TARGET, level) {
            return Ok(Outcome::Ok);
        }
        let mut line = String::new();
        stringify(cx, &self.message, &mut line);
        for (i, (key, expr)) in self.fields.iter().enumerate() {
            line.push_str(if i == 0 { " [" } else { ", " });
            line.push_str(key);
            line.push('=');
            stringify(cx, expr, &mut line);
        }
        if !self.fields.is_empty() {
            line.push(']');
        }
        ::log::log!(target: TARGET, level, "{line}");
        Ok(Outcome::Ok)
    }
}

#[cfg(test)]
mod tests {
    use crate::function::builtins::one_task::logs::logged;
    use crate::function::builtins::one_task::{Observed, untouched};
    use ::log::Level;
    use serde_json::{Value, json};

    fn context() -> Value {
        json!({"data": {"a": 5, "b": 1, "c": "three", "list": [1, "x"], "f": 1.5, "zero": 0}})
    }

    /// A `log` task writes nothing, records nothing and is `Ok`, whatever it
    /// logged and whether or not its expressions evaluated.
    fn assert_quiet(observed: &Observed) {
        assert_eq!(
            observed,
            &Observed {
                context: untouched(&context()),
                changes: "[]".to_owned(),
                errors: "[]".to_owned(),
                status: Some(200),
                error_code: None,
            }
        );
    }

    #[test]
    fn a_task_below_the_loggers_level_evaluates_nothing_and_is_ok() {
        // The message would fail; at `info` under a `warn` logger it is
        // never evaluated, so nothing is logged and nothing fails.
        let input = json!({"level": "info", "message": {"/": [1, {"var": "data.zero"}]}});
        let (observed, lines) = logged(Level::Warn, &input, &context());
        assert_quiet(&observed);
        assert!(lines.is_empty(), "{lines:?}");
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
        let (observed, lines) = logged(Level::Warn, &input, &context());
        assert_quiet(&observed);
        assert_eq!(lines, ["WARN: hello [a=5, b=1, c=three]"]);
    }

    #[test]
    fn values_that_are_not_strings_are_written_as_json() {
        let input = json!({
            "level": "error",
            "message": {"var": "data.list"},
            "fields": {"f": {"var": "data.f"}, "n": {"var": "data.missing"}, "t": true}
        });
        let (observed, lines) = logged(Level::Trace, &input, &context());
        assert_quiet(&observed);
        assert_eq!(lines, [r#"ERROR: [1,"x"] [f=1.5, n=null, t=true]"#]);
    }

    #[test]
    fn a_failing_message_or_field_is_written_as_eval_error() {
        let broken = json!({"/": [1, {"var": "data.zero"}]});
        let input = json!({"level": "warn", "message": broken, "fields": {"x": broken}});
        let (observed, lines) = logged(Level::Warn, &input, &context());
        assert_quiet(&observed);
        assert_eq!(
            lines,
            [
                "ERROR: Log: Failed to evaluate expression",
                "ERROR: Log: Failed to evaluate expression",
                "WARN: <eval error> [x=<eval error>]"
            ]
        );
    }

    #[test]
    fn a_task_logs_exactly_when_its_level_is_enabled() {
        let levels = [
            Level::Error,
            Level::Warn,
            Level::Info,
            Level::Debug,
            Level::Trace,
        ];
        for task in levels {
            let name = task.as_str().to_lowercase();
            for logger in levels {
                let input = json!({"level": name, "message": {"cat": ["m ", {"var": "data.a"}]}});
                let (observed, lines) = logged(logger, &input, &context());
                assert_quiet(&observed);
                let want: Vec<String> = if task <= logger {
                    vec![format!("{task}: m 5")]
                } else {
                    Vec::new()
                };
                assert_eq!(lines, want, "task {name} under {logger}");
            }
        }
        // The default level is `info`.
        let (observed, lines) = logged(Level::Info, &json!({"message": "m"}), &context());
        assert_quiet(&observed);
        assert_eq!(lines, ["INFO: m"]);
    }
}
