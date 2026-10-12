//! The rows a case list must cover: every row of the feature contract (02),
//! every break (06) and every error code (04). `design-v4/` is not committed,
//! so the list lives here, in `corpus/contract.json`.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    /// What a case's `covers` names.
    pub id: String,
    /// `"02"`, `"06"` or `"04"`.
    pub source: String,
    /// The row's name in its document.
    pub row: String,
    /// Whether v3 can record it. A row that cannot says why.
    pub v3: bool,
    /// A code that some covering case's recording must contain.
    #[serde(default)]
    pub expect_code: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// `corpus/contract.json`.
pub fn load_contract() -> Vec<Row> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("contract.json");
    let text = std::fs::read_to_string(&path).expect("corpus/contract.json is readable");
    serde_json::from_str(&text).expect("corpus/contract.json parses")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The ids the phase 1 plan lists, so a row cannot drop out of the file
    /// unnoticed.
    const PLANNED: &[&str] = &[
        "schema.priority",
        "schema.status",
        "schema.workflow-fields",
        "schema.unique-ids",
        "schema.task-condition",
        "schema.group-detection",
        "schema.group-condition",
        "schema.group-refusals",
        "schema.group-continue-on-error",
        "loop.bounds",
        "loop.counter",
        "loop.condition",
        "loop.setup",
        "loop.over",
        "loop.over-once",
        "loop.scratch",
        "loop.halt",
        "loop.continue-on-error",
        "fanout.eligibility",
        "fanout.isolation",
        "fanout.bindings",
        "fanout.fold",
        "fanout.over",
        "fanout.concurrency",
        "rollout",
        "schema.pre39-spellings",
        "map.null",
        "map.removal",
        "map.writes",
        "map.mode",
        "map.errors",
        "validation",
        "validation.truthy-non-bool",
        "filter",
        "filter.literal-true",
        "filter.eval-error",
        "log.level",
        "log.fields-order",
        "parse",
        "publish",
        "publish.null-source",
        "integration-configs",
        "expr.eval-context",
        "expr.parameters",
        "expr.templating",
        "expr.dollar-escape",
        "expr.missing-var",
        "expr.var-default",
        "expr.condition-error",
        "expr.operator-families",
        "expr.secret",
        "expr.budget",
        "expr.budget-split",
        "run.wire-shape",
        "run.wire-key-set",
        "run.context-roots",
        "run.capture-changes",
        "run.audit-mode",
        "run.audit-optional-fields",
        "run.metadata",
        "run.error-channels",
        "run.continue-on-error",
        "run.terminal-halt-on",
        "run.sequential",
        "run.priority-order",
        "run.reload",
        "run.error-classifier",
        "break.1",
        "break.2",
        "break.2a",
        "break.3",
        "break.4",
        "break.5",
        "break.6",
        "break.7",
        "break.8",
        "break.9",
        "break.10",
        "break.11",
        "break.12",
        "break.13",
        "code.LOGIC_ERROR",
        "code.BUDGET_EXCEEDED",
        "code.FUNCTION_NOT_FOUND",
        "code.DESERIALIZATION_ERROR",
        "code.TIMEOUT_ERROR",
        "code.TASK_STATUS_ERROR",
        "code.VALIDATION_ERROR",
        "code.EVALUATION_ERROR",
        "code.HTTP_ERROR",
        "code.IO_ERROR",
        "code.UNKNOWN_ERROR",
        "code.FUNCTION_ERROR",
        "code.WORKFLOW_ERROR",
        "code.TASK_ERROR",
        "code.service-verbatim",
        "code.COMPILATION_ERROR",
        "code.CANCELLED",
        "code.CONFIG_ERROR",
    ];

    #[test]
    fn contract_ids_are_unique_and_v3_false_rows_say_why() {
        let rows = load_contract();
        let mut seen = HashSet::new();
        for row in &rows {
            assert!(seen.insert(row.id.as_str()), "{} appears twice", row.id);
            assert!(
                ["02", "06", "04"].contains(&row.source.as_str()),
                "{}: source",
                row.id
            );
            assert!(
                row.v3 || row.reason.is_some(),
                "{} is v3: false without a reason",
                row.id
            );
        }
        for id in PLANNED {
            assert!(seen.contains(id), "{id} is missing from contract.json");
        }
        assert_eq!(
            rows.len(),
            PLANNED.len(),
            "contract.json has rows the plan does not list"
        );
    }
}
