extern crate dataflow_rs_v4 as dataflow_rs;

// `def::parse`: one walk that builds a `WorkflowDef` and is also the linter.
// Each test asserts the full list of `(code, severity, path)` a definition
// gets, in order: the order is the rules', which is v3's.

use dataflow_rs::def::{
    BUILTIN_FUNCTIONS, FailurePolicy, ForEachDef, GroupDef, LoopDef, MAX_GROUP_DEPTH, ParseOptions,
    Parsed, RejectionPolicy, Rollout, Status, StepDef, TaskDef, WorkflowDef, WorkflowFailurePolicy,
    parse,
};
use dataflow_rs::program::{IssueCode as C, Severity, StepKind};
use serde_json::{Value, json};

const R: Severity = Severity::Rejected;
const D: Severity = Severity::Defect;
const A: Severity = Severity::Advisory;

type Row = (C, Severity, String);

fn rows(parsed: &Parsed) -> Vec<Row> {
    parsed
        .issues
        .iter()
        .map(|i| (i.code, i.severity, i.path.to_string()))
        .collect()
}

fn row(code: C, severity: Severity, path: &str) -> Row {
    (code, severity, path.to_string())
}

/// Parsed with the defaults: the compat reader on.
fn compat(json: &Value) -> Parsed {
    parse(json, ParseOptions::default())
}

/// Parsed with the compat reader off.
fn strict(json: &Value) -> Parsed {
    parse(json, ParseOptions::default().compat(false))
}

/// A task running a custom function.
fn leaf(id: &str) -> Value {
    json!({"id": id, "function": {"name": "f"}})
}

/// A workflow `w` holding `tasks`.
fn wf(tasks: Value) -> Value {
    json!({"id": "w", "tasks": tasks})
}

/// A workflow whose one task carries `for_each` over a custom function.
fn fan_out(for_each: Value) -> Value {
    wf(json!([{"id": "t", "for_each": for_each, "function": {"name": "f"}}]))
}

/// `n` groups nested one inside the next, the innermost holding a task;
/// `deepest` is merged into the innermost group.
fn nested(n: usize, deepest: Value) -> Value {
    let mut node = leaf("innermost");
    for level in (0..n).rev() {
        let mut group = json!({"id": format!("g{level}"), "tasks": [node]});
        if level == n - 1
            && let (Some(group), Some(extra)) = (group.as_object_mut(), deepest.as_object())
        {
            group.extend(extra.clone());
        }
        node = group;
    }
    json!([node])
}

/// The path of the group at `depth` in [`nested`]: `tasks[0]` then
/// `.tasks[0]` per level.
fn nested_path(depth: usize) -> String {
    let mut path = "tasks[0]".to_string();
    for _ in 0..depth {
        path.push_str(".tasks[0]");
    }
    path
}

fn built(parsed: Parsed) -> WorkflowDef {
    assert!(
        parsed.issues.iter().all(|i| i.severity != R),
        "{:?}",
        parsed.issues
    );
    parsed.def.expect("no Rejected issue, so a definition")
}

fn task(step: &StepDef) -> &TaskDef {
    match step {
        StepDef::Task(task) => task,
        StepDef::Group(group) => panic!("a group: {group:?}"),
    }
}

fn group(step: &StepDef) -> &GroupDef {
    match step {
        StepDef::Group(group) => group,
        StepDef::Task(task) => panic!("a task: {task:?}"),
    }
}

// ---------------------------------------------------------------------------
// The whole definition
// ---------------------------------------------------------------------------

#[test]
fn a_clean_definition_parses_with_no_issue() {
    let parsed = compat(&wf(json!([leaf("t")])));
    assert_eq!(rows(&parsed), vec![]);
    assert!(parsed.def.is_some());
}

#[test]
fn a_definition_that_is_not_an_object_is_one_parse_failure_at_the_root() {
    for json in [json!([]), json!("w"), json!(null), json!(5)] {
        let parsed = compat(&json);
        assert_eq!(rows(&parsed), vec![row(C::ParseFailed, R, "")], "{json}");
        assert!(parsed.def.is_none());
    }
}

#[test]
fn def_is_some_exactly_when_no_issue_is_rejected() {
    // A Defect and an Advisory still build.
    let parsed = compat(&json!({"id": "w", "path": "ui", "tasks": [
        {"id": "t", "conditon": true, "function": {"name": "f"}}
    ]}));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::LikelyTypo, D, "tasks[0].conditon"),
            row(C::UnknownKey, A, "path"),
        ]
    );
    assert!(parsed.def.is_some());

    let parsed = compat(&json!({"id": "w", "priority": -1, "tasks": [leaf("t")]}));
    assert_eq!(rows(&parsed), vec![row(C::ParseFailed, R, "priority")]);
    assert!(parsed.def.is_none());
}

// ---------------------------------------------------------------------------
// The order
// ---------------------------------------------------------------------------

#[test]
fn every_issue_comes_out_in_rule_order_not_key_order() {
    // Breaks rules 1, 2, 4.2, 4.3, 4.6, 5, 6 and 7 at once. The keys are
    // read sorted (`loop` and `priority` before `tasks`), so the order of the
    // issues is the rules'.
    let parsed = compat(&json!({
        "id": "",
        "name": 5,
        "priority": "high",
        "loop": {"max": 0, "setup": [{"function": {"name": "f"}}]},
        "tasks": [
            {"id": "a", "terminal": "yes", "function": {"name": "f"}},
            {"id": "a", "function": {"name": ""}}
        ]
    }));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::EmptyWorkflowId, R, "id"),
            row(C::ParseFailed, R, "name"),
            row(C::InvalidTerminal, R, "tasks[0].terminal"),
            row(C::DuplicateStepId, R, "tasks[1].id"),
            row(C::InvalidFunctionName, R, "tasks[1].function.name"),
            row(C::LoopBoundEmpty, R, "loop.max"),
            row(C::MissingStepId, R, "loop.setup[0].id"),
            row(C::ParseFailed, R, "priority"),
        ]
    );
    assert!(parsed.def.is_none());
}

#[test]
fn rule_three_sits_between_the_header_and_the_loop() {
    // Rule 3 cannot break alongside rule 4, which walks the tasks rule 3
    // found missing; this one breaks 1, 2, 3, 5 and 7.
    let parsed = compat(&json!({
        "id": 5,
        "description": 3,
        "tasks": [],
        "loop": {"max": 1, "increment": 0},
        "version": "x",
        "zzz": 1
    }));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::EmptyWorkflowId, R, "id"),
            row(C::ParseFailed, R, "description"),
            row(C::NoTasks, R, "tasks"),
            row(C::LoopIncrementTooSmall, R, "loop.increment"),
            row(C::ParseFailed, R, "version"),
            row(C::UnknownKey, A, "zzz"),
        ]
    );
}

#[test]
fn a_tasks_own_issues_come_in_v3s_order_then_its_other_keys_then_its_unknown_keys() {
    let parsed = compat(&wf(json!([{
        "terminal": 1,
        "for_each": {"over": 3, "as": "p"},
        "on_failure": "explode",
        "conditon": true
    }])));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::MissingStepId, R, "tasks[0].id"),
            row(C::InvalidTerminal, R, "tasks[0].terminal"),
            row(C::InvalidForEach, R, "tasks[0].for_each.over"),
            row(C::MissingFunction, R, "tasks[0].function"),
            row(C::ParseFailed, R, "tasks[0].on_failure"),
            row(C::LikelyTypo, D, "tasks[0].conditon"),
        ]
    );
}

#[test]
fn a_groups_own_issues_come_before_its_members() {
    let parsed = compat(&wf(json!([{
        "id": "g",
        "terminal": "x",
        "halt_on": "failure",
        "for_each": {"over": [], "as": "p"},
        "name": 1,
        "zz": 1,
        "tasks": []
    }])));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::InvalidTerminal, R, "tasks[0].terminal"),
            row(C::InvalidHaltOn, R, "tasks[0].halt_on"),
            row(C::InvalidForEach, R, "tasks[0].for_each"),
            row(C::EmptyGroup, R, "tasks[0].tasks"),
            row(C::ParseFailed, R, "tasks[0].name"),
            row(C::UnknownKey, A, "tasks[0].zz"),
        ]
    );

    let parsed = compat(&wf(json!([{"id": "g", "zz": 1, "tasks": [{"id": "t"}]}])));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::UnknownKey, A, "tasks[0].zz"),
            row(C::MissingFunction, R, "tasks[0].tasks[0].function"),
        ]
    );
}

// ---------------------------------------------------------------------------
// Rules 1 to 3: the header and `tasks`
// ---------------------------------------------------------------------------

#[test]
fn rule_1_the_id_is_a_non_empty_string() {
    for id in [None, Some(json!("")), Some(json!(5)), Some(json!(null))] {
        let mut json = wf(json!([leaf("t")]));
        match &id {
            Some(id) => json["id"] = id.clone(),
            None => {
                json.as_object_mut().unwrap().remove("id");
            }
        }
        assert_eq!(
            rows(&compat(&json)),
            vec![row(C::EmptyWorkflowId, R, "id")],
            "{id:?}"
        );
    }
}

#[test]
fn rule_2_name_and_description_are_optional_strings() {
    let parsed = compat(&json!({"id": "w", "name": 5, "description": [], "tasks": [leaf("t")]}));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::ParseFailed, R, "name"),
            row(C::ParseFailed, R, "description"),
        ]
    );

    // An empty name is accepted (break 6), and so is null.
    let parsed = compat(&json!({"id": "w", "name": "", "description": null, "tasks": [leaf("t")]}));
    let def = built(parsed);
    assert_eq!(def.header.name.as_deref(), Some(""));
    assert_eq!(def.header.description, None);
}

#[test]
fn rule_3_tasks_is_a_non_empty_array() {
    for tasks in [None, Some(json!([])), Some(json!({})), Some(json!("t"))] {
        let mut json = json!({"id": "w"});
        if let Some(tasks) = &tasks {
            json["tasks"] = tasks.clone();
        }
        assert_eq!(
            rows(&compat(&json)),
            vec![row(C::NoTasks, R, "tasks")],
            "{tasks:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the steps of `tasks`
// ---------------------------------------------------------------------------

#[test]
fn rule_4_1_a_group_too_deep_is_reported_and_not_descended_into() {
    // MAX_GROUP_DEPTH levels parse.
    assert_eq!(MAX_GROUP_DEPTH, 8);
    let parsed = compat(&wf(nested(MAX_GROUP_DEPTH, json!({}))));
    assert_eq!(rows(&parsed), vec![]);

    // One more is GROUP_TOO_DEEP at the ninth level. Its members, here a
    // task with no function, are never walked.
    let mut too_deep = nested(MAX_GROUP_DEPTH + 1, json!({}));
    too_deep[0]["zz"] = json!(1);
    let mut innermost = &mut too_deep;
    for _ in 0..=MAX_GROUP_DEPTH {
        innermost = &mut innermost[0]["tasks"];
    }
    innermost[0] = json!({"id": "innermost"});
    let at = nested_path(MAX_GROUP_DEPTH);
    assert_eq!(
        rows(&compat(&wf(too_deep))),
        vec![
            row(C::UnknownKey, A, "tasks[0].zz"),
            row(C::GroupTooDeep, R, &at),
        ]
    );
}

#[test]
fn rule_4_1_group_too_deep_follows_the_nodes_id_terminal_halt_on_and_for_each() {
    // v3's per-node order: the depth is reported last.
    let deepest = json!({"terminal": 1, "halt_on": "failure", "for_each": {}});
    let parsed = compat(&wf(nested(MAX_GROUP_DEPTH + 1, deepest)));
    let at = nested_path(MAX_GROUP_DEPTH);
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::InvalidTerminal, R, &format!("{at}.terminal")),
            row(C::InvalidHaltOn, R, &format!("{at}.halt_on")),
            row(C::InvalidForEach, R, &format!("{at}.for_each")),
            row(C::GroupTooDeep, R, &at),
        ]
    );
}

#[test]
fn rule_4_2_a_step_id_is_a_non_empty_string_unique_across_steps_and_setup() {
    let parsed = compat(&json!({
        "id": "w",
        "loop": {"max": 1, "setup": [leaf("g"), leaf("s"), leaf("s")]},
        "tasks": [
            {"function": {"name": "f"}},
            {"id": "", "function": {"name": "f"}},
            {"id": 5, "function": {"name": "f"}},
            {"id": "g", "tasks": [leaf("a"), leaf("g")]},
            leaf("a")
        ]
    }));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::MissingStepId, R, "tasks[0].id"),
            row(C::MissingStepId, R, "tasks[1].id"),
            row(C::MissingStepId, R, "tasks[2].id"),
            row(C::DuplicateStepId, R, "tasks[3].tasks[1].id"),
            row(C::DuplicateStepId, R, "tasks[4].id"),
            row(C::DuplicateStepId, R, "loop.setup[0].id"),
            row(C::DuplicateStepId, R, "loop.setup[2].id"),
        ]
    );
    let dup = &parsed.issues[3];
    assert!(dup.message.contains("tasks[3]"), "{}", dup.message);
}

#[test]
fn rule_4_2_a_step_that_is_not_an_object_is_a_task_with_neither_id_nor_function() {
    // v3's walker treats it as a leaf.
    assert_eq!(
        rows(&compat(&wf(json!([5])))),
        vec![
            row(C::MissingStepId, R, "tasks[0].id"),
            row(C::MissingFunction, R, "tasks[0].function"),
        ]
    );
}

#[test]
fn rule_4_3_terminal_is_a_bool() {
    let parsed = compat(&wf(json!([
        {"id": "a", "terminal": "on_failure", "function": {"name": "f"}},
        {"id": "b", "terminal": 1, "function": {"name": "f"}},
        {"id": "g", "terminal": null, "tasks": [leaf("c")]},
    ])));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::InvalidTerminal, R, "tasks[0].terminal"),
            row(C::InvalidTerminal, R, "tasks[1].terminal"),
            row(C::InvalidTerminal, R, "tasks[2].terminal"),
        ]
    );
    assert!(parsed.issues[0].message.contains("halt_on"));
    assert_eq!(parsed.issues[1].message, "terminal must be a boolean");
}

#[test]
fn rule_4_4_halt_on_on_a_group_is_refused_compat_or_not() {
    let json = wf(json!([{"id": "g", "halt_on": "failure", "tasks": [leaf("t")]}]));
    for parsed in [compat(&json), strict(&json)] {
        assert_eq!(
            rows(&parsed),
            vec![row(C::InvalidHaltOn, R, "tasks[0].halt_on")]
        );
        assert!(parsed.def.is_none());
    }
}

#[test]
fn rule_4_5_for_each_on_a_group_is_refused() {
    let parsed = compat(&wf(json!([
        {"id": "g", "for_each": {"over": [], "as": "p"}, "tasks": [leaf("t")]}
    ])));
    assert_eq!(
        rows(&parsed),
        vec![row(C::InvalidForEach, R, "tasks[0].for_each")]
    );
    assert!(parsed.issues[0].message.contains("task group"));
}

#[test]
fn rules_4_4_and_4_5_refuse_a_groups_halt_on_and_for_each_even_when_null() {
    // v3's `check_shape` asks whether the key is present, whatever its value.
    let parsed = compat(&wf(json!([
        {"id": "g", "halt_on": null, "for_each": null, "tasks": [leaf("t")]}
    ])));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::InvalidHaltOn, R, "tasks[0].halt_on"),
            row(C::InvalidForEach, R, "tasks[0].for_each"),
        ]
    );
    assert!(parsed.def.is_none());
}

#[test]
fn rule_4_6_a_task_has_a_function_with_a_non_empty_name() {
    let parsed = compat(&wf(json!([
        {"id": "a"},
        {"id": "b", "function": "map"},
        {"id": "c", "function": {"name": ""}},
        {"id": "d", "function": {"input": {}}},
        {"id": "e", "function": null},
        {"id": "f", "function": {"name": 5}},
    ])));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::MissingFunction, R, "tasks[0].function"),
            row(C::InvalidFunctionName, R, "tasks[1].function.name"),
            row(C::InvalidFunctionName, R, "tasks[2].function.name"),
            row(C::InvalidFunctionName, R, "tasks[3].function.name"),
            row(C::InvalidFunctionName, R, "tasks[4].function.name"),
            row(C::InvalidFunctionName, R, "tasks[5].function.name"),
        ]
    );
}

#[test]
fn rule_4_6_input_is_any_json_and_defaults_to_an_empty_object() {
    let def = built(compat(&wf(json!([
        leaf("a"),
        {"id": "b", "function": {"name": "f", "input": [1, 2]}},
        {"id": "c", "function": {"name": "f", "input": null}},
    ]))));
    let inputs: Vec<&Value> = def.tasks.iter().map(|s| &task(s).function.input).collect();
    assert_eq!(inputs, [&json!({}), &json!([1, 2]), &Value::Null]);
    assert_eq!(task(&def.tasks[0]).function.name, "f");
}

#[test]
fn rule_4_7_a_groups_tasks_is_a_non_empty_array() {
    let parsed = compat(&wf(json!([
        {"id": "a", "tasks": []},
        {"id": "b", "tasks": "oops"},
        {"id": "c", "tasks": null},
        {"id": "d", "tasks": [{"id": "e", "tasks": {}}]},
    ])));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::EmptyGroup, R, "tasks[0].tasks"),
            row(C::EmptyGroup, R, "tasks[1].tasks"),
            row(C::EmptyGroup, R, "tasks[2].tasks"),
            row(C::EmptyGroup, R, "tasks[3].tasks[0].tasks"),
        ]
    );
}

#[test]
fn rule_4_8_a_steps_other_keys_are_typed() {
    let parsed = compat(&wf(json!([
        {"id": "a", "name": 1, "description": true, "on_failure": "explode",
         "on_rejection": "fail", "ext": 5, "function": {"name": "f"}},
        {"id": "g", "name": [], "ext": [], "tasks": [leaf("t")]},
    ])));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::ParseFailed, R, "tasks[0].name"),
            row(C::ParseFailed, R, "tasks[0].description"),
            row(C::ParseFailed, R, "tasks[0].on_failure"),
            row(C::ParseFailed, R, "tasks[0].on_rejection"),
            row(C::ParseFailed, R, "tasks[0].ext"),
            row(C::ParseFailed, R, "tasks[1].name"),
            row(C::ParseFailed, R, "tasks[1].ext"),
        ]
    );
    assert!(
        parsed.issues[2].message.contains("\"halt\""),
        "the message names the spellings: {}",
        parsed.issues[2].message
    );
}

#[test]
fn rule_4_8_the_policies_read_their_spellings() {
    let def = built(compat(&wf(json!([
        {"id": "a", "on_failure": "halt", "on_rejection": "halt", "function": {"name": "f"}},
        {"id": "b", "on_failure": "continue", "function": {"name": "f"}},
        {"id": "c", "on_failure": "fail", "on_rejection": "continue", "function": {"name": "f"}},
    ]))));
    let policies: Vec<(FailurePolicy, RejectionPolicy)> = def
        .tasks
        .iter()
        .map(|s| (task(s).on_failure, task(s).on_rejection))
        .collect();
    assert_eq!(
        policies,
        [
            (FailurePolicy::Halt, RejectionPolicy::Halt),
            (FailurePolicy::Continue, RejectionPolicy::Continue),
            (FailurePolicy::Fail, RejectionPolicy::Continue),
        ]
    );
}

// ---------------------------------------------------------------------------
// `for_each` on a task: at most one issue, its first problem
// ---------------------------------------------------------------------------

#[test]
fn for_each_on_a_built_in_is_refused_first() {
    assert_eq!(BUILTIN_FUNCTIONS.len(), 8);
    for name in BUILTIN_FUNCTIONS {
        // `over` is missing too; the built-in is the first problem.
        let parsed = compat(&wf(json!([
            {"id": "t", "for_each": {"as": "p"}, "function": {"name": name, "input": {}}}
        ])));
        assert_eq!(
            rows(&parsed),
            vec![row(C::InvalidForEach, R, "tasks[0].for_each")],
            "{name}"
        );
        assert!(
            parsed.issues[0].message.contains(&format!("'{name}'")),
            "{}",
            parsed.issues[0].message
        );
    }
}

#[test]
fn for_each_reports_its_first_problem_at_its_key() {
    for (for_each, path, code) in [
        (json!(5), "tasks[0].for_each", C::ParseFailed),
        (json!({"as": "p"}), "tasks[0].for_each.over", C::ParseFailed),
        (json!({}), "tasks[0].for_each.over", C::ParseFailed),
        (json!({"over": []}), "tasks[0].for_each.as", C::ParseFailed),
        (
            json!({"over": "data.ps", "as": "p"}),
            "tasks[0].for_each.over",
            C::InvalidForEach,
        ),
        (
            json!({"over": null, "as": "p"}),
            "tasks[0].for_each.over",
            C::InvalidForEach,
        ),
        // `over` first, as v3's order has it.
        (
            json!({"over": 3, "as": ""}),
            "tasks[0].for_each.over",
            C::InvalidForEach,
        ),
        (
            json!({"over": [], "as": 5}),
            "tasks[0].for_each.as",
            C::ParseFailed,
        ),
        (
            json!({"over": [], "as": "a..b"}),
            "tasks[0].for_each.as",
            C::InvalidForEach,
        ),
        (
            json!({"over": [], "as": ""}),
            "tasks[0].for_each.as",
            C::InvalidForEach,
        ),
        (
            json!({"over": [], "as": "p", "max_concurrency": 0}),
            "tasks[0].for_each.max_concurrency",
            C::InvalidForEach,
        ),
        (
            json!({"over": [], "as": "p", "max_concurrency": "8"}),
            "tasks[0].for_each.max_concurrency",
            C::ParseFailed,
        ),
        (
            json!({"over": [], "as": "p", "max_concurrency": -1}),
            "tasks[0].for_each.max_concurrency",
            C::ParseFailed,
        ),
        (
            json!({"over": [], "as": "p", "collect": "temp_data.m"}),
            "tasks[0].for_each.collect",
            C::InvalidForEach,
        ),
        (
            json!({"over": [], "as": "p", "into": "temp_data.ms"}),
            "tasks[0].for_each.into",
            C::InvalidForEach,
        ),
        (
            json!({"over": [], "as": "p", "collect": 5, "into": "temp_data.ms"}),
            "tasks[0].for_each.collect",
            C::ParseFailed,
        ),
        (
            json!({"over": [], "as": "p", "collect": "m", "into": "temp_data.ms"}),
            "tasks[0].for_each.collect",
            C::InvalidForEach,
        ),
        (
            json!({"over": [], "as": "p", "collect": "temp_data.m", "into": "temp_data"}),
            "tasks[0].for_each.into",
            C::InvalidForEach,
        ),
        (
            json!({"over": [], "as": "p", "collect": "temp_data.m", "into": "payload.ms"}),
            "tasks[0].for_each.into",
            C::InvalidForEach,
        ),
        (
            json!({"over": [], "as": "p", "collect": "temp_data.p.out", "into": "data.outs"}),
            "tasks[0].for_each.collect",
            C::InvalidForEach,
        ),
        (
            json!({"over": [], "as": "p", "collect": "temp_data.p_index", "into": "data.outs"}),
            "tasks[0].for_each.collect",
            C::InvalidForEach,
        ),
        (
            json!({"over": [], "as": "p", "collect": "temp_data.m", "into": "temp_data.m.x"}),
            "tasks[0].for_each.into",
            C::InvalidForEach,
        ),
        (
            json!({"over": [], "as": "p", "collect": "temp_data.m", "into": "temp_data.p"}),
            "tasks[0].for_each.into",
            C::InvalidForEach,
        ),
        (
            json!({"over": [], "as": "p", "collect": "temp_data.m", "into": "temp_data.p_index"}),
            "tasks[0].for_each.into",
            C::InvalidForEach,
        ),
    ] {
        assert_eq!(
            rows(&compat(&fan_out(for_each.clone()))),
            vec![row(code, R, path)],
            "{for_each}"
        );
    }
}

#[test]
fn a_valid_for_each_is_read_with_its_defaults() {
    let def = built(compat(&fan_out(
        json!({"over": {"var": "data.ps"}, "as": "p"}),
    )));
    let for_each: &ForEachDef = task(&def.tasks[0]).for_each.as_ref().unwrap();
    assert_eq!(for_each.over.0, json!({"var": "data.ps"}));
    assert_eq!(for_each.r#as, "p");
    assert_eq!(for_each.max_concurrency, 1);
    assert_eq!((&for_each.collect, &for_each.into), (&None, &None));

    let def = built(compat(&fan_out(json!({
        "over": [], "as": "cur.p", "max_concurrency": 8,
        "collect": "temp_data.p_out", "into": "data.outs"
    }))));
    let for_each = task(&def.tasks[0]).for_each.as_ref().unwrap();
    assert_eq!(for_each.max_concurrency, 8);
    assert_eq!(for_each.collect.as_deref(), Some("temp_data.p_out"));
    assert_eq!(for_each.into.as_deref(), Some("data.outs"));

    // An explicit null is absent, as v3's `Option` read it.
    let def = built(compat(&fan_out(Value::Null)));
    assert_eq!(task(&def.tasks[0]).for_each, None);
}

// ---------------------------------------------------------------------------
// Rule 5: `loop`, in v3's `check_loop` order
// ---------------------------------------------------------------------------

fn looped(config: Value) -> Value {
    json!({"id": "w", "loop": config, "tasks": [leaf("t")]})
}

#[test]
fn rule_5_max_is_a_required_integer() {
    for (config, path) in [
        (json!({}), "loop.max"),
        (json!({"max": "3"}), "loop.max"),
        (json!({"max": 3.5}), "loop.max"),
        (json!({"max": null}), "loop.max"),
        (json!(5), "loop"),
    ] {
        assert_eq!(
            rows(&compat(&looped(config.clone()))),
            vec![row(C::ParseFailed, R, path)],
            "{config}"
        );
    }
    // An explicit null is no loop.
    assert_eq!(built(compat(&looped(Value::Null))).r#loop, None);
}

#[test]
fn rule_5_the_loop_rules_in_v3s_order() {
    for (config, expected) in [
        (
            json!({"max": 3, "init": "0", "increment": 1.5}),
            vec![
                row(C::ParseFailed, R, "loop.init"),
                row(C::ParseFailed, R, "loop.increment"),
            ],
        ),
        (
            json!({"max": 0, "increment": 0}),
            vec![
                row(C::LoopIncrementTooSmall, R, "loop.increment"),
                row(C::LoopBoundEmpty, R, "loop.max"),
            ],
        ),
        (
            json!({"max": 5, "init": 5}),
            vec![row(C::LoopBoundEmpty, R, "loop.max")],
        ),
        (
            json!({"max": 3, "counter": "a..b"}),
            vec![row(C::LoopCounterInvalid, R, "loop.counter")],
        ),
        (
            json!({"max": 3, "counter": 5}),
            vec![row(C::ParseFailed, R, "loop.counter")],
        ),
        (
            json!({"max": 3, "over": [], "as": "", "scratch": "s."}),
            vec![
                row(C::LoopSlotInvalid, R, "loop.as"),
                row(C::LoopSlotInvalid, R, "loop.scratch"),
            ],
        ),
        (
            json!({"max": 3, "over": [], "as": 1, "scratch": false}),
            vec![
                row(C::ParseFailed, R, "loop.as"),
                row(C::ParseFailed, R, "loop.scratch"),
            ],
        ),
        (
            json!({"max": 3, "as": "item"}),
            vec![row(C::LoopItemWithoutOver, R, "loop.as")],
        ),
        (
            json!({"max": 3, "over": null}),
            vec![row(C::LoopOverInvalid, R, "loop.over")],
        ),
        (
            json!({"max": 3, "over": "data.items"}),
            vec![row(C::LoopOverInvalid, R, "loop.over")],
        ),
        (
            json!({"max": 3, "init": -1, "over": 5}),
            vec![
                row(C::LoopOverInvalid, R, "loop.over"),
                row(C::LoopOverInvalid, R, "loop.init"),
            ],
        ),
        (
            json!({"max": 3, "counter": "it", "as": "it.item", "scratch": "it", "over": []}),
            vec![
                row(C::LoopSlotCollision, R, "loop.as"),
                row(C::LoopSlotCollision, R, "loop.scratch"),
                row(C::LoopSlotCollision, R, "loop.scratch"),
            ],
        ),
        (
            json!({"max": 3, "setup": {}}),
            vec![row(C::ParseFailed, R, "loop.setup")],
        ),
    ] {
        assert_eq!(rows(&compat(&looped(config.clone()))), expected, "{config}");
    }
}

#[test]
fn rule_5_messages_are_v3s() {
    let parsed = compat(&looped(json!({"max": 3, "over": null})));
    assert_eq!(
        parsed.issues[0].message,
        "loop over must be a JSONLogic expression or an array literal — \
         null can never evaluate to an array"
    );
    let parsed = compat(&looped(json!({"max": 0, "init": 2})));
    assert_eq!(
        parsed.issues[0].message,
        "loop max (0) must be greater than init (2) — the bound is half-open, \
         so this could never run a sweep"
    );
}

#[test]
fn a_valid_loop_is_read_with_its_defaults() {
    let def = built(compat(&looped(json!({"max": 3}))));
    let l: &LoopDef = def.r#loop.as_ref().unwrap();
    assert_eq!((l.init, l.increment, l.max), (0, 1, 3));
    assert_eq!(
        (&l.counter, &l.over, &l.r#as, &l.scratch),
        (&None, &None, &None, &None)
    );
    assert!(l.setup.is_empty());

    let def = built(compat(&looped(json!({
        "counter": "i", "init": 1, "increment": 2, "max": 9,
        "over": {"var": "data.items"}, "as": "item", "scratch": "s",
        "setup": [leaf("s0")]
    }))));
    let l = def.r#loop.as_ref().unwrap();
    assert_eq!((l.init, l.increment, l.max), (1, 2, 9));
    assert_eq!(l.counter.as_deref(), Some("i"));
    assert_eq!(
        l.over.as_ref().map(|o| &o.0),
        Some(&json!({"var": "data.items"}))
    );
    assert_eq!(l.r#as.as_deref(), Some("item"));
    assert_eq!(l.scratch.as_deref(), Some("s"));
    assert_eq!(l.setup.len(), 1);
    assert_eq!(l.setup[0].id(), "s0");
}

// ---------------------------------------------------------------------------
// Rule 6: the steps of `loop.setup`
// ---------------------------------------------------------------------------

#[test]
fn rule_6_setup_steps_are_walked_like_tasks_after_the_loop() {
    let parsed = compat(&json!({
        "id": "w",
        "loop": {"max": 0, "zz": 1, "setup": [
            {"id": "s"},
            {"id": "g", "halt_on": "failure", "tasks": [{"id": "t", "function": "f"}]}
        ]},
        "tasks": [leaf("t")]
    }));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::LoopBoundEmpty, R, "loop.max"),
            row(C::UnknownKey, A, "loop.zz"),
            row(C::MissingFunction, R, "loop.setup[0].function"),
            row(C::InvalidHaltOn, R, "loop.setup[1].halt_on"),
            row(C::DuplicateStepId, R, "loop.setup[1].tasks[0].id"),
            row(
                C::InvalidFunctionName,
                R,
                "loop.setup[1].tasks[0].function.name"
            ),
        ]
    );
    for issue in &parsed.issues[2..] {
        assert_eq!(issue.step_kind, Some(StepKind::Setup), "{issue}");
    }
}

// ---------------------------------------------------------------------------
// Rule 7: everything else at the workflow
// ---------------------------------------------------------------------------

#[test]
fn rule_7_the_workflows_other_keys_are_typed() {
    let parsed = compat(&json!({
        "id": "w",
        "priority": "high",
        "version": -1,
        "channel": 5,
        "status": "live",
        "on_failure": "halt",
        "rollout": {"bucket_start": -1, "bucket_end": 300},
        "tags": ["a", 1, "b", null],
        "created_at": "yesterday",
        "updated_at": 5,
        "ext": [],
        "tasks": [leaf("t")]
    }));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::ParseFailed, R, "priority"),
            row(C::ParseFailed, R, "version"),
            row(C::ParseFailed, R, "channel"),
            row(C::ParseFailed, R, "status"),
            row(C::ParseFailed, R, "on_failure"),
            row(C::ParseFailed, R, "rollout.bucket_start"),
            row(C::ParseFailed, R, "rollout.bucket_end"),
            row(C::ParseFailed, R, "tags[1]"),
            row(C::ParseFailed, R, "tags[3]"),
            row(C::ParseFailed, R, "created_at"),
            row(C::ParseFailed, R, "updated_at"),
            row(C::ParseFailed, R, "ext"),
        ]
    );
}

#[test]
fn rule_7_priority_high_is_a_parse_failure_at_priority() {
    let parsed = compat(&json!({"id": "w", "priority": "high", "tasks": [leaf("t")]}));
    assert_eq!(rows(&parsed), vec![row(C::ParseFailed, R, "priority")]);
    assert!(
        parsed.issues[0].message.contains("integer"),
        "{}",
        parsed.issues[0].message
    );
}

#[test]
fn rule_7_priority_and_version_fit_u32() {
    let parsed = compat(&json!({
        "id": "w", "priority": 4_294_967_296_u64, "version": 1.0, "tasks": [leaf("t")]
    }));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::ParseFailed, R, "priority"),
            row(C::ParseFailed, R, "version"),
        ]
    );
    let def = built(compat(&json!({
        "id": "w", "priority": 4_294_967_295_u64, "tasks": [leaf("t")]
    })));
    assert_eq!(def.priority, u32::MAX);
}

#[test]
fn rule_7_rollout_needs_both_bounds() {
    for (rollout, expected) in [
        (
            json!({"bucket_start": 0}),
            vec![row(C::ParseFailed, R, "rollout.bucket_end")],
        ),
        (
            json!({}),
            vec![
                row(C::ParseFailed, R, "rollout.bucket_start"),
                row(C::ParseFailed, R, "rollout.bucket_end"),
            ],
        ),
        (json!(50), vec![row(C::ParseFailed, R, "rollout")]),
    ] {
        let json = json!({"id": "w", "rollout": rollout, "tasks": [leaf("t")]});
        assert_eq!(rows(&compat(&json)), expected, "{rollout}");
    }
    let def = built(compat(&json!({
        "id": "w", "rollout": {"bucket_start": 0, "bucket_end": 50}, "tasks": [leaf("t")]
    })));
    assert_eq!(def.rollout, Some(Rollout::new(0, 50)));
    let def_none = def_of(&json!({"id": "w", "rollout": null, "tasks": [leaf("t")]}));
    assert_eq!(def_none.rollout, None);
}

#[test]
fn rule_7_timestamps_are_read_as_v3_read_them() {
    // v3 read them through chrono's serde, which is `FromStr`: RFC 3339 and
    // its relaxed forms, chrono's own `Display` among them.
    for (text, utc) in [
        ("2026-01-02T03:04:05Z", "2026-01-02T03:04:05+00:00"),
        ("2026-01-02T05:04:05+02:00", "2026-01-02T03:04:05+00:00"),
        ("2026-01-02 03:04:05 UTC", "2026-01-02T03:04:05+00:00"),
        ("2026-01-02T03:04:05+0000", "2026-01-02T03:04:05+00:00"),
    ] {
        let def = def_of(&json!({
            "id": "w", "created_at": text, "updated_at": text, "tasks": [leaf("t")]
        }));
        assert_eq!(
            def.created_at.map(|t| t.to_rfc3339()).as_deref(),
            Some(utc),
            "{text}"
        );
        assert_eq!(def.updated_at, def.created_at, "{text}");
        let v3: dataflow_rs_v3::Workflow = serde_json::from_value(json!({
            "id": "w", "name": "w", "created_at": text, "tasks": []
        }))
        .unwrap_or_else(|e| panic!("v3 loads {text}: {e}"));
        assert_eq!(v3.created_at, def.created_at, "{text}");
    }

    let parsed = compat(&json!({"id": "w", "created_at": "2026-01-02", "tasks": [leaf("t")]}));
    assert_eq!(rows(&parsed), vec![row(C::ParseFailed, R, "created_at")]);
}

fn def_of(json: &Value) -> WorkflowDef {
    built(compat(json))
}

#[test]
fn every_key_set_is_read() {
    let def = def_of(&json!({
        "id": "w",
        "name": "W",
        "description": "d",
        "priority": 5,
        "condition": {"==": [1, 1]},
        "on_failure": "continue",
        "channel": "orders",
        "status": "paused",
        "version": 2,
        "rollout": {"bucket_start": 10, "bucket_end": 20},
        "tags": ["a", "b"],
        "created_at": "2026-01-02T03:04:05Z",
        "updated_at": "2026-01-02T05:04:05+02:00",
        "ext": {"path": "ui/folder"},
        "tasks": [{
            "id": "t", "name": "T", "description": "td", "condition": false,
            "terminal": true, "ext": {"x": 1}, "function": {"name": "f", "input": {"a": 1}}
        }, {
            "id": "g", "name": "G", "condition": null, "terminal": false, "tasks": [leaf("u")]
        }]
    }));
    assert_eq!(def.id, "w");
    assert_eq!(def.header.name.as_deref(), Some("W"));
    assert_eq!(def.header.description.as_deref(), Some("d"));
    assert_eq!(def.header.ext.get("path"), Some(&json!("ui/folder")));
    assert_eq!(def.priority, 5);
    assert_eq!(
        def.condition.as_ref().map(|c| &c.0),
        Some(&json!({"==": [1, 1]}))
    );
    assert_eq!(def.on_failure, WorkflowFailurePolicy::Continue);
    assert_eq!(def.channel, "orders");
    assert_eq!(def.status, Status::Paused);
    assert_eq!(def.version, 2);
    assert_eq!(def.rollout, Some(Rollout::new(10, 20)));
    assert_eq!(def.tags, ["a", "b"]);
    assert_eq!(
        def.created_at.map(|t| t.to_rfc3339()),
        Some("2026-01-02T03:04:05+00:00".to_string())
    );
    assert_eq!(
        def.updated_at.map(|t| t.to_rfc3339()),
        Some("2026-01-02T03:04:05+00:00".to_string())
    );

    let t = task(&def.tasks[0]);
    assert_eq!(t.id, "t");
    assert_eq!(t.header.name.as_deref(), Some("T"));
    assert_eq!(t.header.description.as_deref(), Some("td"));
    assert_eq!(t.header.ext.get("x"), Some(&json!(1)));
    assert_eq!(t.condition.as_ref().map(|c| &c.0), Some(&json!(false)));
    assert!(t.terminal);
    assert_eq!(t.function.input, json!({"a": 1}));

    let g = group(&def.tasks[1]);
    assert_eq!(g.id, "g");
    assert_eq!(g.header.name.as_deref(), Some("G"));
    // An explicit null condition is kept: v3 evaluated it, to a falsy null.
    assert_eq!(g.condition.as_ref().map(|c| &c.0), Some(&Value::Null));
    assert!(!g.terminal);
    assert_eq!(g.tasks.len(), 1);

    for status in ["active", "paused", "archived"] {
        let def = def_of(&json!({"id": "w", "status": status, "tasks": [leaf("t")]}));
        assert_eq!(def.status.as_str(), status);
    }
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

#[test]
fn the_defaults() {
    let def = def_of(&wf(json!([leaf("t"), {"id": "g", "tasks": [leaf("u")]}])));
    assert_eq!(def.header.name, None);
    assert_eq!(def.header.description, None);
    assert!(def.header.ext.is_empty());
    assert_eq!(def.condition, None);
    assert_eq!(def.priority, 0);
    assert_eq!(def.channel, "default");
    assert_eq!(def.status, Status::Active);
    assert_eq!(def.version, 1);
    assert_eq!(def.on_failure, WorkflowFailurePolicy::Fail);
    assert_eq!(def.rollout, None);
    assert_eq!(def.r#loop, None);
    assert!(def.tags.is_empty());
    assert_eq!((def.created_at, def.updated_at), (None, None));

    let t = task(&def.tasks[0]);
    assert_eq!(t.header.name, None);
    assert_eq!(t.condition, None);
    assert_eq!(t.function.input, json!({}));
    assert_eq!(t.on_failure, FailurePolicy::Fail);
    assert_eq!(t.on_rejection, RejectionPolicy::Continue);
    assert!(!t.terminal);
    assert_eq!(t.for_each, None);

    let g = group(&def.tasks[1]);
    assert_eq!(g.condition, None);
    assert!(!g.terminal);
    assert!(g.header.ext.is_empty());
}

// ---------------------------------------------------------------------------
// Unknown keys
// ---------------------------------------------------------------------------

#[test]
fn a_task_typo_under_compat_is_a_likely_typo_kept_in_ext() {
    let parsed = compat(&wf(json!([
        {"id": "t", "conditon": false, "function": {"name": "f"}}
    ])));
    assert_eq!(
        rows(&parsed),
        vec![row(C::LikelyTypo, D, "tasks[0].conditon")]
    );
    assert!(
        parsed.issues[0].message.contains("`condition`"),
        "{}",
        parsed.issues[0].message
    );
    let def = built(parsed);
    let t = task(&def.tasks[0]);
    assert_eq!(t.header.ext.get("conditon"), Some(&json!(false)));
    assert_eq!(t.condition, None, "the typo is not read as the key");
}

#[test]
fn the_uis_path_on_a_workflow_is_an_unknown_key_kept_in_ext() {
    let parsed = compat(&json!({"id": "w", "path": "ui/folder", "tasks": [leaf("t")]}));
    assert_eq!(rows(&parsed), vec![row(C::UnknownKey, A, "path")]);
    assert_eq!(
        built(parsed).header.ext.get("path"),
        Some(&json!("ui/folder"))
    );
}

#[test]
fn a_loop_typo_lands_in_the_workflows_ext_under_loop() {
    let parsed = compat(&looped(json!({"maxx": 3, "max": 3})));
    assert_eq!(rows(&parsed), vec![row(C::LikelyTypo, D, "loop.maxx")]);
    assert!(parsed.issues[0].message.contains("`max`"));
    let def = built(parsed);
    assert_eq!(def.header.ext.get("loop.maxx"), Some(&json!(3)));
    assert_eq!(def.r#loop.unwrap().max, 3);
}

#[test]
fn nested_unknown_keys_land_in_the_owning_nodes_ext() {
    let parsed = compat(&json!({
        "id": "w",
        "rollout": {"bucket_start": 0, "bucket_end": 100, "bucket_strat": 5},
        "tasks": [{
            "id": "t",
            "for_each": {"over": [1, 2], "as": "p", "ass": "q"},
            "function": {"name": "f", "inptu": {}, "retries": 3}
        }]
    }));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::LikelyTypo, D, "tasks[0].for_each.ass"),
            row(C::LikelyTypo, D, "tasks[0].function.inptu"),
            row(C::UnknownKey, A, "tasks[0].function.retries"),
            row(C::LikelyTypo, D, "rollout.bucket_strat"),
        ]
    );
    let def = built(parsed);
    assert_eq!(def.header.ext.get("rollout.bucket_strat"), Some(&json!(5)));
    let ext = &task(&def.tasks[0]).header.ext;
    assert_eq!(ext.get("for_each.ass"), Some(&json!("q")));
    assert_eq!(ext.get("function.inptu"), Some(&json!({})));
    assert_eq!(ext.get("function.retries"), Some(&json!(3)));
}

#[test]
fn v3_keys_are_rewritten_under_compat_and_unknown_without() {
    // The compat reader's rules are `tests/compat.rs`'s; here, only that the
    // v3 keys never reach the walk as unknown keys under compat, and are
    // Rejected unknown keys without it.
    let json = json!({
        "id": "w", "continue_on_error": true,
        "tasks": [{"id": "t", "continue_on_error": true, "halt_on": "failure",
                   "function": {"name": "f"}}]
    });
    let parsed = compat(&json);
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::V3Spelling, A, "continue_on_error"),
            row(C::V3Spelling, A, "tasks[0].continue_on_error"),
            row(C::V3Spelling, A, "tasks[0].halt_on"),
        ]
    );
    let def = built(parsed);
    assert_eq!(def.on_failure, WorkflowFailurePolicy::Continue);
    let t = task(&def.tasks[0]);
    assert_eq!(
        (t.on_failure, t.on_rejection),
        (FailurePolicy::Halt, RejectionPolicy::Halt)
    );
    assert!(t.header.ext.is_empty());

    assert_eq!(
        rows(&strict(&json)),
        vec![
            row(C::UnknownKey, R, "tasks[0].continue_on_error"),
            row(C::UnknownKey, R, "tasks[0].halt_on"),
            row(C::UnknownKey, R, "continue_on_error"),
        ]
    );
}

#[test]
fn with_compat_off_every_unknown_key_is_rejected_and_nothing_moves() {
    for (json, expected) in [
        (
            wf(json!([{"id": "t", "conditon": false, "function": {"name": "f"}}])),
            row(C::LikelyTypo, R, "tasks[0].conditon"),
        ),
        (
            json!({"id": "w", "path": "ui/folder", "tasks": [leaf("t")]}),
            row(C::UnknownKey, R, "path"),
        ),
        (
            looped(json!({"maxx": 3, "max": 3})),
            row(C::LikelyTypo, R, "loop.maxx"),
        ),
        (
            fan_out(json!({"over": [], "as": "p", "zz": 1})),
            row(C::UnknownKey, R, "tasks[0].for_each.zz"),
        ),
    ] {
        let parsed = strict(&json);
        assert_eq!(rows(&parsed), vec![expected], "{json}");
        assert!(parsed.def.is_none());
    }
}

#[test]
fn ext_is_authored_and_moved_keys_join_it() {
    let def = def_of(&json!({
        "id": "w", "path": "p", "ext": {"owner": "ops"}, "tasks": [leaf("t")]
    }));
    assert_eq!(def.header.ext.len(), 2);
    assert_eq!(def.header.ext.get("owner"), Some(&json!("ops")));
    assert_eq!(def.header.ext.get("path"), Some(&json!("p")));

    // With compat off, an authored ext is the one home for host data.
    let parsed = strict(&json!({"id": "w", "ext": {"path": "p"}, "tasks": [leaf("t")]}));
    assert_eq!(rows(&parsed), vec![]);
    assert_eq!(
        parsed.def.unwrap().header.ext.get("path"),
        Some(&json!("p"))
    );
}

#[test]
fn an_unknown_key_already_in_ext_is_refused() {
    let parsed = compat(&json!({
        "id": "w", "path": "p", "ext": {"path": "q", "loop.maxx": 1},
        "loop": {"max": 3, "maxx": 2},
        "tasks": [{"id": "t", "zz": 1, "ext": {"zz": 2}, "function": {"name": "f"}}]
    }));
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::ParseFailed, R, "tasks[0].zz"),
            row(C::ParseFailed, R, "loop.maxx"),
            row(C::ParseFailed, R, "path"),
        ]
    );
    for issue in &parsed.issues {
        assert!(
            issue.message.contains("authored `ext`"),
            "{}",
            issue.message
        );
    }
    assert!(parsed.def.is_none());
}

#[test]
fn two_unknown_keys_moving_to_one_ext_name_refuse_the_second() {
    // The loop's `maxx` moves to `loop.maxx` (rule 5); the workflow's own key
    // spelled `loop.maxx` would land on the same name (rule 7). Neither came
    // from the authored `ext`.
    let parsed = compat(&json!({
        "id": "w", "loop.maxx": 1, "loop": {"max": 3, "maxx": 2}, "tasks": [leaf("t")]
    }));
    // Different paths, `loop` then `maxx` and one key: before phase 3 both
    // displayed as `loop.maxx`, and the one key is now quoted (I52).
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::LikelyTypo, D, "loop.maxx"),
            row(C::ParseFailed, R, r#"["loop.maxx"]"#),
        ]
    );
    assert_eq!(parsed.issues[0].path.segs().len(), 2);
    assert_eq!(parsed.issues[1].path.segs().len(), 1);
    let message = &parsed.issues[1].message;
    assert!(message.contains("already kept there"), "{message}");
    assert!(!message.contains("authored"), "{message}");
}

#[test]
fn a_hostile_unknown_key_costs_linear_time() {
    // A million-character key: far longer than every known key, so no
    // distance is computed and nothing quadratic is allocated.
    let key = "k".repeat(1_000_000);
    let mut json = json!({"id": "w", "tasks": [leaf("t")]});
    json[key.as_str()] = json!(1);
    let parsed = compat(&json);
    assert_eq!(parsed.issues.len(), 1);
    assert_eq!(parsed.issues[0].code, C::UnknownKey);
    assert_eq!(parsed.issues[0].severity, A);
    assert!(built(parsed).header.ext.contains_key(&key));
}

// ---------------------------------------------------------------------------
// Issue stamps
// ---------------------------------------------------------------------------

#[test]
fn every_issue_on_a_step_names_its_kind_and_id() {
    let parsed = compat(&json!({
        "id": "w",
        "priority": "x",
        "loop": {"max": 1, "setup": [{"id": "s", "function": "f"}]},
        "tasks": [
            {"id": "t", "terminal": 1, "function": {"name": "f"}},
            {"id": "g", "zz": 1, "tasks": [{"function": {"name": "f"}}]}
        ]
    }));
    let stamps: Vec<(String, Option<StepKind>, Option<&str>)> = parsed
        .issues
        .iter()
        .map(|i| (i.path.to_string(), i.step_kind, i.step_id.as_deref()))
        .collect();
    assert_eq!(
        stamps,
        vec![
            ("tasks[0].terminal".into(), Some(StepKind::Task), Some("t")),
            ("tasks[1].zz".into(), Some(StepKind::Group), Some("g")),
            ("tasks[1].tasks[0].id".into(), Some(StepKind::Task), None),
            (
                "loop.setup[0].function.name".into(),
                Some(StepKind::Setup),
                Some("s")
            ),
            ("priority".into(), None, None),
        ]
    );
    assert!(parsed.issues.iter().all(|i| i.field.is_none()));
}

// ---------------------------------------------------------------------------
// `walk`
// ---------------------------------------------------------------------------

#[test]
fn walk_yields_setup_first_then_tasks_pre_order_with_each_kind() {
    let def = def_of(&json!({
        "id": "w",
        "loop": {"max": 1, "setup": [leaf("s"), {"id": "sg", "tasks": [leaf("s2")]}]},
        "tasks": [
            leaf("a"),
            {"id": "g", "tasks": [leaf("b"), {"id": "h", "tasks": [leaf("c")]}]},
            leaf("d")
        ]
    }));
    let walked: Vec<(String, StepKind, &str)> = def
        .walk()
        .map(|(path, kind, step)| (path.to_string(), kind, step.id()))
        .collect();
    assert_eq!(
        walked,
        vec![
            ("loop.setup[0]".into(), StepKind::Setup, "s"),
            ("loop.setup[1]".into(), StepKind::Setup, "sg"),
            ("loop.setup[1].tasks[0]".into(), StepKind::Setup, "s2"),
            ("tasks[0]".into(), StepKind::Task, "a"),
            ("tasks[1]".into(), StepKind::Group, "g"),
            ("tasks[1].tasks[0]".into(), StepKind::Task, "b"),
            ("tasks[1].tasks[1]".into(), StepKind::Group, "h"),
            ("tasks[1].tasks[1].tasks[0]".into(), StepKind::Task, "c"),
            ("tasks[2]".into(), StepKind::Task, "d"),
        ]
    );
    // The walk's leaves are the steps' tasks, in order.
    let (_, _, step) = def.walk().nth(5).unwrap();
    assert_eq!(task(step).id, "b");
}
