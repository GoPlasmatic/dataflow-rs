//! `filter`'s config: a gate on the rest of the workflow.

use crate::base::Result;
use crate::expr::Expr;
use crate::function::{FunctionConfig, Outcome, SyncCtx, SyncFunction};
use ::log::debug;
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

impl SyncFunction for FilterConfig {
    /// `Ok` when the condition is a literal `true`. Anything else, including
    /// an evaluation error, rejects: `Halt` or `Skip` by `on_reject`.
    fn call<'p>(&'p self, cx: &mut SyncCtx<'_, '_, 'p>) -> Result<Outcome> {
        let met = cx.test(&self.condition).unwrap_or_else(|e| {
            debug!("Filter: condition evaluation error: {e:?}");
            false
        });
        Ok(match (met, self.on_reject) {
            (true, _) => Outcome::Ok,
            (false, RejectAction::Halt) => Outcome::Halt,
            (false, RejectAction::Skip) => Outcome::Skip,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::function::builtins::differential::same;
    use serde_json::{Value, json};

    async fn filter(condition: Value, on_reject: Option<&str>) -> Option<u16> {
        let mut input = json!({"condition": condition});
        if let Some(action) = on_reject {
            input["on_reject"] = json!(action);
        }
        let context = json!({"data": {"a": 5, "ok": true, "zero": 0}});
        let observed = same("filter", &input, &Value::Null, &context).await;
        assert_eq!(observed.errors, "[]", "a filter never records an error");
        observed.status
    }

    #[tokio::test]
    async fn a_literal_true_passes() {
        for condition in [
            json!(true),
            json!({"var": "data.ok"}),
            json!({">": [{"var": "data.a"}, 1]}),
        ] {
            assert_eq!(filter(condition, None).await, Some(200));
        }
    }

    #[tokio::test]
    async fn anything_else_rejects_as_halt_or_skip() {
        for condition in [
            json!(false),
            json!(1),
            json!("true"),
            json!([1]),
            json!({"var": "data.a"}),
            json!({"var": "data.missing"}),
            // An evaluation error rejects too.
            json!({"/": [1, {"var": "data.zero"}]}),
        ] {
            // `halt` is the default; it is `Halt`, status 299.
            assert_eq!(
                filter(condition.clone(), None).await,
                Some(299),
                "{condition}"
            );
            assert_eq!(filter(condition.clone(), Some("halt")).await, Some(299));
            // `skip` leaves no audit entry at all.
            assert_eq!(filter(condition, Some("skip")).await, None);
        }
    }
}
