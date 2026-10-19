//! `filter`'s config: a gate on the rest of the workflow.

use crate::base::{EVALUATION_ERROR, ErrorKind, Result};
use crate::expr::Expr;
use crate::function::{FunctionConfig, Outcome, SyncCtx};
use ::log::debug;
use serde::{Deserialize, Serialize};

/// `filter`'s input.
#[derive(Debug, FunctionConfig)]
pub(crate) struct FilterConfig {
    /// JSONLogic that lets the message through only when it evaluates to
    /// `true`.
    pub condition: Expr,
    /// What a rejected message does: `halt` (the default) or `skip`.
    #[config(serde, default = "halt")]
    pub on_reject: RejectAction,
}

/// What a [`FilterConfig`] does when its condition is not met.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RejectAction {
    /// Stop the workflow: no later task in it runs.
    #[default]
    Halt,
    /// Skip this task only: the next one runs.
    Skip,
}

impl FilterConfig {
    /// `Ok` when the condition is a literal `true`. Anything else rejects:
    /// `Halt` or `Skip` by `on_reject`. A condition that fails to evaluate
    /// also records `EVALUATION_ERROR`, so the rejection says why.
    ///
    /// An exceeded ops budget is no rejection: it is the call's `Err`, which
    /// the classifier records as `BUDGET_EXCEEDED` and `on_failure` governs,
    /// as `validation`'s (3.x made it a silent rejection).
    pub(crate) fn call<'p>(&'p self, cx: &mut SyncCtx<'_, '_, 'p>) -> Result<Outcome> {
        let met = match cx.test(&self.condition) {
            Ok(met) => met,
            Err(e) if e.kind() == &ErrorKind::Budget => return Err(e),
            Err(e) => {
                debug!("Filter: condition evaluation error: {e:?}");
                let message = format!("Filter condition failed to evaluate: {e}");
                let record = cx.record(EVALUATION_ERROR, message);
                cx.errors.push(record);
                false
            }
        };
        Ok(match (met, self.on_reject) {
            (true, _) => Outcome::Ok,
            (false, RejectAction::Halt) => Outcome::Halt,
            (false, RejectAction::Skip) => Outcome::Skip,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::function::builtins::one_task::{observe, untouched};
    use serde_json::{Value, json};

    /// The filter's status, after checking it wrote nothing, failed nothing
    /// and recorded no error.
    fn filter(condition: Value, on_reject: Option<&str>) -> Option<u16> {
        let (status, errors) = filter_recording(condition, on_reject);
        assert_eq!(errors, "[]", "a condition that evaluates records nothing");
        status
    }

    /// The filter's status and its records, after checking it wrote nothing
    /// and failed nothing.
    fn filter_recording(condition: Value, on_reject: Option<&str>) -> (Option<u16>, String) {
        let mut input = json!({"condition": condition});
        if let Some(action) = on_reject {
            input["on_reject"] = json!(action);
        }
        let context = json!({"data": {"a": 5, "ok": true, "zero": 0}});
        let observed = observe("filter", &input, &Value::Null, &context);
        assert_eq!(
            observed.context,
            untouched(&context),
            "a filter never writes"
        );
        assert_eq!(observed.changes, "[]");
        assert_eq!(observed.error_code, None);
        (observed.status, observed.errors)
    }

    #[test]
    fn a_literal_true_passes() {
        for condition in [
            json!(true),
            json!({"var": "data.ok"}),
            json!({">": [{"var": "data.a"}, 1]}),
        ] {
            assert_eq!(filter(condition, None), Some(200));
        }
    }

    #[test]
    fn anything_else_rejects_as_halt_or_skip() {
        for condition in [
            json!(false),
            json!(1),
            json!("true"),
            json!([1]),
            json!({"var": "data.a"}),
            json!({"var": "data.missing"}),
        ] {
            // `halt` is the default; it is `Halt`, status 299.
            assert_eq!(filter(condition.clone(), None), Some(299), "{condition}");
            assert_eq!(filter(condition.clone(), Some("halt")), Some(299));
            // `skip` leaves no audit entry at all.
            assert_eq!(filter(condition, Some("skip")), None);
        }
    }

    #[test]
    fn an_evaluation_error_rejects_and_records_why() {
        let record = r#"[{"code":"EVALUATION_ERROR","message":"Filter condition failed to evaluate: Thrown: <dependency message>","path":null,"workflow_id":"w","task_id":"t","timestamp":"<timestamp>"}]"#;
        let condition = json!({"/": [1, {"var": "data.zero"}]});
        for (on_reject, status) in [(None, Some(299)), (Some("skip"), None)] {
            assert_eq!(
                filter_recording(condition.clone(), on_reject),
                (status, record.to_owned()),
                "{on_reject:?}"
            );
        }
    }
}
