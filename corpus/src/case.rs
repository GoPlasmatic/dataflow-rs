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

/// Every feature name a case may require: the operator families, `all-operators`
/// (the seven other than `tensor`), and `budget`.
pub const FEATURES: &[&str] = &[
    "datetime",
    "ext-string",
    "ext-array",
    "ext-math",
    "ext-control",
    "ext-object",
    "error-handling",
    "tensor",
    "all-operators",
    "budget",
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
    /// Custom operators to register, by the driver's names (`shout`).
    #[serde(default)]
    pub operators: Vec<String>,
    /// Attach a recording observer and keep its events.
    #[serde(default)]
    pub observer: bool,
    /// Enable target `dataflow::log` up to this level and keep its lines:
    /// `error`, `warn`, `info`, `debug` or `trace`.
    #[serde(default)]
    pub logs: Option<String>,
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
    #[serde(default = "TraceSpec::recording_default")]
    pub trace: TraceSpec,
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

/// What the traced run records. `false` skips the traced run; an object
/// overrides the recording default field by field.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(from = "TraceRepr")]
pub struct TraceSpec {
    pub enabled: bool,
    pub snapshots: bool,
    pub mapping_contexts: bool,
    pub changes: bool,
    pub max_snapshot_bytes: usize,
    pub redact_paths: Vec<String>,
    /// `full`, `own` or `none`.
    pub snapshot_audit_trail: String,
}

impl TraceSpec {
    /// Snapshots, mapping contexts and the per-step changes, each snapshot
    /// carrying only its own audit entry so a recording stays linear in steps.
    pub fn recording_default() -> Self {
        Self {
            enabled: true,
            snapshots: true,
            mapping_contexts: true,
            changes: true,
            max_snapshot_bytes: 0,
            redact_paths: Vec::new(),
            snapshot_audit_trail: "own".to_owned(),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TraceRepr {
    Flag(bool),
    Fields(TraceFields),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TraceFields {
    snapshots: Option<bool>,
    mapping_contexts: Option<bool>,
    changes: Option<bool>,
    max_snapshot_bytes: Option<usize>,
    redact_paths: Option<Vec<String>>,
    snapshot_audit_trail: Option<String>,
}

impl From<TraceRepr> for TraceSpec {
    fn from(repr: TraceRepr) -> Self {
        let d = Self::recording_default();
        match repr {
            TraceRepr::Flag(enabled) => Self { enabled, ..d },
            TraceRepr::Fields(f) => Self {
                enabled: true,
                snapshots: f.snapshots.unwrap_or(d.snapshots),
                mapping_contexts: f.mapping_contexts.unwrap_or(d.mapping_contexts),
                changes: f.changes.unwrap_or(d.changes),
                max_snapshot_bytes: f.max_snapshot_bytes.unwrap_or(d.max_snapshot_bytes),
                redact_paths: f.redact_paths.unwrap_or(d.redact_paths),
                snapshot_audit_trail: f.snapshot_audit_trail.unwrap_or(d.snapshot_audit_trail),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_case_takes_every_default() {
        let case: Case = serde_json::from_str(
            r#"{"description":"d","workflows":[],"steps":[{"run":{"message":{}}}]}"#,
        )
        .unwrap();
        let Step::Run(run) = &case.steps[0] else {
            panic!("a run step")
        };
        assert_eq!(run.message.id, "m1");
        assert_eq!(run.message.payload, Json::object());
        assert_eq!(run.trace, TraceSpec::recording_default());
    }

    #[test]
    fn an_unknown_case_key_is_refused() {
        assert!(
            serde_json::from_str::<Case>(r#"{"description":"d","workflows":[],"cover":[]}"#)
                .is_err()
        );
    }

    #[test]
    fn trace_false_disables_the_traced_run() {
        let run: RunSpec = serde_json::from_str(r#"{"message":{},"trace":false}"#).unwrap();
        assert!(!run.trace.enabled);
    }

    #[test]
    fn a_partial_trace_keeps_the_other_defaults() {
        let run: RunSpec =
            serde_json::from_str(r#"{"trace":{"redact_paths":["data.secret"]}}"#).unwrap();
        assert!(run.trace.enabled && run.trace.changes);
        assert_eq!(run.trace.redact_paths, ["data.secret"]);
    }

    #[test]
    fn audit_modes_read_as_names_or_a_last_object() {
        let modes: Vec<AuditSpec> = serde_json::from_str(r#"["full","off",{"last":2}]"#).unwrap();
        assert_eq!(modes, [AuditSpec::Full, AuditSpec::Off, AuditSpec::Last(2)]);
    }
}
