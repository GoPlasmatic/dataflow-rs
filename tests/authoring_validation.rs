//! `Workflow::validate_authored` — authoring-time checking of a definition,
//! before it reaches `Engine::build()`.
//!
//! The API's promise is a biconditional: it returns empty **iff** the JSON
//! parses and validates. Two things have to be tested, and only one of them is
//! obvious.
//!
//! The obvious one is the biconditional itself. The subtle one is that the
//! biconditional is guaranteed by the *last* stage — an actual parse — so the
//! structural walk that produces good multi-violation reporting could quietly
//! become a no-op and every biconditional test would still pass. So each broken
//! fixture also asserts its **specific** semantic code, which fails if the walk
//! stops doing its job and the catch-all takes over.

use dataflow_rs::{IssueCode, Workflow};
use serde_json::{Value, json};

fn codes(json: &Value) -> Vec<IssueCode> {
    Workflow::validate_authored(json)
        .iter()
        .map(|i| i.code)
        .collect()
}

fn loads(json: &Value) -> bool {
    Workflow::from_json(&json.to_string()).is_ok_and(|w| w.validate().is_ok())
}

fn task(id: &str) -> Value {
    json!({"id": id, "name": id, "function": {"name": "map", "input": {"mappings": []}}})
}

fn workflow(tasks: Value) -> Value {
    json!({"id": "w", "name": "w", "priority": 0, "tasks": tasks})
}

/// Every fixture: what it is, and the code it must produce.
fn broken_fixtures() -> Vec<(&'static str, IssueCode, Value)> {
    vec![
        (
            "empty workflow id",
            IssueCode::EmptyWorkflowId,
            json!({"id": "", "name": "w", "tasks": [task("t")]}),
        ),
        (
            "missing workflow name",
            IssueCode::EmptyWorkflowName,
            json!({"id": "w", "tasks": [task("t")]}),
        ),
        (
            "empty tasks array",
            IssueCode::NoTasks,
            json!({"id": "w", "name": "w", "tasks": []}),
        ),
        (
            "tasks is not an array",
            IssueCode::NoTasks,
            json!({"id": "w", "name": "w", "tasks": "nope"}),
        ),
        (
            "step with no id",
            IssueCode::MissingStepId,
            workflow(
                json!([{"name": "t", "function": {"name": "map", "input": {"mappings": []}}}]),
            ),
        ),
        (
            "duplicate task ids",
            IssueCode::DuplicateStepId,
            workflow(json!([task("dup"), task("dup")])),
        ),
        (
            "group id colliding with a task id",
            IssueCode::DuplicateStepId,
            workflow(json!([task("shared"), {"id": "shared", "tasks": [task("inner")]}])),
        ),
        (
            "empty group",
            IssueCode::EmptyGroup,
            workflow(json!([{"id": "g", "tasks": []}])),
        ),
        (
            "task with no function",
            IssueCode::MissingFunction,
            workflow(json!([{"id": "t", "name": "t"}])),
        ),
        (
            "function with an empty name",
            IssueCode::InvalidFunctionName,
            workflow(json!([{"id": "t", "name": "t", "function": {"name": ""}}])),
        ),
        (
            "terminal is not a boolean",
            IssueCode::InvalidTerminal,
            workflow(json!([{"id": "t", "name": "t", "terminal": "yes",
                            "function": {"name": "map", "input": {"mappings": []}}}])),
        ),
        (
            "loop increment below one",
            IssueCode::LoopIncrementTooSmall,
            json!({"id": "w", "name": "w", "loop": {"max": 3, "increment": 0},
                   "tasks": [task("t")]}),
        ),
        (
            "loop max not above init",
            IssueCode::LoopBoundEmpty,
            json!({"id": "w", "name": "w", "loop": {"init": 5, "max": 5}, "tasks": [task("t")]}),
        ),
        (
            "loop counter with an empty segment",
            IssueCode::LoopCounterInvalid,
            json!({"id": "w", "name": "w", "loop": {"max": 3, "counter": "a..b"},
                   "tasks": [task("t")]}),
        ),
    ]
}

/// The load-bearing test. Each fixture must report *its own* code — not merely
/// be rejected. Without this, the structural walk could return nothing and the
/// stage-2 parse would still make every biconditional assertion pass.
#[test]
fn every_broken_fixture_reports_its_own_code() {
    for (label, expected, json) in broken_fixtures() {
        let found = codes(&json);
        assert!(
            found.contains(&expected),
            "{label}: expected {expected:?}, got {found:?} — \
             if this is ParseFailed, the structural walk stopped doing its job"
        );
        assert!(
            !found.contains(&IssueCode::ParseFailed),
            "{label}: fell through to the catch-all instead of being diagnosed"
        );
    }
}

#[test]
fn empty_iff_the_workflow_loads() {
    let valid = vec![
        workflow(json!([task("a")])),
        workflow(json!([task("a"), {"id": "g", "condition": true, "tasks": [task("b")]}])),
        json!({"id": "w", "name": "w", "loop": {"max": 3, "counter": "i"},
               "tasks": [task("t")]}),
    ];
    for json in valid {
        assert!(
            Workflow::validate_authored(&json).is_empty(),
            "loadable workflow reported issues: {:?}",
            Workflow::validate_authored(&json)
        );
        assert!(loads(&json), "fixture claimed valid but does not load");
    }

    for (label, _, json) in broken_fixtures() {
        assert!(
            !Workflow::validate_authored(&json).is_empty(),
            "{label}: reported no issues"
        );
        assert!(!loads(&json), "{label}: claimed broken but loads fine");
    }
}

/// The six cases from the design: they break no semantic rule, so only the
/// parse stage can catch them. This is what the biconditional rests on.
#[test]
fn a_type_error_falls_through_to_parse_failed() {
    let cases = vec![
        (
            "map with no mappings",
            workflow(json!([{"id": "t", "name": "t", "function": {"name": "map", "input": {}}}])),
        ),
        (
            "priority as a string",
            json!({"id": "w", "name": "w", "priority": "high", "tasks": [task("t")]}),
        ),
        (
            "continue_on_error as an integer",
            workflow(json!([{"id": "t", "name": "t", "continue_on_error": 3,
                          "function": {"name": "map", "input": {"mappings": []}}}])),
        ),
        (
            "misspelled status",
            json!({"id": "w", "name": "w", "status": "enabled", "tasks": [task("t")]}),
        ),
        (
            "http_call with no connector",
            workflow(
                json!([{"id": "t", "name": "t", "function": {"name": "http_call", "input": {}}}]),
            ),
        ),
        (
            "loop max as a string",
            json!({"id": "w", "name": "w", "loop": {"max": "3"}, "tasks": [task("t")]}),
        ),
    ];

    for (label, json) in cases {
        let issues = Workflow::validate_authored(&json);
        assert_eq!(
            issues.iter().map(|i| i.code).collect::<Vec<_>>(),
            vec![IssueCode::ParseFailed],
            "{label}: should be caught by the parse stage alone"
        );
        assert!(
            !issues[0].message.is_empty(),
            "{label}: carries the parser's own message"
        );
        assert!(!loads(&json), "{label}: should not load");
    }
}

#[test]
fn all_violations_are_reported_not_just_the_first() {
    let json = json!({
        "id": "", "name": "",
        "tasks": [task("dup"), task("dup"), {"id": "g", "tasks": []}]
    });
    let found = codes(&json);

    for expected in [
        IssueCode::EmptyWorkflowId,
        IssueCode::EmptyWorkflowName,
        IssueCode::DuplicateStepId,
        IssueCode::EmptyGroup,
    ] {
        assert!(
            found.contains(&expected),
            "missing {expected:?} in {found:?}"
        );
    }
    assert!(
        found.len() >= 4,
        "expected four distinct problems, got {found:?}"
    );
}

#[test]
fn violations_carry_authored_coordinates_not_flat_indices() {
    // The duplicate is the second member of a group. A flattened view would
    // call it tasks[2]; the author wrote tasks[1].tasks[1].
    let json = workflow(json!([
        task("first"),
        {"id": "g", "condition": true, "tasks": [task("inner"), task("first")]}
    ]));

    let issues = Workflow::validate_authored(&json);
    let dup = issues
        .iter()
        .find(|i| i.code == IssueCode::DuplicateStepId)
        .expect("the collision is reported");

    assert_eq!(dup.path.as_deref(), Some("tasks[1].tasks[1].id"));
    assert_eq!(dup.task_id.as_deref(), Some("first"));
    assert!(
        dup.message.contains("tasks[0]"),
        "the message names where the id was first used, got: {}",
        dup.message
    );
}

#[test]
fn a_group_past_the_depth_cap_is_reported_at_the_parsers_boundary() {
    let depth = dataflow_rs::MAX_GROUP_DEPTH;

    let mut node = task("innermost");
    for level in (0..=depth).rev() {
        node = json!({"id": format!("g{level}"), "condition": true, "tasks": [node]});
    }
    let json = workflow(json!([node]));

    assert!(codes(&json).contains(&IssueCode::GroupTooDeep));
    assert!(!loads(&json), "the parser rejects it at the same boundary");

    // Exactly at the cap is fine, on both sides.
    let mut node = task("innermost");
    for level in (0..depth).rev() {
        node = json!({"id": format!("g{level}"), "condition": true, "tasks": [node]});
    }
    let ok = workflow(json!([node]));
    assert!(Workflow::validate_authored(&ok).is_empty());
    assert!(loads(&ok));
}

#[test]
fn a_non_object_input_does_not_panic() {
    for input in [Value::Null, json!([]), json!("workflow"), json!(7)] {
        let issues = Workflow::validate_authored(&input);
        assert!(!issues.is_empty(), "{input} is not a workflow");
        assert!(!loads(&input));
    }
}

#[test]
fn issue_codes_have_distinct_stable_strings() {
    let all = [
        IssueCode::EmptyWorkflowId,
        IssueCode::EmptyWorkflowName,
        IssueCode::NoTasks,
        IssueCode::MissingStepId,
        IssueCode::DuplicateStepId,
        IssueCode::EmptyGroup,
        IssueCode::GroupTooDeep,
        IssueCode::MissingFunction,
        IssueCode::InvalidFunctionName,
        IssueCode::InvalidTerminal,
        IssueCode::LoopIncrementTooSmall,
        IssueCode::LoopBoundEmpty,
        IssueCode::LoopCounterInvalid,
        IssueCode::ParseFailed,
        IssueCode::ValidateFailed,
    ];
    let mut seen = std::collections::HashSet::new();
    for code in all {
        assert!(seen.insert(code.as_str()), "duplicate string for {code:?}");
        assert_eq!(code.to_string(), code.as_str(), "Display matches as_str");
        assert!(
            code.as_str()
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '_'),
            "{code:?} is not SCREAMING_SNAKE"
        );
    }
}

#[test]
fn the_validate_failed_backstop_stays_unreached() {
    // If this ever fires, `check_shape` has stopped modelling a rule that
    // `Workflow::validate` enforces — the caller still gets a correct answer,
    // but a semantic code with a path was owed and not delivered.
    for (label, _, json) in broken_fixtures() {
        assert!(
            !codes(&json).contains(&IssueCode::ValidateFailed),
            "{label}: reached the backstop instead of being diagnosed structurally"
        );
    }
}
