//! `validation`'s config: rules, each a condition and the error recorded when
//! it fails.

use crate::base::{ErrorRecord, Result};
use crate::expr::Expr;
use crate::function::{FunctionConfig, Outcome, SyncCtx, SyncFunction};
use ::log::{debug, error};
use std::borrow::Cow;

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

impl SyncFunction for ValidationConfig {
    /// Every rule in order, none of them stopping the rest. A rule passes only
    /// on a literal `true`; one that does not records its message, rendered
    /// only then, and one that fails to evaluate records
    /// `EVALUATION_ERROR`. The call is `Status(400)` when any record was
    /// made. The records carry no ids, as v3's do.
    fn call<'p>(&'p self, cx: &mut SyncCtx<'_, '_, 'p>) -> Result<Outcome> {
        let mut records = Vec::new();
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
                    "VALIDATION_ERROR",
                    cx.eval(&rule.message)
                        .map(Cow::into_owned)
                        .unwrap_or_else(|e| {
                            error!("Validation: rule {idx} message failed to render: {e:?}");
                            "Validation failed".to_owned()
                        }),
                ),
                Err(e) => {
                    error!("Validation: Error evaluating rule {idx}: {e:?}");
                    (
                        "EVALUATION_ERROR",
                        format!("Failed to evaluate rule {idx}: {e}"),
                    )
                }
            };
            let mut record = ErrorRecord::new(code, text);
            record.timestamp = Some(cx.now.to_owned());
            records.push(record);
        }
        if records.is_empty() {
            return Ok(Outcome::Ok);
        }
        cx.errors.extend(records);
        Ok(Outcome::Status(400))
    }
}

#[cfg(test)]
mod tests {
    use crate::function::builtins::differential::same;
    use serde_json::{Value, json};

    fn context() -> Value {
        json!({"data": {"a": 5, "ok": true, "name": "x", "zero": 0, "list": [1]}})
    }

    async fn rules(rules: Value) -> crate::function::builtins::differential::Observed {
        same(
            "validation",
            &json!({"rules": rules}),
            &Value::Null,
            &context(),
        )
        .await
    }

    #[tokio::test]
    async fn every_rule_passing_is_ok_and_records_nothing() {
        let observed = rules(json!([
            {"logic": true, "message": "m0"},
            {"logic": {"var": "data.ok"}, "message": "m1"},
            {"logic": {"==": [{"var": "data.a"}, 5]}, "message": "m2"},
        ]))
        .await;
        assert_eq!(observed.status, Some(200));
        assert_eq!(observed.errors, "[]");
    }

    #[tokio::test]
    async fn a_rule_that_is_not_a_literal_true_fails_with_its_message() {
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
            let observed = rules(json!([{"logic": logic, "message": "nope"}])).await;
            assert_eq!(observed.status, Some(400), "{logic}");
            assert!(observed.errors.contains("VALIDATION_ERROR"), "{logic}");
            assert!(observed.errors.contains("nope"), "{logic}");
        }
    }

    #[tokio::test]
    async fn an_evaluation_error_is_evaluation_error_and_the_rest_still_run() {
        let observed = rules(json!([
            {"logic": true, "message": "m0"},
            {"logic": false, "message": "m1"},
            {"logic": {"/": [1, {"var": "data.zero"}]}, "message": "m2"},
            {"logic": false, "message": "m3"},
        ]))
        .await;
        assert_eq!(observed.status, Some(400));
        assert!(observed.errors.contains("Failed to evaluate rule 2: "));
        assert!(observed.errors.contains("EVALUATION_ERROR"));
        assert!(observed.errors.contains("m3"));
    }

    #[tokio::test]
    async fn a_message_is_rendered_only_when_its_rule_fails() {
        let broken = json!({"/": [1, {"var": "data.zero"}]});
        // Passing: the broken message is never evaluated.
        let observed = rules(json!([{"logic": true, "message": broken}])).await;
        assert_eq!(observed.status, Some(200));
        assert_eq!(observed.errors, "[]");
        // Failing: the failure is reported, with v3's stand-in text.
        let observed = rules(json!([{"logic": false, "message": broken}])).await;
        assert!(observed.errors.contains("Validation failed"));
        // A computed message names the value that failed.
        let observed = rules(json!([{
            "logic": false,
            "message": {"cat": ["a was ", {"var": "data.a"}, " and name ", {"var": "data.name"}]}
        }]))
        .await;
        assert!(observed.errors.contains("a was 5 and name x"));
        // A non-string message renders as its JSON, as v3's resolve does.
        rules(json!([{"logic": false, "message": {"var": "data.list"}}])).await;
        rules(json!([{"logic": false, "message": 7}])).await;
    }

    #[tokio::test]
    async fn no_rules_is_ok() {
        assert_eq!(rules(json!([])).await.status, Some(200));
    }
}
