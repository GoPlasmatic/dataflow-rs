//! `validation`'s config: rules, each a condition and the error recorded when
//! it fails.

use crate::base::{EVALUATION_ERROR, ErrorKind, Result, VALIDATION_ERROR};
use crate::expr::Expr;
use crate::function::{FunctionConfig, Outcome, SyncCtx};
use ::log::{debug, error};

/// `validation`'s input.
#[derive(Debug, FunctionConfig)]
pub(crate) struct ValidationConfig {
    /// The rules, checked in order.
    pub rules: Vec<Rule>,
}

/// One rule of a [`ValidationConfig`].
#[derive(Debug, FunctionConfig)]
pub(crate) struct Rule {
    /// JSONLogic that passes the rule only when it evaluates to `true`.
    pub logic: Expr,
    /// The error recorded when the rule fails, evaluated only then.
    #[config(records)]
    pub message: Expr<String>,
}

impl ValidationConfig {
    /// Every rule in order, none of them stopping the rest. A rule passes only
    /// on a literal `true`; one that does not records its message, rendered
    /// only then, and one that fails to evaluate records
    /// `EVALUATION_ERROR`. The call is `Status(400)` when any record was
    /// made. Each record carries the workflow's and the task's ids, as every
    /// other task record does (v3's carried none).
    ///
    /// An exceeded ops budget is no rejection: it is the call's `Err`, which
    /// the classifier records as `BUDGET_EXCEEDED` and `on_failure` governs,
    /// so a runaway rule cannot pass as a 400 (3.x made it one).
    pub(crate) fn call<'p>(&'p self, cx: &mut SyncCtx<'_, '_, 'p>) -> Result<Outcome> {
        let mut passed = true;
        for (idx, rule) in self.rules.iter().enumerate() {
            debug!("Processing validation rule {idx}");
            let (code, text) = match cx.test(&rule.logic) {
                Ok(true) => {
                    debug!("Validation passed for rule {idx}");
                    continue;
                }
                // A message that fails to render still reports the failure:
                // the rule did fail.
                Ok(false) => (
                    VALIDATION_ERROR,
                    match cx.eval(&rule.message) {
                        Ok(text) => text.into_owned(),
                        Err(e) if e.kind() == &ErrorKind::Budget => return Err(e),
                        Err(e) => {
                            error!("Validation: rule {idx} message failed to render: {e:?}");
                            "Validation failed".to_owned()
                        }
                    },
                ),
                Err(e) if e.kind() == &ErrorKind::Budget => return Err(e),
                Err(e) => {
                    error!("Validation: Error evaluating rule {idx}: {e:?}");
                    (
                        EVALUATION_ERROR,
                        format!("Failed to evaluate rule {idx}: {e}"),
                    )
                }
            };
            passed = false;
            let record = cx.record(code, text);
            cx.errors.push(record);
        }
        Ok(if passed {
            Outcome::Ok
        } else {
            Outcome::Status(400)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::function::builtins::one_task::{Golden, Observed};
    use serde_json::{Value, json};

    fn context() -> Value {
        json!({"data": {"a": 5, "ok": true, "name": "x", "zero": 0, "list": [1]}})
    }

    /// `validation` with `rules`, its whole observation held by `golden`.
    fn rules(golden: &mut Golden, rules: Value) -> Observed {
        golden.observe(
            "validation",
            &json!({"rules": rules}),
            &Value::Null,
            &context(),
        )
    }

    #[test]
    fn every_rule_passing_is_ok_and_records_nothing() {
        let mut golden = Golden::new("validation", "every_rule_passing_is_ok_and_records_nothing");
        let observed = rules(
            &mut golden,
            json!([
                {"logic": true, "message": "m0"},
                {"logic": {"var": "data.ok"}, "message": "m1"},
                {"logic": {"==": [{"var": "data.a"}, 5]}, "message": "m2"},
            ]),
        );
        assert_eq!(observed.status, Some(200));
        assert_eq!(observed.errors, "[]");
    }

    #[test]
    fn a_rule_that_is_not_a_literal_true_fails_with_its_message() {
        let mut golden = Golden::new(
            "validation",
            "a_rule_that_is_not_a_literal_true_fails_with_its_message",
        );
        // `false`, and the truthy non-booleans a condition never accepts.
        for logic in [
            json!(false),
            json!(1),
            json!("yes"),
            json!([1]),
            json!({"var": "data.a"}),
            json!({"var": "data.list"}),
            json!({"var": "data.missing"}),
        ] {
            let observed = rules(&mut golden, json!([{"logic": logic, "message": "nope"}]));
            assert_eq!(observed.status, Some(400), "{logic}");
            assert!(observed.errors.contains("VALIDATION_ERROR"), "{logic}");
            assert!(observed.errors.contains("nope"), "{logic}");
        }
    }

    #[test]
    fn an_evaluation_error_is_evaluation_error_and_the_rest_still_run() {
        let mut golden = Golden::new(
            "validation",
            "an_evaluation_error_is_evaluation_error_and_the_rest_still_run",
        );
        let observed = rules(
            &mut golden,
            json!([
                {"logic": true, "message": "m0"},
                {"logic": false, "message": "m1"},
                {"logic": {"/": [1, {"var": "data.zero"}]}, "message": "m2"},
                {"logic": false, "message": "m3"},
            ]),
        );
        assert_eq!(observed.status, Some(400));
        assert!(observed.errors.contains("Failed to evaluate rule 2: "));
        assert!(observed.errors.contains("EVALUATION_ERROR"));
        assert!(observed.errors.contains("m3"));
    }

    #[test]
    fn a_message_is_rendered_only_when_its_rule_fails() {
        let mut golden = Golden::new(
            "validation",
            "a_message_is_rendered_only_when_its_rule_fails",
        );
        let broken = json!({"/": [1, {"var": "data.zero"}]});
        // Passing: the broken message is never evaluated.
        let observed = rules(&mut golden, json!([{"logic": true, "message": broken}]));
        assert_eq!(observed.status, Some(200));
        assert_eq!(observed.errors, "[]");
        // Failing: the failure is reported, with a stand-in text.
        let observed = rules(&mut golden, json!([{"logic": false, "message": broken}]));
        assert!(observed.errors.contains("Validation failed"));
        // A computed message names the value that failed.
        let observed = rules(
            &mut golden,
            json!([{
                "logic": false,
                "message": {"cat": ["a was ", {"var": "data.a"}, " and name ", {"var": "data.name"}]}
            }]),
        );
        assert!(observed.errors.contains("a was 5 and name x"));
        // A non-string message renders as its JSON.
        rules(
            &mut golden,
            json!([{"logic": false, "message": {"var": "data.list"}}]),
        );
        rules(&mut golden, json!([{"logic": false, "message": 7}]));
    }

    #[test]
    fn no_rules_is_ok() {
        let mut golden = Golden::new("validation", "no_rules_is_ok");
        assert_eq!(rules(&mut golden, json!([])).status, Some(200));
    }
}
