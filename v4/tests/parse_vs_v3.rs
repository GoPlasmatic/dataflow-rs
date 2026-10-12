extern crate dataflow_rs_v4 as dataflow_rs;

// v4's `parse` against v3's `Workflow::validate_authored`, over hand-made
// definitions that each break a structural rule, or sit on its edge. Each
// side's structural issues (code, path, message and step id, in order) must
// be equal, and the two must agree on refusing.
//
// Not compared: `INVALID_MAPPING`, which v4 reports from `map`'s config;
// v3's type errors, which carry no path (v4 reports each at its key); and the
// compat reader's `V3_SPELLING`s, which report a rewrite v3 never needed. The
// corpus-wide comparison is the corpus driver's.

use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::program::IssueCode;
use dataflow_rs_v3::Workflow as V3;
use serde_json::{Value, json};

/// One structural issue: code, path, message and step id.
type Row = (String, String, String, Option<String>);

/// v3's issues that carry a path, except `INVALID_MAPPING`.
fn v3_rows(json: &Value) -> Vec<Row> {
    V3::validate_authored(json)
        .into_iter()
        .filter(|i| i.code.as_str() != "INVALID_MAPPING")
        .filter_map(|i| {
            let path = i.path?;
            Some((i.code.as_str().to_string(), path, i.message, i.task_id))
        })
        .collect()
}

/// Whether v3 refuses the definition, by any issue.
fn v3_refuses(json: &Value) -> bool {
    !V3::validate_authored(json).is_empty()
}

/// v4's issues at v3's structural codes: everything but `PARSE_FAILED`, the
/// unknown-key codes and the rewrites, which v3 has no counterpart for.
fn v4_rows(json: &Value) -> Vec<Row> {
    parse(json, ParseOptions::default())
        .issues
        .into_iter()
        .filter(|i| {
            !matches!(
                i.code,
                IssueCode::ParseFailed
                    | IssueCode::LikelyTypo
                    | IssueCode::UnknownKey
                    | IssueCode::V3Spelling
            )
        })
        .map(|i| {
            (
                i.code.as_str().to_string(),
                i.path.to_string(),
                i.message,
                i.step_id,
            )
        })
        .collect()
}

fn v4_refuses(json: &Value) -> bool {
    parse(json, ParseOptions::default()).def.is_none()
}

/// A task v3 loads: a name, and a custom function with an input.
fn t(id: &str) -> Value {
    json!({"id": id, "name": id, "function": {"name": "f", "input": {}}})
}

/// `t` with `extra` merged in.
fn t_with(id: &str, extra: Value) -> Value {
    let mut task = t(id);
    if let (Some(task), Some(extra)) = (task.as_object_mut(), extra.as_object()) {
        task.extend(extra.clone());
    }
    task
}

fn wf(tasks: Value) -> Value {
    json!({"id": "w", "name": "w", "tasks": tasks})
}

fn looped(config: Value) -> Value {
    json!({"id": "w", "name": "w", "loop": config, "tasks": [t("a")]})
}

/// A task fanning out over a custom function.
fn fan_out(for_each: Value) -> Value {
    wf(json!([t_with("a", json!({"for_each": for_each}))]))
}

/// `n` groups nested one inside the next; `deepest` is merged into the
/// innermost group.
fn nested(n: usize, deepest: Value) -> Value {
    let mut node = t("innermost");
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

fn agreeing() -> Vec<(&'static str, Value)> {
    vec![
        // Rules 1 to 3.
        ("no id", json!({"name": "w", "tasks": [t("a")]})),
        (
            "empty id",
            json!({"id": "", "name": "w", "tasks": [t("a")]}),
        ),
        (
            "numeric id",
            json!({"id": 5, "name": "w", "tasks": [t("a")]}),
        ),
        ("no tasks", json!({"id": "w", "name": "w"})),
        ("empty tasks", wf(json!([]))),
        ("tasks an object", wf(json!({}))),
        // Rule 4: ids.
        ("a step that is a number", wf(json!([5]))),
        ("a step that is null", wf(json!([null]))),
        (
            "no step id",
            wf(json!([{"name": "a", "function": {"name": "f", "input": {}}}])),
        ),
        ("duplicate ids", wf(json!([t("a"), t("a")]))),
        (
            "a group and a task share an id",
            wf(json!([{"id": "a", "tasks": [t("a")]}])),
        ),
        // `terminal`.
        (
            "terminal on_failure",
            wf(json!([t_with("a", json!({"terminal": "on_failure"}))])),
        ),
        (
            "terminal null",
            wf(json!([t_with("a", json!({"terminal": null}))])),
        ),
        (
            "a group's terminal",
            wf(json!([{"id": "g", "terminal": 1, "tasks": [t("a")]}])),
        ),
        // A group's `halt_on` and `for_each`, whatever their value.
        (
            "a group's halt_on",
            wf(json!([{"id": "g", "halt_on": "failure", "tasks": [t("a")]}])),
        ),
        (
            "a group's halt_on null",
            wf(json!([{"id": "g", "halt_on": null, "tasks": [t("a")]}])),
        ),
        (
            "a group's for_each",
            wf(json!([{"id": "g", "for_each": {"over": [], "as": "p"}, "tasks": [t("a")]}])),
        ),
        (
            "a group's for_each null",
            wf(json!([{"id": "g", "for_each": null, "tasks": [t("a")]}])),
        ),
        // The function.
        ("no function", wf(json!([{"id": "a", "name": "a"}]))),
        (
            "function null",
            wf(json!([{"id": "a", "name": "a", "function": null}])),
        ),
        (
            "function a string",
            wf(json!([{"id": "a", "name": "a", "function": "map"}])),
        ),
        (
            "function with an empty name",
            wf(json!([{"id": "a", "name": "a", "function": {"name": "", "input": {}}}])),
        ),
        // Groups.
        ("an empty group", wf(json!([{"id": "g", "tasks": []}]))),
        (
            "a group's tasks a string",
            wf(json!([{"id": "g", "tasks": "oops"}])),
        ),
        (
            "an empty group inside a group",
            wf(json!([{"id": "g", "tasks": [{"id": "inner", "tasks": []}]}])),
        ),
        ("eight levels", wf(nested(8, json!({})))),
        ("nine levels", wf(nested(9, json!({})))),
        (
            "nine levels, the deepest a duplicate",
            wf(nested(
                9,
                json!({"id": "g0", "terminal": "x", "for_each": {}}),
            )),
        ),
        // `for_each`, each rule.
        (
            "for_each on map",
            wf(
                json!([{"id": "a", "name": "a", "for_each": {"over": [], "as": "p"},
                       "function": {"name": "map", "input": {"mappings": []}}}]),
            ),
        ),
        (
            "for_each over a string",
            fan_out(json!({"over": "x", "as": "p"})),
        ),
        (
            "for_each over null",
            fan_out(json!({"over": null, "as": "p"})),
        ),
        (
            "for_each as a..b",
            fan_out(json!({"over": [], "as": "a..b"})),
        ),
        (
            "for_each max_concurrency 0",
            fan_out(json!({"over": [], "as": "p", "max_concurrency": 0})),
        ),
        (
            "for_each collect alone",
            fan_out(json!({"over": [], "as": "p", "collect": "temp_data.m"})),
        ),
        (
            "for_each into alone",
            fan_out(json!({"over": [], "as": "p", "into": "temp_data.m"})),
        ),
        (
            "for_each collect not a context path",
            fan_out(json!({"over": [], "as": "p", "collect": "m", "into": "temp_data.ms"})),
        ),
        (
            "for_each into under collect",
            fan_out(
                json!({"over": [], "as": "p", "collect": "temp_data.m", "into": "temp_data.m.x"}),
            ),
        ),
        (
            "for_each collect under the binding",
            fan_out(json!({"over": [], "as": "p", "collect": "temp_data.p.x", "into": "data.x"})),
        ),
        (
            "for_each into on the index",
            fan_out(
                json!({"over": [], "as": "p", "collect": "temp_data.m", "into": "temp_data.p_index"}),
            ),
        ),
        ("for_each null", fan_out(Value::Null)),
        ("for_each without over", fan_out(json!({"as": "p"}))),
        (
            "a valid for_each",
            fan_out(json!({"over": {"var": "data.ps"}, "as": "p"})),
        ),
        // `loop`, each rule.
        ("loop without max", looped(json!({}))),
        ("loop null", looped(Value::Null)),
        ("loop not an object", looped(json!(5))),
        ("loop bound empty", looped(json!({"max": 0}))),
        (
            "loop increment 0",
            looped(json!({"max": 3, "increment": 0})),
        ),
        (
            "loop increment and bound",
            looped(json!({"max": 0, "increment": -1})),
        ),
        (
            "loop counter a..b",
            looped(json!({"max": 3, "counter": "a..b"})),
        ),
        (
            "loop as and scratch",
            looped(json!({"max": 3, "over": [], "as": "", "scratch": "s."})),
        ),
        ("loop as without over", looped(json!({"max": 3, "as": "x"}))),
        (
            "loop as a number",
            looped(json!({"max": 3, "over": [], "as": 5})),
        ),
        ("loop over null", looped(json!({"max": 3, "over": null}))),
        (
            "loop over a number, init negative",
            looped(json!({"max": 3, "init": -2, "over": 4})),
        ),
        (
            "loop slots collide",
            looped(json!({"max": 3, "counter": "it", "as": "it.x", "scratch": "it", "over": []})),
        ),
        (
            "loop setup an object",
            looped(json!({"max": 3, "setup": {}})),
        ),
        // Setup.
        (
            "a setup id used by a task",
            json!({"id": "w", "name": "w", "loop": {"max": 3, "setup": [t("a")]}, "tasks": [t("a")]}),
        ),
        (
            "broken setup steps",
            looped(json!({"max": 3, "setup": [{"id": "s"}, {"id": "g", "tasks": []}]})),
        ),
        // Several rules at once, in order.
        (
            "everything at once",
            json!({"id": "", "name": "w", "loop": {"max": 0, "setup": [{"name": "s"}]},
                   "tasks": [{"id": "a", "terminal": 1}, {"id": "a", "function": {"name": ""}}]}),
        ),
        // Rule 7, and nulls v3 reads as absent.
        (
            "priority high",
            json!({"id": "w", "name": "w", "priority": "high", "tasks": [t("a")]}),
        ),
        (
            "a rollout without its end",
            json!({"id": "w", "name": "w", "rollout": {"bucket_start": 0}, "tasks": [t("a")]}),
        ),
        (
            "rollout null",
            json!({"id": "w", "name": "w", "rollout": null, "tasks": [t("a")]}),
        ),
        (
            "a tag that is a number",
            json!({"id": "w", "name": "w", "tags": [1], "tasks": [t("a")]}),
        ),
        (
            "status live",
            json!({"id": "w", "name": "w", "status": "live", "tasks": [t("a")]}),
        ),
        (
            "channel null",
            json!({"id": "w", "name": "w", "channel": null, "tasks": [t("a")]}),
        ),
        (
            "description null",
            json!({"id": "w", "name": "w", "description": null, "tasks": [t("a")]}),
        ),
        (
            "a task's description null",
            wf(json!([t_with("a", json!({"description": null}))])),
        ),
        (
            "created_at, three spellings v3 loads",
            json!({"id": "w", "name": "w", "created_at": "2026-01-02 03:04:05 UTC",
                   "updated_at": "2026-01-02T03:04:05+0000", "tasks": [t("a")]}),
        ),
        (
            "created_at yesterday",
            json!({"id": "w", "name": "w", "created_at": "yesterday", "tasks": [t("a")]}),
        ),
        // The compat reader: what v3 loads loads, and what it refuses is
        // refused, at v3's code and path where v3 gives one.
        (
            "a task's halt_on of an unknown value",
            wf(json!([t_with("a", json!({"halt_on": "sometimes"}))])),
        ),
        (
            "a task's halt_on null",
            wf(json!([t_with("a", json!({"halt_on": null}))])),
        ),
        (
            "a task's halt_on and continue_on_error",
            wf(json!([t_with(
                "a",
                json!({"halt_on": "failure", "continue_on_error": true})
            )])),
        ),
        (
            "a task's halt_on never and continue_on_error false",
            wf(json!([t_with(
                "a",
                json!({"halt_on": "never", "continue_on_error": false})
            )])),
        ),
        (
            "a task's continue_on_error a string",
            wf(json!([t_with("a", json!({"continue_on_error": "yes"}))])),
        ),
        (
            "a workflow's continue_on_error",
            json!({"id": "w", "name": "w", "continue_on_error": true, "tasks": [t("a")]}),
        ),
        (
            "a workflow's continue_on_error null",
            json!({"id": "w", "name": "w", "continue_on_error": null, "tasks": [t("a")]}),
        ),
        (
            "validate",
            wf(json!([{"id": "a", "name": "a",
                       "function": {"name": "validate", "input": {"rules": []}}}])),
        ),
        (
            "the pre-3.9 aliases",
            wf(json!([
                {"id": "call", "name": "call", "function": {"name": "http_call", "input": {
                    "connector": "c", "path_logic": {"var": "data.id"},
                    "body_logic": {"var": "data.obj"}, "output": "data.resp"}}},
                {"id": "kafka", "name": "kafka", "function": {"name": "publish_kafka", "input": {
                    "connector": "c", "topic": "t", "key_logic": {"var": "data.id"},
                    "value_logic": {"var": "data.obj"}}}}
            ])),
        ),
        (
            "an alias beside its target",
            wf(
                json!([{"id": "call", "name": "call", "function": {"name": "http_call",
                       "input": {"connector": "c", "path": "/a", "path_logic": "/b"}}}]),
            ),
        ),
        (
            "a stray publish_json input key",
            wf(
                json!([{"id": "p", "name": "p", "function": {"name": "publish_json",
                       "input": {"source": "s", "target": "t", "targte": "u"}}}]),
            ),
        ),
        (
            "a group's continue_on_error false",
            wf(json!([{"id": "g", "continue_on_error": false, "tasks": [t("a")]}])),
        ),
    ]
}

#[test]
fn every_structural_rule_reports_as_v3_does() {
    let mut with_issues = 0;
    for (label, json) in agreeing() {
        let v3 = v3_rows(&json);
        with_issues += usize::from(!v3.is_empty());
        assert_eq!(v4_rows(&json), v3, "{label}");
        assert_eq!(v4_refuses(&json), v3_refuses(&json), "{label}: refusal");
    }
    // The rest pin v3's silences: nulls read as absent, valid shapes.
    assert!(
        with_issues >= 50,
        "only {with_issues} fixtures exercise one of v3's rules"
    );
}

/// `(code, path)`, the part of a row each side's divergence is stated in.
fn codes(rows: Vec<Row>) -> Vec<(String, String)> {
    rows.into_iter().map(|(c, p, _, _)| (c, p)).collect()
}

fn r(code: &str, path: &str) -> (String, String) {
    (code.to_string(), path.to_string())
}

#[test]
fn where_v4_differs_from_v3_it_is_deliberate() {
    let deepest = {
        let mut path = "tasks[0]".to_string();
        for _ in 0..8 {
            path.push_str(".tasks[0]");
        }
        path
    };
    for (label, json, v3, v4, refusals, why) in [
        (
            "for_each on a task with no function",
            wf(json!([{"id": "a", "name": "a", "for_each": {"over": 3, "as": "p"}}])),
            vec![r("MISSING_FUNCTION", "tasks[0].function")],
            vec![
                r("INVALID_FOR_EACH", "tasks[0].for_each.over"),
                r("MISSING_FUNCTION", "tasks[0].function"),
            ],
            (true, true),
            "v3 asks the for_each rules only of a function that deserializes; v4's do not \
             need one",
        ),
        (
            "for_each on map with an input map's config refuses",
            wf(
                json!([{"id": "a", "name": "a", "for_each": {"over": [], "as": "p"},
                       "function": {"name": "map", "input": {}}}]),
            ),
            vec![],
            vec![r("INVALID_FOR_EACH", "tasks[0].for_each")],
            (true, true),
            "I25: the built-in names are reserved, so the name alone refuses (v3: a pathless \
             PARSE_FAILED)",
        ),
        (
            "a function without input",
            wf(json!([{"id": "a", "name": "a", "function": {"name": "f"}}])),
            vec![],
            vec![],
            (true, false),
            "I25: input defaults to {} (v3: missing field `input`)",
        ),
        (
            "a group nested too deep carrying halt_on",
            wf(nested(9, json!({"halt_on": "failure"}))),
            vec![r("GROUP_TOO_DEEP", &deepest)],
            vec![
                r("INVALID_HALT_ON", &format!("{deepest}.halt_on")),
                r("GROUP_TOO_DEEP", &deepest),
            ],
            (true, true),
            "v3's halt_on rule asks for StepKind::Group, and a group too deep is TooDeep; \
             halt_on on any group is refused",
        ),
        (
            "a task's halt_on of an unknown value after an empty workflow id",
            json!({"id": "", "name": "w", "tasks": [t_with("a", json!({"halt_on": "x"}))]}),
            vec![
                r("EMPTY_WORKFLOW_ID", "id"),
                r("INVALID_HALT_ON", "tasks[0].halt_on"),
            ],
            vec![
                r("INVALID_HALT_ON", "tasks[0].halt_on"),
                r("EMPTY_WORKFLOW_ID", "id"),
            ],
            (true, true),
            "the compat reader runs before the walk, so its refusals come first; v3 checks a \
             task's halt_on in its step walk",
        ),
        (
            "a group's continue_on_error true",
            wf(json!([{"id": "g", "continue_on_error": true, "tasks": [t("a")]}])),
            vec![],
            vec![r("GROUP_CONTINUE_ON_ERROR", "tasks[0].continue_on_error")],
            (false, false),
            "v3 reports it from check_workflow, not validate_authored, at the bare key \
             `continue_on_error`; v4's one walk reports it at its path",
        ),
    ] {
        assert_eq!(codes(v3_rows(&json)), v3, "{label}: v3 ({why})");
        assert_eq!(codes(v4_rows(&json)), v4, "{label}: v4 ({why})");
        assert_eq!(
            (v3_refuses(&json), v4_refuses(&json)),
            refusals,
            "{label}: refusals ({why})"
        );
    }
}

#[test]
fn for_each_on_validate_is_refused_as_v3_refuses_it_naming_v4s_built_in() {
    // The compat reader rewrites `validate` before the walk, so the refusal
    // is v3's at v3's path. Its message names the built-in by v4's name,
    // `validation`; v3's names its own, `validate`, for either spelling.
    let json = wf(
        json!([{"id": "a", "name": "a", "for_each": {"over": [], "as": "p"},
                          "function": {"name": "validate", "input": {"rules": []}}}]),
    );
    let (v3, v4) = (v3_rows(&json), v4_rows(&json));
    assert_eq!(codes(v4.clone()), codes(v3.clone()));
    assert_eq!(
        codes(v4.clone()),
        vec![r("INVALID_FOR_EACH", "tasks[0].for_each")]
    );
    assert_eq!(v4[0].3, v3[0].3, "step id");
    assert_eq!(v4[0].2, v3[0].2.replace("'validate'", "'validation'"));
    assert_eq!((v3_refuses(&json), v4_refuses(&json)), (true, true));
}
