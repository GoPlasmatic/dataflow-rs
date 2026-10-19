//! The case format. Every type refuses unknown keys, so a typo in a case fails
//! loudly instead of silently testing less.

use crate::json::Json;
use serde::Deserialize;

/// One case: definitions, an engine, and the steps run against it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    /// One sentence: the behaviour this case pins.
    pub description: String,
    #[serde(default)]
    pub requires: Requires,
    #[serde(default)]
    pub engine: EngineSpec,
    /// Raw definitions, each parsed on its own.
    pub workflows: Vec<Json>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

/// The features a case needs on or off. A case must run in one of CI's two
/// builds, default (nothing on) or all-features (everything on), so `on` and
/// `off` are never both non-empty.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requires {
    #[serde(default)]
    pub on: Vec<String>,
    #[serde(default)]
    pub off: Vec<String>,
}

/// Every cargo feature name a case may require: `datetime`, `tensor`, and
/// `budget`.
pub const FEATURES: &[&str] = &["datetime", "tensor", "budget"];

/// Every operator-family name `engine.families` may select at runtime.
pub const FAMILIES: &[&str] = &[
    "datetime",
    "ext-string",
    "ext-array",
    "ext-math",
    "ext-control",
    "ext-object",
    "error-handling",
    "tensor",
    "all-operators",
];

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineSpec {
    #[serde(default)]
    pub secrets: Option<Json>,
    #[serde(default)]
    pub ops_budget: Option<u64>,
    #[serde(default)]
    pub error_context: Option<ErrorContextSpec>,
    /// Built-in operator families to enable on `ExprEngineBuilder`
    /// ([`FAMILIES`]).
    #[serde(default)]
    pub families: Vec<String>,
    /// Custom operators to register, by the driver's names (`shout`).
    #[serde(default)]
    pub operators: Vec<String>,
    /// Attach a recording observer and keep its events.
    #[serde(default)]
    pub observer: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorContextSpec {
    pub path: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    Run(Box<RunSpec>),
    Reload(ReloadSpec),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReloadSpec {
    pub workflows: Vec<Json>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSpec {
    #[serde(default)]
    pub message: MessageSpec,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub routing_bucket: Option<u8>,
    #[serde(default)]
    pub capture_changes: Option<bool>,
    #[serde(default)]
    pub audit_mode: Option<AuditSpec>,
    /// Whether the step also runs traced, which the driver holds to the
    /// plain run. Default `true`.
    #[serde(default = "yes")]
    pub trace: bool,
    /// This run's normalised message must equal the one step `n` recorded.
    #[serde(default)]
    pub same_as: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageSpec {
    #[serde(default = "default_id")]
    pub id: String,
    #[serde(default = "Json::object")]
    pub payload: Json,
    #[serde(default)]
    pub data: Option<Json>,
    #[serde(default)]
    pub metadata: Option<Json>,
    #[serde(default)]
    pub temp_data: Option<Json>,
}

impl Default for MessageSpec {
    fn default() -> Self {
        Self {
            id: default_id(),
            payload: Json::object(),
            data: None,
            metadata: None,
            temp_data: None,
        }
    }
}

fn default_id() -> String {
    "m1".to_owned()
}

/// `"full"`, `"off"` or `{"last": n}`.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditSpec {
    Full,
    Off,
    Last(usize),
}

fn yes() -> bool {
    true
}

impl Case {
    /// The rules between a case's keys that serde cannot state: every
    /// `requires` name is a [`FEATURES`] entry, `on` and `off` are not both
    /// set, every `engine.families` name is a [`FAMILIES`] entry, an
    /// `ops_budget` requires `budget`, and a `same_as` names an earlier run
    /// step.
    pub fn validate(&self) -> Result<(), String> {
        let names = self.requires.on.iter().chain(&self.requires.off);
        if let Some(name) = names.into_iter().find(|n| !FEATURES.contains(&n.as_str())) {
            return Err(format!("requires names `{name}`, which is no feature"));
        }
        if !self.requires.on.is_empty() && !self.requires.off.is_empty() {
            return Err("requires has features both on and off: no CI build has both".to_owned());
        }
        if let Some(family) = self
            .engine
            .families
            .iter()
            .find(|f| !FAMILIES.contains(&f.as_str()))
        {
            return Err(format!(
                "engine.families names `{family}`, which is no family"
            ));
        }
        if self.engine.ops_budget.is_some() && !self.requires.on.iter().any(|n| n == "budget") {
            return Err("engine.ops_budget without requires.on budget runs unbounded".to_owned());
        }
        for (i, step) in self.steps.iter().enumerate() {
            let Step::Run(run) = step else { continue };
            if let Some(n) = run.same_as
                && !(n < i && matches!(self.steps[n], Step::Run(_)))
            {
                return Err(format!(
                    "step {i}'s same_as names {n}, which is no earlier run step"
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(text: &str) -> Case {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn a_minimal_case_takes_every_default() {
        let case = case(r#"{"description":"d","workflows":[],"steps":[{"run":{"message":{}}}]}"#);
        let Step::Run(run) = &case.steps[0] else {
            panic!("a run step")
        };
        assert_eq!(run.message.id, "m1");
        assert_eq!(run.message.payload, Json::object());
        assert!(run.trace, "a step runs traced too by default");
        assert_eq!(case.validate(), Ok(()));
    }

    #[test]
    fn an_unknown_case_key_is_refused() {
        assert!(
            serde_json::from_str::<Case>(r#"{"description":"d","workflows":[],"cover":[]}"#)
                .is_err()
        );
        // `logs` and the object form of `trace` are gone.
        assert!(serde_json::from_str::<EngineSpec>(r#"{"logs":"warn"}"#).is_err());
        assert!(serde_json::from_str::<RunSpec>(r#"{"trace":{"changes":true}}"#).is_err());
    }

    #[test]
    fn trace_false_disables_the_traced_run() {
        let run: RunSpec = serde_json::from_str(r#"{"message":{},"trace":false}"#).unwrap();
        assert!(!run.trace);
    }

    #[test]
    fn validate_refuses_what_serde_cannot() {
        let refused = |text: &str| case(text).validate().unwrap_err();
        assert!(
            refused(r#"{"description":"d","workflows":[],"requires":{"on":["fast"]}}"#)
                .contains("`fast`")
        );
        assert!(
            refused(r#"{"description":"d","workflows":[],"engine":{"families":["flagd"]}}"#)
                .contains("`flagd`")
        );
        assert!(refused(
            r#"{"description":"d","workflows":[],"requires":{"on":["budget"],"off":["tensor"]}}"#
        )
        .contains("both"));
        assert!(
            refused(r#"{"description":"d","workflows":[],"engine":{"ops_budget":5}}"#)
                .contains("budget")
        );
        assert!(
            refused(r#"{"description":"d","workflows":[],"steps":[{"run":{"same_as":0}}]}"#)
                .contains("same_as")
        );
        assert!(refused(
            r#"{"description":"d","workflows":[],"steps":[{"reload":{"workflows":[]}},{"run":{"same_as":0}}]}"#
        )
        .contains("same_as"));
    }

    #[test]
    fn audit_modes_read_as_names_or_a_last_object() {
        let modes: Vec<AuditSpec> = serde_json::from_str(r#"["full","off",{"last":2}]"#).unwrap();
        assert_eq!(modes, [AuditSpec::Full, AuditSpec::Off, AuditSpec::Last(2)]);
    }
}
