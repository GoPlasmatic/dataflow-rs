//! What the driver holds v4 to beyond v3's recording: which v4 issues are
//! compared against it, what v4 reports for each definition a deviation
//! covers, and what v4's run records at each path a deviation covers.

use crate::Row;

/// v3's 34 issue codes, in v3's order: the first 34 of v4's `IssueCode`.
pub const V3_CODES: [&str; 34] = [
    "EMPTY_WORKFLOW_ID",
    "EMPTY_WORKFLOW_NAME",
    "NO_TASKS",
    "MISSING_STEP_ID",
    "DUPLICATE_STEP_ID",
    "EMPTY_GROUP",
    "GROUP_TOO_DEEP",
    "MISSING_FUNCTION",
    "INVALID_FUNCTION_NAME",
    "INVALID_TERMINAL",
    "UNGUARDED_VALIDATION",
    "INVALID_HALT_ON",
    "GROUP_CONTINUE_ON_ERROR",
    "INVALID_MAPPING",
    "NULL_MAPPING",
    "LOOP_INCREMENT_TOO_SMALL",
    "LOOP_BOUND_EMPTY",
    "LOOP_COUNTER_INVALID",
    "LOOP_SLOT_INVALID",
    "LOOP_ITEM_WITHOUT_OVER",
    "LOOP_SLOT_COLLISION",
    "LOOP_OVER_INVALID",
    "INVALID_FOR_EACH",
    "UNKNOWN_FUNCTION",
    "MISSING_HANDLER",
    "INPUT_PARSE",
    "TEMPLATE_COMPILE",
    "UNKNOWN_SECRET",
    "SECRET_IN_MESSAGE_WRITE",
    "INVALID_SECRET_STORE",
    "DUPLICATE_TEMPLATE_KEY",
    "ESCAPED_TEMPLATE_KEY",
    "PARSE_FAILED",
    "VALIDATE_FAILED",
];

/// Whether a v4 row of a definition no deviation covers is compared against
/// v3's recording: every row of one of v3's codes, and every Rejected or
/// Defect row of one of v4's own. v4's own Advisories are not, since v3 had
/// nothing to record them with.
pub fn compared(row: &Row) -> bool {
    V3_CODES.contains(&row.code.as_str()) || matches!(row.severity.as_str(), "REJECTED" | "DEFECT")
}

/// One issue v4 reports: (code, severity, path).
pub type Reported = (&'static str, &'static str, &'static str);

/// One deviated definition: (case, definition index, the breaks that allow
/// it, and every issue v4 reports for it).
pub type Deviated = (&'static str, usize, &'static str, &'static [Reported]);

/// Every definition a deviation covers, and everything v4 reports for it,
/// unfiltered, each issue in the order v4 reports them.
///
/// `breaks/unknown_keys` and `builtins/pre39_spellings#1` are what `parse`
/// reports, as phase 2's tests pin it: the typos in
/// `v4/tests/repository_definitions.rs`, the UI's `path` in `v4/tests/parse.rs`,
/// and both spellings at once in `v4/tests/compat.rs`.
pub const DEVIATED: &[Deviated] = &[
    ("breaks/name_required", 0, "6", &[]),
    ("breaks/name_required", 1, "6", &[]),
    (
        "breaks/path_outside_roots",
        0,
        "15",
        &[(
            "INVALID_PATH",
            "REJECTED",
            "tasks[0].function.input.mappings[0].path",
        )],
    ),
    (
        "breaks/unknown_keys",
        0,
        "2",
        &[
            ("LIKELY_TYPO", "DEFECT", "conditon"),
            ("UNKNOWN_KEY", "ADVISORY", "path"),
        ],
    ),
    (
        "breaks/unknown_keys",
        1,
        "2",
        &[("LIKELY_TYPO", "DEFECT", "tasks[0].conditon")],
    ),
    (
        "breaks/unknown_keys",
        2,
        "2",
        &[("LIKELY_TYPO", "DEFECT", "loop.maxx")],
    ),
    (
        "breaks/unknown_keys",
        3,
        "2",
        &[("LIKELY_TYPO", "DEFECT", "tasks[0].for_each.ass")],
    ),
    (
        "breaks/unknown_keys",
        4,
        "2",
        &[("LIKELY_TYPO", "DEFECT", "rollout.bucket_strat")],
    ),
    (
        "breaks/unknown_keys",
        5,
        "2",
        &[("LIKELY_TYPO", "DEFECT", "tasks[0].function.input.targte")],
    ),
    (
        "builtins/integration_configs",
        0,
        "3, 10",
        &[
            ("V3_SPELLING", "ADVISORY", "continue_on_error"),
            ("V3_SPELLING", "ADVISORY", "tasks[0].continue_on_error"),
            ("V3_SPELLING", "ADVISORY", "tasks[1].continue_on_error"),
            ("V3_SPELLING", "ADVISORY", "tasks[2].continue_on_error"),
            ("UNKNOWN_FUNCTION", "REJECTED", "tasks[0].function.name"),
            ("UNKNOWN_FUNCTION", "REJECTED", "tasks[1].function.name"),
            ("UNKNOWN_FUNCTION", "REJECTED", "tasks[2].function.name"),
        ],
    ),
    (
        "builtins/integration_configs",
        1,
        "3, 10",
        &[("UNKNOWN_FUNCTION", "REJECTED", "tasks[0].function.name")],
    ),
    (
        "builtins/pre39_spellings",
        0,
        "8, 3",
        &[
            ("V3_SPELLING", "ADVISORY", "continue_on_error"),
            ("V3_SPELLING", "ADVISORY", "tasks[0].continue_on_error"),
            (
                "V3_SPELLING",
                "ADVISORY",
                "tasks[0].function.input.body_logic",
            ),
            ("V3_SPELLING", "ADVISORY", "tasks[0].function.input.output"),
            (
                "V3_SPELLING",
                "ADVISORY",
                "tasks[0].function.input.path_logic",
            ),
            ("V3_SPELLING", "ADVISORY", "tasks[1].continue_on_error"),
            (
                "V3_SPELLING",
                "ADVISORY",
                "tasks[1].function.input.path_logic",
            ),
            ("V3_SPELLING", "ADVISORY", "tasks[2].continue_on_error"),
            (
                "V3_SPELLING",
                "ADVISORY",
                "tasks[2].function.input.key_logic",
            ),
            (
                "V3_SPELLING",
                "ADVISORY",
                "tasks[2].function.input.value_logic",
            ),
            ("UNKNOWN_FUNCTION", "REJECTED", "tasks[0].function.name"),
            ("UNKNOWN_FUNCTION", "REJECTED", "tasks[1].function.name"),
            ("UNKNOWN_FUNCTION", "REJECTED", "tasks[2].function.name"),
        ],
    ),
    (
        "builtins/pre39_spellings",
        1,
        "8",
        &[(
            "PARSE_FAILED",
            "REJECTED",
            "tasks[0].function.input.path_logic",
        )],
    ),
    (
        "builtins/validate_alias",
        0,
        "8",
        &[("V3_SPELLING", "ADVISORY", "tasks[0].function.name")],
    ),
    (
        "builtins/validation",
        0,
        "8",
        &[("UNGUARDED_VALIDATION", "ADVISORY", "tasks[0].on_rejection")],
    ),
    (
        "expressions/dollar_escape",
        0,
        "14",
        &[
            (
                "ESCAPED_TEMPLATE_KEY",
                "ADVISORY",
                "tasks[0].function.input.mappings[0].logic.$cat",
            ),
            (
                "ESCAPED_TEMPLATE_KEY",
                "ADVISORY",
                "tasks[0].function.input.mappings[4].logic.$oid",
            ),
        ],
    ),
    (
        "expressions/host_config_stray_key",
        0,
        "2a",
        &[("UNKNOWN_KEY", "REJECTED", "tasks[0].function.input.colour")],
    ),
    (
        "expressions/payload_not_in_context",
        0,
        "9",
        &[(
            "PAYLOAD_VAR",
            "DEFECT",
            "tasks[0].function.input.mappings[0].logic",
        )],
    ),
];

/// One deviated run path: (case, JSON pointer into the run record, v4's JSON
/// there).
pub type RunDeviated = (&'static str, &'static str, &'static str);

/// What v4 records at every deviated path of a run step (I69), as JSON
/// (whitespace is not compared). A deviation path with `*` is expanded to
/// every pointer it names in v3's recording or in v4's record, and each needs
/// its row here. A pointer v4's record does not reach is written
/// [`MISSING`]. A path under the trace needs no row until phase 6 compares
/// traces.
///
/// - `breaks/name_required` (break 6): both unnamed definitions parse, so
///   all four workflows run, in definition order, and each appends.
/// - `loop/over_shapes` (break 4, I61): the non-array `over` is one record
///   with `LOGIC_ERROR` and the loop's own message, where v3 wrapped a
///   `WORKFLOW_ERROR`.
/// - `schema/status_without_channel` (break 7): only the Active workflow
///   runs without a channel.
pub const RUN_DEVIATED: &[RunDeviated] = &[
    (
        "breaks/name_required",
        "/steps/0/run/message/context/data/order",
        r#"["no_workflow_name", "no_task_name", "no_group_name", "named"]"#,
    ),
    (
        "breaks/name_required",
        "/steps/0/run/message/audit_trail",
        r#"[
            {"workflow_id": "no_workflow_name", "task_id": "t", "timestamp": "<timestamp>",
             "changes": [{"path": "data.order.0", "old_value": null, "new_value": "no_workflow_name"}],
             "status": 200},
            {"workflow_id": "no_task_name", "task_id": "t", "timestamp": "<timestamp>",
             "changes": [{"path": "data.order.1", "old_value": null, "new_value": "no_task_name"}],
             "status": 200},
            {"workflow_id": "no_group_name", "task_id": "t", "timestamp": "<timestamp>",
             "changes": [{"path": "data.order.2", "old_value": null, "new_value": "no_group_name"}],
             "status": 200},
            {"workflow_id": "named", "task_id": "t", "timestamp": "<timestamp>",
             "changes": [{"path": "data.order.3", "old_value": null, "new_value": "named"}],
             "status": 200}
        ]"#,
    ),
    (
        "loop/over_shapes",
        "/steps/0/run/message/errors/0/code",
        r#""LOGIC_ERROR""#,
    ),
    (
        "loop/over_shapes",
        "/steps/0/run/message/errors/0/message",
        r#""Workflow not_array: loop.over must evaluate to an array, got a string""#,
    ),
    (
        "schema/status_without_channel",
        "/steps/0/run/message/context/data/order",
        r#"["active"]"#,
    ),
    (
        "schema/status_without_channel",
        "/steps/0/run/message/context/metadata/progress",
        r#"{"workflow_id": "active", "task_id": "t", "status_code": 200}"#,
    ),
    (
        "schema/status_without_channel",
        "/steps/0/run/message/audit_trail",
        r#"[
            {"workflow_id": "active", "task_id": "t", "timestamp": "<timestamp>",
             "changes": [{"path": "data.order.0", "old_value": null, "new_value": "active"}],
             "status": 200}
        ]"#,
    ),
];

/// The JSON of a [`RUN_DEVIATED`] row whose pointer v4's record does not
/// reach.
pub const MISSING: &str = "<missing>";

#[cfg(test)]
mod tests {
    use super::*;
    use dataflow_rs::program::IssueCode;

    /// `IssueCode`'s variants, in declaration order. The enum is
    /// `#[non_exhaustive]` and v4 exposes no list of its codes, so this reads
    /// the declaration itself.
    fn declared_variants() -> Vec<&'static str> {
        let source = include_str!("../../../v4/src/base/issue.rs");
        let body = source
            .split_once("pub enum IssueCode {")
            .and_then(|(_, rest)| rest.split_once("\n}"))
            .map(|(body, _)| body)
            .expect("v4 declares `pub enum IssueCode`");
        body.lines()
            .filter_map(|line| line.trim().strip_suffix(','))
            .filter(|name| {
                name.starts_with(|c: char| c.is_ascii_uppercase())
                    && name.chars().all(|c| c.is_ascii_alphanumeric())
            })
            .collect()
    }

    #[test]
    fn v3_codes_are_v4s_first_34_in_order() {
        let declared = declared_variants();
        assert!(declared.len() > V3_CODES.len(), "{declared:?}");
        let parsed: Vec<String> = V3_CODES
            .iter()
            .map(|code| {
                let parsed: IssueCode = code
                    .parse()
                    .unwrap_or_else(|e| panic!("{code} is not a v4 code: {e}"));
                assert_eq!(parsed.as_str(), *code);
                format!("{parsed:?}")
            })
            .collect();
        assert_eq!(parsed, declared[..V3_CODES.len()]);
    }
}
