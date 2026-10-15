extern crate dataflow_rs_v4 as dataflow_rs;

// The compat reader: every v3 spelling rewritten into v4's before `parse`'s
// walk, each rewrite reported as a `V3_SPELLING` at the v3 key; the
// definition types' `Serialize`, which writes the v4 spelling; and
// `migrate_v3`, whose output parses with the reader off to an equal
// definition.

use dataflow_rs::compat::{Migrated, migrate_v3};
use dataflow_rs::def::{
    FailurePolicy, GroupDef, ParseOptions, Parsed, RejectionPolicy, StepDef, TaskDef, WorkflowDef,
    WorkflowFailurePolicy, parse,
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

fn compat(json: &Value) -> Parsed {
    parse(json, ParseOptions::default())
}

fn strict(json: &Value) -> Parsed {
    parse(json, ParseOptions::default().compat(false))
}

fn wf(tasks: Value) -> Value {
    json!({"id": "w", "tasks": tasks})
}

/// A task `t` running a custom function, with `extra` merged in.
fn task_with(extra: Value) -> Value {
    let mut task = json!({"id": "t", "function": {"name": "f"}});
    if let (Some(task), Some(extra)) = (task.as_object_mut(), extra.as_object()) {
        task.extend(extra.clone());
    }
    task
}

/// A workflow whose one task `t` runs `name` with `input`.
fn calling(name: &str, input: Value) -> Value {
    wf(json!([{"id": "t", "function": {"name": name, "input": input}}]))
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

/// `(path, v3, v4)` of each rewrite `migrate_v3` reports.
fn rewrites(migrated: &Migrated) -> Vec<(String, String, String)> {
    migrated
        .rewrites
        .iter()
        .map(|r| (r.path.to_string(), r.v3.clone(), r.v4.clone()))
        .collect()
}

fn rewrite(path: &str, v3: &str, v4: &str) -> (String, String, String) {
    (path.to_string(), v3.to_string(), v4.to_string())
}

fn migrated(json: &Value) -> Migrated {
    migrate_v3(json).unwrap_or_else(|issues| panic!("{json}: {issues:?}"))
}

// ---------------------------------------------------------------------------
// Rewrites
// ---------------------------------------------------------------------------

#[test]
fn a_workflows_continue_on_error_is_its_on_failure() {
    for (flag, policy, v4) in [
        (
            true,
            WorkflowFailurePolicy::Continue,
            "on_failure: continue",
        ),
        (false, WorkflowFailurePolicy::Fail, "on_failure: fail"),
    ] {
        let json = json!({"id": "w", "continue_on_error": flag, "tasks": [task_with(json!({}))]});
        let parsed = compat(&json);
        assert_eq!(
            rows(&parsed),
            vec![row(C::V3Spelling, A, "continue_on_error")]
        );
        assert_eq!(parsed.issues[0].step_kind, None);
        let def = built(parsed);
        assert_eq!(def.on_failure, policy);
        assert!(def.header.ext.is_empty(), "nothing moves: {:?}", def.header);
        assert_eq!(
            rewrites(&migrated(&json)),
            vec![rewrite(
                "continue_on_error",
                &format!("continue_on_error: {flag}"),
                v4
            )]
        );
    }
}

#[test]
fn a_tasks_two_flags_land_as_the_policy_table_says() {
    use FailurePolicy as F;
    use RejectionPolicy as J;
    // 06's policy table, row by row, with the two default spellings that are
    // still reported.
    for (flags, policies, reported, v4) in [
        (json!({}), (F::Fail, J::Continue), vec![], ""),
        (
            json!({"continue_on_error": true}),
            (F::Continue, J::Continue),
            vec!["continue_on_error"],
            "on_failure: continue, on_rejection: continue",
        ),
        (
            json!({"halt_on": "failure"}),
            (F::Fail, J::Halt),
            vec!["halt_on"],
            "on_failure: fail, on_rejection: halt",
        ),
        (
            json!({"continue_on_error": true, "halt_on": "failure"}),
            (F::Halt, J::Halt),
            vec!["continue_on_error", "halt_on"],
            "on_failure: halt, on_rejection: halt",
        ),
        (
            json!({"halt_on": "never"}),
            (F::Fail, J::Continue),
            vec!["halt_on"],
            "on_failure: fail, on_rejection: continue",
        ),
        (
            json!({"continue_on_error": false}),
            (F::Fail, J::Continue),
            vec!["continue_on_error"],
            "on_failure: fail, on_rejection: continue",
        ),
        (
            json!({"continue_on_error": false, "halt_on": "never"}),
            (F::Fail, J::Continue),
            vec!["continue_on_error", "halt_on"],
            "on_failure: fail, on_rejection: continue",
        ),
    ] {
        let json = wf(json!([task_with(flags.clone())]));
        let parsed = compat(&json);
        let expected: Vec<Row> = reported
            .iter()
            .map(|key| row(C::V3Spelling, A, &format!("tasks[0].{key}")))
            .collect();
        assert_eq!(rows(&parsed), expected, "{flags}");
        for issue in &parsed.issues {
            assert_eq!(issue.step_kind, Some(StepKind::Task), "{flags}");
            assert_eq!(issue.step_id.as_deref(), Some("t"), "{flags}");
            assert_eq!(issue.field, None, "{flags}");
        }
        let def = built(parsed);
        let t = task(&def.tasks[0]);
        assert_eq!((t.on_failure, t.on_rejection), policies, "{flags}");
        assert!(t.header.ext.is_empty(), "{flags}: {:?}", t.header.ext);

        let expected: Vec<_> = reported
            .iter()
            .map(|key| {
                let value = &flags[*key];
                let v3 = match value {
                    Value::String(text) => format!("{key}: {text}"),
                    other => format!("{key}: {other}"),
                };
                rewrite(&format!("tasks[0].{key}"), &v3, v4)
            })
            .collect();
        assert_eq!(rewrites(&migrated(&json)), expected, "{flags}");
    }
}

#[test]
fn a_v3_policy_beside_any_v4_policy_is_a_parse_failure_at_the_v3_key() {
    // One policy, two spellings: which wins would be a guess, so neither
    // does. Each v3 key is refused, naming the v4 key beside it.
    for (flags, refused) in [
        (
            json!({"halt_on": "failure", "on_failure": "continue"}),
            vec![("halt_on", "on_failure")],
        ),
        (
            json!({"continue_on_error": true, "on_rejection": "halt"}),
            vec![("continue_on_error", "on_rejection")],
        ),
        (
            json!({"continue_on_error": false, "on_failure": "halt"}),
            vec![("continue_on_error", "on_failure")],
        ),
        (
            json!({"halt_on": "never", "on_rejection": "halt"}),
            vec![("halt_on", "on_rejection")],
        ),
        (
            json!({"continue_on_error": true, "halt_on": "failure", "on_rejection": "halt"}),
            vec![
                ("continue_on_error", "on_rejection"),
                ("halt_on", "on_rejection"),
            ],
        ),
    ] {
        let json = wf(json!([task_with(flags.clone())]));
        let parsed = compat(&json);
        let expected: Vec<Row> = refused
            .iter()
            .map(|(v3, _)| row(C::ParseFailed, R, &format!("tasks[0].{v3}")))
            .collect();
        assert_eq!(rows(&parsed), expected, "{flags}");
        for (issue, (v3, v4)) in parsed.issues.iter().zip(&refused) {
            assert_eq!(
                issue.message,
                format!("v3's `{v3}` and v4's `{v4}` on one task; write only the v4 policies"),
                "{flags}"
            );
            assert_eq!(issue.step_id.as_deref(), Some("t"), "{flags}");
        }
        assert!(parsed.def.is_none(), "{flags}");
        assert!(migrate_v3(&json).is_err(), "{flags}");
    }

    // Both v4 keys are named.
    let json = wf(json!([task_with(
        json!({"halt_on": "failure", "on_failure": "fail", "on_rejection": "halt"})
    )]));
    let parsed = compat(&json);
    assert_eq!(
        rows(&parsed),
        vec![row(C::ParseFailed, R, "tasks[0].halt_on")]
    );
    assert_eq!(
        parsed.issues[0].message,
        "v3's `halt_on` and v4's `on_failure` and `on_rejection` on one task; write only the \
         v4 policies"
    );

    // A workflow's, likewise.
    let json = json!({"id": "w", "continue_on_error": false, "on_failure": "fail",
                      "tasks": [task_with(json!({}))]});
    let parsed = compat(&json);
    assert_eq!(
        rows(&parsed),
        vec![row(C::ParseFailed, R, "continue_on_error")]
    );
    assert_eq!(
        parsed.issues[0].message,
        "v3's `continue_on_error` and v4's `on_failure` on one workflow; write only the v4 \
         policy"
    );
}

#[test]
fn a_setup_step_and_a_group_member_are_rewritten_too() {
    let json = json!({
        "id": "w",
        "loop": {"max": 1, "setup": [task_with(json!({"id": "s", "halt_on": "failure"}))]},
        "tasks": [{"id": "g", "tasks": [task_with(json!({"continue_on_error": true}))]}]
    });
    let parsed = compat(&json);
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::V3Spelling, A, "loop.setup[0].halt_on"),
            row(C::V3Spelling, A, "tasks[0].tasks[0].continue_on_error"),
        ]
    );
    assert_eq!(parsed.issues[0].step_kind, Some(StepKind::Setup));
    assert_eq!(parsed.issues[0].step_id.as_deref(), Some("s"));
    assert_eq!(parsed.issues[1].step_kind, Some(StepKind::Task));
    let def = built(parsed);
    let setup = &def.r#loop.as_ref().unwrap().setup;
    assert_eq!(task(&setup[0]).on_rejection, RejectionPolicy::Halt);
    let member = task(&group(&def.tasks[0]).tasks[0]);
    assert_eq!(member.on_failure, FailurePolicy::Continue);
}

#[test]
fn validate_is_read_as_validation() {
    let json = calling("validate", json!({"rules": []}));
    let parsed = compat(&json);
    assert_eq!(
        rows(&parsed),
        vec![row(C::V3Spelling, A, "tasks[0].function.name")]
    );
    assert_eq!(parsed.issues[0].field, None, "not under input");
    assert_eq!(parsed.issues[0].step_id.as_deref(), Some("t"));
    let def = built(parsed);
    assert_eq!(task(&def.tasks[0]).function.name, "validation");
    assert_eq!(
        rewrites(&migrated(&json)),
        vec![rewrite("tasks[0].function.name", "validate", "validation")]
    );
}

#[test]
fn for_each_on_validate_is_refused_after_the_rewrite() {
    let json = wf(json!([{"id": "t", "for_each": {"over": [], "as": "p"},
                          "function": {"name": "validate", "input": {"rules": []}}}]));
    let parsed = compat(&json);
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::V3Spelling, A, "tasks[0].function.name"),
            row(C::InvalidForEach, R, "tasks[0].for_each"),
        ]
    );
}

#[test]
fn the_integration_aliases_are_rewritten_by_function_name() {
    for (name, input, aliases, rewritten) in [
        (
            "http_call",
            json!({"connector": "c", "body_logic": {"var": "data.obj"},
                   "output": "data.resp", "path_logic": {"var": "data.id"}}),
            vec![
                ("body_logic", "body"),
                ("output", "response_path"),
                ("path_logic", "path"),
            ],
            json!({"connector": "c", "path": {"var": "data.id"},
                   "body": {"var": "data.obj"}, "response_path": "data.resp"}),
        ),
        (
            "enrich",
            json!({"connector": "c", "merge_path": "data.out", "path_logic": "/x"}),
            vec![("path_logic", "path")],
            json!({"connector": "c", "merge_path": "data.out", "path": "/x"}),
        ),
        (
            "publish_kafka",
            json!({"connector": "c", "topic": "t", "key_logic": {"var": "data.id"},
                   "value_logic": {"var": "data.obj"}}),
            vec![("key_logic", "key"), ("value_logic", "value")],
            json!({"connector": "c", "topic": "t", "key": {"var": "data.id"},
                   "value": {"var": "data.obj"}}),
        ),
    ] {
        let json = calling(name, input);
        let parsed = compat(&json);
        // In the input's key order, which the inputs above spell sorted, as `aliases` does.
        let expected: Vec<Row> = aliases
            .iter()
            .map(|(alias, _)| {
                row(
                    C::V3Spelling,
                    A,
                    &format!("tasks[0].function.input.{alias}"),
                )
            })
            .collect();
        assert_eq!(rows(&parsed), expected, "{name}");
        for (issue, (alias, _)) in parsed.issues.iter().zip(&aliases) {
            assert_eq!(issue.field.as_deref(), Some(*alias), "{name}");
            assert_eq!(issue.step_id.as_deref(), Some("t"), "{name}");
        }
        let def = built(parsed);
        assert_eq!(task(&def.tasks[0]).function.input, rewritten, "{name}");
        let expected: Vec<_> = aliases
            .iter()
            .map(|(alias, target)| {
                rewrite(&format!("tasks[0].function.input.{alias}"), alias, target)
            })
            .collect();
        assert_eq!(rewrites(&migrated(&json)), expected, "{name}");
    }
}

#[test]
fn an_alias_of_another_function_is_left_alone() {
    // The aliases are per function: `output` is http_call's, not a custom
    // function's, and `path_logic` is not publish_kafka's.
    for json in [
        calling("f", json!({"output": "data.x", "path_logic": 1})),
        calling(
            "publish_kafka",
            json!({"connector": "c", "topic": "t", "path_logic": 1}),
        ),
    ] {
        let parsed = compat(&json);
        assert_eq!(rows(&parsed), vec![], "{json}");
        let def = built(parsed);
        assert_eq!(
            task(&def.tasks[0]).function.input,
            json["tasks"][0]["function"]["input"]
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[test]
fn both_spellings_at_once_is_a_parse_failure_at_the_v3_key() {
    for (json, path) in [
        (
            calling(
                "http_call",
                json!({"connector": "c", "path": "/a", "path_logic": "/b"}),
            ),
            "tasks[0].function.input.path_logic",
        ),
        (
            calling(
                "http_call",
                json!({"connector": "c", "output": "data.a",
                                        "response_path": "data.b"}),
            ),
            "tasks[0].function.input.output",
        ),
        (
            calling(
                "publish_kafka",
                json!({"connector": "c", "topic": "t", "value": 1,
                                            "value_logic": 2}),
            ),
            "tasks[0].function.input.value_logic",
        ),
    ] {
        let parsed = compat(&json);
        assert_eq!(rows(&parsed), vec![row(C::ParseFailed, R, path)], "{json}");
        assert!(
            parsed.issues[0].message.contains("both"),
            "{}",
            parsed.issues[0].message
        );
        assert!(parsed.def.is_none());
    }
}

#[test]
fn a_continue_on_error_that_is_not_a_bool_is_a_parse_failure() {
    for json in [
        json!({"id": "w", "continue_on_error": "yes", "tasks": [task_with(json!({}))]}),
        wf(json!([task_with(json!({"continue_on_error": null}))])),
    ] {
        let parsed = compat(&json);
        let path = if json.get("continue_on_error").is_some() {
            "continue_on_error"
        } else {
            "tasks[0].continue_on_error"
        };
        assert_eq!(rows(&parsed), vec![row(C::ParseFailed, R, path)], "{json}");
        assert!(parsed.def.is_none());
    }
}

#[test]
fn a_halt_on_that_is_not_never_or_failure_is_invalid_halt_on() {
    for value in [
        json!("sometimes"),
        json!(null),
        json!(true),
        json!("Failure"),
    ] {
        let parsed = compat(&wf(json!([task_with(json!({"halt_on": value}))])));
        assert_eq!(
            rows(&parsed),
            vec![row(C::InvalidHaltOn, R, "tasks[0].halt_on")],
            "{value}"
        );
        // v3's message.
        assert_eq!(
            parsed.issues[0].message,
            "halt_on must be one of \"never\", \"failure\""
        );
        assert_eq!(parsed.issues[0].step_id.as_deref(), Some("t"));
        assert!(parsed.def.is_none());
    }
}

#[test]
fn a_groups_halt_on_is_still_the_walks_refusal() {
    let parsed = compat(&wf(json!([{"id": "g", "halt_on": "failure",
                                    "tasks": [task_with(json!({}))]}])));
    assert_eq!(
        rows(&parsed),
        vec![row(C::InvalidHaltOn, R, "tasks[0].halt_on")]
    );
    assert!(parsed.issues[0].message.contains("group"));
}

// ---------------------------------------------------------------------------
// Publish inputs
// ---------------------------------------------------------------------------

#[test]
fn a_stray_publish_input_key_moves_to_the_tasks_ext() {
    for name in ["publish_json", "publish_xml"] {
        let json = calling(
            name,
            json!({"source": "obj", "target": "out", "pretyy": true, "zzz": 1}),
        );
        let parsed = compat(&json);
        assert_eq!(
            rows(&parsed),
            vec![
                row(C::LikelyTypo, D, "tasks[0].function.input.pretyy"),
                row(C::UnknownKey, A, "tasks[0].function.input.zzz"),
            ],
            "{name}"
        );
        assert!(
            parsed.issues[0].message.contains("`pretty`"),
            "{}",
            parsed.issues[0].message
        );
        assert_eq!(parsed.issues[0].field.as_deref(), Some("pretyy"));
        assert_eq!(parsed.issues[1].field.as_deref(), Some("zzz"));
        assert_eq!(parsed.issues[0].step_id.as_deref(), Some("t"));
        let def = built(parsed);
        let t = task(&def.tasks[0]);
        assert_eq!(
            t.function.input,
            json!({"source": "obj", "target": "out"}),
            "{name}"
        );
        assert_eq!(
            t.header.ext.get("function.input.pretyy"),
            Some(&json!(true))
        );
        assert_eq!(t.header.ext.get("function.input.zzz"), Some(&json!(1)));
        // A move is not a rewrite.
        assert_eq!(rewrites(&migrated(&json)), vec![]);
    }
}

/// A minimal input each built-in's config accepts.
fn minimal(name: &str) -> Value {
    match name {
        "map" => json!({"mappings": [{"path": "data.x", "logic": 1}]}),
        "validation" | "validate" => json!({"rules": [{"logic": true, "message": "m"}]}),
        "filter" => json!({"condition": true}),
        "log" => json!({"message": "m"}),
        "parse_json" | "parse_xml" => json!({"source": "payload", "target": "in"}),
        _ => json!({"source": "obj", "target": "out"}),
    }
}

#[test]
fn a_stray_key_in_any_builtin_input_moves_to_the_tasks_ext() {
    // I54: v3's configs ignored a key they did not know; `validate`'s input
    // is `validation`'s.
    for name in [
        "map",
        "validation",
        "validate",
        "filter",
        "log",
        "parse_json",
        "parse_xml",
        "publish_json",
        "publish_xml",
    ] {
        let mut input = minimal(name);
        input["zzz"] = json!(1);
        let parsed = compat(&calling(name, input));
        let issues: Vec<Row> = rows(&parsed)
            .into_iter()
            .filter(|(code, _, _)| *code != C::V3Spelling)
            .collect();
        assert_eq!(
            issues,
            vec![row(C::UnknownKey, A, "tasks[0].function.input.zzz")],
            "{name}"
        );
        let stray = parsed
            .issues
            .iter()
            .find(|i| i.code == C::UnknownKey)
            .unwrap();
        assert!(
            stray
                .message
                .ends_with("(kept in `ext` as `function.input.zzz`)"),
            "{}",
            stray.message
        );
        assert_eq!(stray.field.as_deref(), Some("zzz"));
        let def = built(parsed);
        let t = task(&def.tasks[0]);
        assert_eq!(t.function.input, minimal(name), "{name}");
        assert_eq!(t.header.ext.get("function.input.zzz"), Some(&json!(1)));
    }
}

#[test]
fn an_unknown_key_of_a_mapping_or_a_rule_moves_to_the_tasks_ext() {
    let input = json!({"aaa": 0, "mappings": [
        {"path": "data.x", "logic": 1, "lgic": 2},
        5,
        {"path": "data.y", "logic": 2, "note": "n"}
    ], "zzz": 1});
    let parsed = compat(&calling("map", input));
    // In document order: the input's keys as it holds them, each element's
    // within its key.
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::UnknownKey, A, "tasks[0].function.input.aaa"),
            row(C::LikelyTypo, D, "tasks[0].function.input.mappings[0].lgic"),
            row(C::UnknownKey, A, "tasks[0].function.input.mappings[2].note"),
            row(C::UnknownKey, A, "tasks[0].function.input.zzz"),
        ]
    );
    assert!(parsed.issues[1].message.contains("`logic`"));
    assert!(
        parsed.issues[1]
            .message
            .ends_with("(kept in `ext` as `function.input.mappings[0].lgic`)")
    );
    assert_eq!(parsed.issues[1].field.as_deref(), Some("mappings"));
    let def = built(parsed);
    let t = task(&def.tasks[0]);
    // An element that is not an object is the config's to refuse.
    assert_eq!(
        t.function.input,
        json!({"mappings": [{"path": "data.x", "logic": 1}, 5,
                            {"path": "data.y", "logic": 2}]})
    );
    let ext = &t.header.ext;
    assert_eq!(ext.len(), 4);
    assert_eq!(ext.get("function.input.mappings[0].lgic"), Some(&json!(2)));
    assert_eq!(
        ext.get("function.input.mappings[2].note"),
        Some(&json!("n"))
    );

    let parsed = compat(&calling(
        "validation",
        json!({"rules": [{"logic": true, "message": "m", "code": "E1"}]}),
    ));
    assert_eq!(
        rows(&parsed),
        vec![row(
            C::UnknownKey,
            A,
            "tasks[0].function.input.rules[0].code"
        )]
    );
    let def = built(parsed);
    assert_eq!(
        task(&def.tasks[0])
            .header
            .ext
            .get("function.input.rules[0].code"),
        Some(&json!("E1"))
    );
}

#[test]
fn a_stray_key_that_needs_quoting_is_named_as_its_path_displays() {
    let parsed = compat(&calling("log", json!({"message": "m", "a.b": 1})));
    assert_eq!(
        rows(&parsed),
        vec![row(C::UnknownKey, A, "tasks[0].function.input[\"a.b\"]")]
    );
    let def = built(parsed);
    assert_eq!(
        task(&def.tasks[0])
            .header
            .ext
            .get("function.input[\"a.b\"]"),
        Some(&json!(1))
    );
}

#[test]
fn without_the_reader_a_builtin_inputs_stray_keys_stay_for_its_config() {
    let input = json!({"mappings": [{"path": "data.x", "logic": 1, "note": "n"}], "zzz": 1});
    let parsed = strict(&calling("map", input.clone()));
    assert_eq!(rows(&parsed), vec![]);
    let def = built(parsed);
    let t = task(&def.tasks[0]);
    assert_eq!(t.function.input, input);
    assert!(t.header.ext.is_empty());

    // Nor is an input that is not an object touched with it on.
    let parsed = compat(&calling("filter", json!([1])));
    assert_eq!(rows(&parsed), vec![]);
    assert_eq!(task(&built(parsed).tasks[0]).function.input, json!([1]));
}

#[test]
fn nested_strays_migrate_to_a_definition_that_parses_with_compat_off() {
    let json = calling(
        "validation",
        json!({"rules": [{"logic": true, "message": "m", "code": "E1"}], "strict": true}),
    );
    let m = migrated(&json);
    // A move is not a rewrite.
    assert_eq!(rewrites(&m), vec![]);
    let reparsed = strict(&m.json);
    assert_eq!(rows(&reparsed), vec![]);
    assert_eq!(reparsed.def, compat(&json).def);
}

#[test]
fn the_four_publish_keys_stay_in_the_input() {
    let input = json!({"source": "obj", "target": "out", "pretty": true, "root_element": "doc"});
    let parsed = compat(&calling("publish_xml", input.clone()));
    assert_eq!(rows(&parsed), vec![]);
    assert_eq!(task(&built(parsed).tasks[0]).function.input, input);
}

#[test]
fn a_stray_publish_key_joins_an_authored_ext_and_refuses_a_collision() {
    let json = wf(json!([{"id": "t", "ext": {"owner": "ops"},
                          "function": {"name": "publish_json",
                                       "input": {"source": "s", "target": "t", "x": 1}}}]));
    let def = built(compat(&json));
    let ext = &task(&def.tasks[0]).header.ext;
    assert_eq!(ext.len(), 2);
    assert_eq!(ext.get("owner"), Some(&json!("ops")));
    assert_eq!(ext.get("function.input.x"), Some(&json!(1)));

    let json = wf(json!([{"id": "t", "ext": {"function.input.x": 0},
                          "function": {"name": "publish_json",
                                       "input": {"source": "s", "target": "t", "x": 1}}}]));
    let parsed = compat(&json);
    assert_eq!(
        rows(&parsed),
        vec![row(C::ParseFailed, R, "tasks[0].function.input.x")]
    );
    assert_eq!(
        parsed.issues[0].message,
        "`x` appears both on the node and in its authored `ext`, as `function.input.x`"
    );
    assert!(parsed.def.is_none());
}

#[test]
fn a_key_the_walk_moves_onto_one_the_reader_moved_names_the_first() {
    // The reader moves the stray `x` to `function.input.x`; the task's own
    // unknown key of that name would land on it. Neither is authored `ext`.
    let json = wf(json!([{"id": "t", "function.input.x": 2,
                          "function": {"name": "publish_json",
                                       "input": {"source": "s", "target": "t", "x": 1}}}]));
    let parsed = compat(&json);
    assert_eq!(
        rows(&parsed)
            .into_iter()
            .map(|(code, severity, _)| (code, severity))
            .collect::<Vec<_>>(),
        vec![(C::UnknownKey, A), (C::ParseFailed, R)]
    );
    assert_eq!(
        parsed.issues[0].path.to_string(),
        "tasks[0].function.input.x"
    );
    assert_eq!(parsed.issues[1].path.segs().len(), 3, "the task's own key");
    assert_eq!(
        parsed.issues[1].message,
        "`function.input.x` cannot be kept in `ext` as `function.input.x`: the key at \
         `tasks[0].function.input.x` is already kept there"
    );
    assert!(parsed.def.is_none());
}

// ---------------------------------------------------------------------------
// A group's `continue_on_error`
// ---------------------------------------------------------------------------

#[test]
fn a_groups_continue_on_error_true_is_group_continue_on_error_kept_in_ext() {
    let parsed = compat(&wf(json!([
        {"id": "g", "continue_on_error": true, "tasks": [task_with(json!({}))]}
    ])));
    assert_eq!(
        rows(&parsed),
        vec![row(
            C::GroupContinueOnError,
            A,
            "tasks[0].continue_on_error"
        )]
    );
    let issue = &parsed.issues[0];
    assert_eq!(issue.step_kind, Some(StepKind::Group));
    assert_eq!(issue.step_id.as_deref(), Some("g"));
    // v3's message.
    assert_eq!(
        issue.message,
        "group 'g' carries continue_on_error, which the engine does not honour — error \
         handling is per task and per workflow, and a group only gates a span. Put it on the \
         tasks inside the group, or on the workflow"
    );
    let def = built(parsed);
    let g = group(&def.tasks[0]);
    assert_eq!(g.header.ext.get("continue_on_error"), Some(&json!(true)));
    // The member's policies are untouched: the flag is not honoured.
    assert_eq!(task(&g.tasks[0]).on_failure, FailurePolicy::Fail);
}

#[test]
fn a_groups_other_continue_on_error_is_an_unknown_key_kept_in_ext() {
    for value in [json!(false), json!("yes"), json!(null)] {
        let parsed = compat(&wf(json!([
            {"id": "g", "continue_on_error": value, "ext": {"ui": 1},
             "tasks": [task_with(json!({}))]}
        ])));
        assert_eq!(
            rows(&parsed),
            vec![row(C::UnknownKey, A, "tasks[0].continue_on_error")],
            "{value}"
        );
        let def = built(parsed);
        let ext = &group(&def.tasks[0]).header.ext;
        assert_eq!(ext.get("continue_on_error"), Some(&value));
        assert_eq!(ext.get("ui"), Some(&json!(1)));
    }
}

#[test]
fn a_groups_continue_on_error_already_in_its_ext_is_refused() {
    let parsed = compat(&wf(json!([
        {"id": "g", "continue_on_error": true, "ext": {"continue_on_error": false},
         "tasks": [task_with(json!({}))]}
    ])));
    assert_eq!(
        rows(&parsed),
        vec![row(C::ParseFailed, R, "tasks[0].continue_on_error")]
    );
}

// ---------------------------------------------------------------------------
// Typo candidates
// ---------------------------------------------------------------------------

#[test]
fn under_compat_a_near_miss_of_a_v3_key_is_a_likely_typo() {
    for (json, path, near) in [
        (
            json!({"id": "w", "continue_on_eror": true, "tasks": [task_with(json!({}))]}),
            "continue_on_eror",
            "continue_on_error",
        ),
        (
            wf(json!([task_with(json!({"continue_on_eror": true}))])),
            "tasks[0].continue_on_eror",
            "continue_on_error",
        ),
        (
            wf(json!([task_with(json!({"halt_onn": "failure"}))])),
            "tasks[0].halt_onn",
            "halt_on",
        ),
        (
            wf(json!([{"id": "g", "continue_on_eror": true, "tasks": [task_with(json!({}))]}])),
            "tasks[0].continue_on_eror",
            "continue_on_error",
        ),
    ] {
        let parsed = compat(&json);
        assert_eq!(rows(&parsed), vec![row(C::LikelyTypo, D, path)], "{json}");
        assert!(
            parsed.issues[0].message.contains(&format!("`{near}`")),
            "{}",
            parsed.issues[0].message
        );
        // Without the reader, no v3 spelling is a candidate.
        assert_eq!(
            rows(&strict(&json)),
            vec![row(C::UnknownKey, R, path)],
            "{json}"
        );
    }
}

// ---------------------------------------------------------------------------
// Order
// ---------------------------------------------------------------------------

#[test]
fn the_readers_issues_come_first_in_document_order() {
    let parsed = compat(&json!({
        "continue_on_error": true,
        "id": "",
        "loop": {"max": 1, "setup": [{"id": "s", "halt_on": "x", "function": {"name": "f"}}]},
        "tasks": [
            {"continue_on_error": true, "function": {"name": "validate", "input": {"rules": []}},
             "halt_on": "failure", "id": "t", "terminal": 1},
            {"id": "u", "function": {"name": "http_call", "input": {"output": "data.x"}}}
        ]
    }));
    // The input spells its keys in this order, so `function` comes between a
    // task's two flags, and `loop` before `tasks`.
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::V3Spelling, A, "continue_on_error"),
            row(C::InvalidHaltOn, R, "loop.setup[0].halt_on"),
            row(C::V3Spelling, A, "tasks[0].continue_on_error"),
            row(C::V3Spelling, A, "tasks[0].function.name"),
            row(C::V3Spelling, A, "tasks[0].halt_on"),
            row(C::V3Spelling, A, "tasks[1].function.input.output"),
            row(C::EmptyWorkflowId, R, "id"),
            row(C::InvalidTerminal, R, "tasks[0].terminal"),
        ]
    );
}

// ---------------------------------------------------------------------------
// With the reader off
// ---------------------------------------------------------------------------

#[test]
fn with_compat_off_every_v3_spelling_is_rejected() {
    let json = json!({
        "id": "w",
        "continue_on_error": true,
        "tasks": [
            {"id": "t", "continue_on_error": true, "halt_on": "failure",
             "function": {"name": "f"}},
            {"id": "g", "continue_on_error": true, "tasks": [task_with(json!({"id": "m"}))]}
        ]
    });
    let parsed = strict(&json);
    assert_eq!(
        rows(&parsed),
        vec![
            row(C::UnknownKey, R, "tasks[0].continue_on_error"),
            row(C::UnknownKey, R, "tasks[0].halt_on"),
            row(C::UnknownKey, R, "tasks[1].continue_on_error"),
            row(C::UnknownKey, R, "continue_on_error"),
        ]
    );
    assert!(parsed.def.is_none());
}

#[test]
fn with_compat_off_validate_and_the_aliases_stay_as_written() {
    // Phase 3 reports `validate` unregistered and the alias in its config;
    // the walk has nothing to say about either.
    for json in [
        calling("validate", json!({"rules": []})),
        calling("http_call", json!({"connector": "c", "path_logic": "/x"})),
        calling(
            "publish_json",
            json!({"source": "s", "target": "t", "pretyy": true}),
        ),
    ] {
        let parsed = strict(&json);
        assert_eq!(rows(&parsed), vec![], "{json}");
        let def = parsed.def.unwrap();
        let function = &task(&def.tasks[0]).function;
        assert_eq!(function.name, json["tasks"][0]["function"]["name"]);
        assert_eq!(function.input, json["tasks"][0]["function"]["input"]);
    }
}

// ---------------------------------------------------------------------------
// `Serialize`
// ---------------------------------------------------------------------------

#[test]
fn serialize_omits_the_defaults_and_the_empties() {
    let def = built(compat(&json!({
        "id": "w",
        "name": null,
        "priority": 0,
        "channel": "default",
        "status": "active",
        "version": 1,
        "on_failure": "fail",
        "tags": [],
        "ext": {},
        "loop": {"max": 3, "init": 0, "increment": 1, "setup": []},
        "tasks": [
            {"id": "t", "terminal": false, "on_failure": "fail", "on_rejection": "continue",
             "for_each": {"over": {"var": "data.xs"}, "as": "x", "max_concurrency": 1},
             "function": {"name": "f"}},
            {"id": "g", "terminal": false, "tasks": [
                {"id": "m", "function": {"name": "map", "input": {"mappings": []}}}
            ]}
        ]
    })));
    assert_eq!(
        serde_json::to_value(&def).unwrap(),
        json!({
            "id": "w",
            "loop": {"max": 3},
            "tasks": [
                {"id": "t", "for_each": {"over": {"var": "data.xs"}, "as": "x"},
                 "function": {"name": "f", "input": {}}},
                {"id": "g", "tasks": [
                    {"id": "m", "function": {"name": "map", "input": {"mappings": []}}}
                ]}
            ]
        })
    );
}

#[test]
fn serialize_writes_every_key_that_is_set() {
    let v4 = json!({
        "id": "w",
        "name": "W",
        "description": "d",
        "priority": 7,
        "condition": {"==": [1, 1]},
        "tasks": [
            {"id": "t", "name": "T", "description": "td", "condition": null,
             "function": {"name": "f", "input": [1, "x"]},
             "on_failure": "halt", "on_rejection": "halt", "terminal": true,
             "for_each": {"over": [1, 2], "as": "x", "max_concurrency": 4,
                          "collect": "temp_data.r", "into": "data.rs"},
             "ext": {"ui": {"x": 1}}},
            {"id": "g", "name": "G", "description": "gd", "condition": false,
             "terminal": true, "tasks": [{"id": "m", "function": {"name": "f", "input": {}}}],
             "ext": {"continue_on_error": true}}
        ],
        "on_failure": "continue",
        "channel": "orders",
        "status": "paused",
        "version": 3,
        "rollout": {"bucket_start": 10, "bucket_end": 60},
        "loop": {"counter": "i", "init": 2, "increment": 3, "max": 9,
                 "setup": [{"id": "s", "function": {"name": "f", "input": {}}}],
                 "over": {"var": "data.xs"}, "as": "it", "scratch": "sc"},
        "tags": ["a", "b"],
        "created_at": "2026-01-02T03:04:05.123456789+00:00",
        "updated_at": "2026-02-03T04:05:06+00:00",
        "ext": {"path": "ui/folder"}
    });
    let def = built(strict(&v4));
    assert_eq!(serde_json::to_value(&def).unwrap(), v4);
    // The text keeps the table's key order, `id` first and `ext` last.
    let text = serde_json::to_string(&def).unwrap();
    assert!(
        text.starts_with(r#"{"id":"w","name":"W","description":"d","priority":7"#),
        "{text}"
    );
    assert!(text.ends_with(r#""ext":{"path":"ui/folder"}}"#), "{text}");
}

#[test]
fn a_timestamp_is_written_in_rfc_3339() {
    // v3 reads chrono's relaxed forms; v4 writes the strict one.
    let def = built(compat(&json!({
        "id": "w", "created_at": "2026-01-02 03:04:05 UTC", "tasks": [task_with(json!({}))]
    })));
    assert_eq!(
        serde_json::to_value(&def).unwrap()["created_at"],
        json!("2026-01-02T03:04:05+00:00")
    );
}

// ---------------------------------------------------------------------------
// `migrate_v3`
// ---------------------------------------------------------------------------

#[test]
fn migrate_v3_writes_the_v4_spelling() {
    let m = migrated(&json!({
        "id": "w",
        "name": "w",
        "continue_on_error": true,
        "path": "ui/folder",
        "tasks": [
            {"continue_on_error": true, "function": {"name": "validate", "input": {"rules": []}},
             "halt_on": "failure", "id": "t", "name": "t"},
            {"id": "g", "continue_on_error": true, "tasks": [
                {"id": "p", "function": {"name": "publish_json",
                                         "input": {"source": "s", "target": "t", "x": 1}}}
            ]}
        ]
    }));
    assert_eq!(
        m.json,
        json!({
            "id": "w",
            "name": "w",
            "on_failure": "continue",
            "tasks": [
                {"id": "t", "name": "t", "on_failure": "halt", "on_rejection": "halt",
                 "function": {"name": "validation", "input": {"rules": []}}},
                {"id": "g", "ext": {"continue_on_error": true}, "tasks": [
                    {"id": "p", "function": {"name": "publish_json",
                                             "input": {"source": "s", "target": "t"}},
                     "ext": {"function.input.x": 1}}
                ]}
            ],
            "ext": {"path": "ui/folder"}
        })
    );
    assert_eq!(
        rewrites(&m),
        vec![
            rewrite(
                "continue_on_error",
                "continue_on_error: true",
                "on_failure: continue"
            ),
            rewrite(
                "tasks[0].continue_on_error",
                "continue_on_error: true",
                "on_failure: halt, on_rejection: halt"
            ),
            rewrite("tasks[0].function.name", "validate", "validation"),
            rewrite(
                "tasks[0].halt_on",
                "halt_on: failure",
                "on_failure: halt, on_rejection: halt"
            ),
        ]
    );
    // The issues that remain, none Rejected: the rewrites and the moves.
    let codes: Vec<(C, String)> = m
        .issues
        .iter()
        .map(|i| (i.code, i.path.to_string()))
        .collect();
    assert_eq!(
        codes,
        vec![
            (C::V3Spelling, "continue_on_error".into()),
            (C::V3Spelling, "tasks[0].continue_on_error".into()),
            (C::V3Spelling, "tasks[0].function.name".into()),
            (C::V3Spelling, "tasks[0].halt_on".into()),
            (C::GroupContinueOnError, "tasks[1].continue_on_error".into()),
            (C::UnknownKey, "tasks[1].tasks[0].function.input.x".into()),
            (C::UnknownKey, "path".into()),
        ]
    );

    let reparsed = strict(&m.json);
    assert_eq!(rows(&reparsed), vec![]);
}

#[test]
fn migrate_v3_refuses_with_every_issue() {
    let issues = migrate_v3(&json!({
        "id": "",
        "continue_on_error": true,
        "tasks": [task_with(json!({"halt_on": "sometimes"}))]
    }))
    .unwrap_err();
    let codes: Vec<(C, Severity, String)> = issues
        .iter()
        .map(|i| (i.code, i.severity, i.path.to_string()))
        .collect();
    assert_eq!(
        codes,
        vec![
            row(C::V3Spelling, A, "continue_on_error"),
            row(C::InvalidHaltOn, R, "tasks[0].halt_on"),
            row(C::EmptyWorkflowId, R, "id"),
        ]
    );
}

#[test]
fn migrate_v3_of_a_v4_definition_is_the_definition() {
    let v4 = json!({"id": "w", "tasks": [{"id": "t", "function": {"name": "f", "input": {}}}]});
    let m = migrated(&v4);
    assert_eq!(m.json, v4);
    assert!(m.rewrites.is_empty());
    assert!(m.issues.is_empty());
}

/// Every definition in the corpus: each case's `workflows`, and those its
/// reload steps swap in.
fn corpus_definitions() -> Vec<(String, Value)> {
    use dataflow_corpus::case::Step;
    let mut out = Vec::new();
    for (path, case) in dataflow_corpus::load_cases() {
        let name = dataflow_corpus::case_name(&path);
        let reloads = case.steps.iter().filter_map(|step| match step {
            Step::Reload(reload) => Some(&reload.workflows),
            Step::Run(_) => None,
        });
        for (i, workflow) in std::iter::once(&case.workflows)
            .chain(reloads)
            .flatten()
            .enumerate()
        {
            let json = serde_json::to_value(workflow).expect("a corpus workflow is JSON");
            out.push((format!("{name}#{i}"), json));
        }
    }
    out
}

/// Whether any node of `def` carries `ext` data.
fn has_ext(def: &WorkflowDef) -> bool {
    !def.header.ext.is_empty()
        || def.walk().any(|(_, _, step)| match step {
            StepDef::Task(task) => !task.header.ext.is_empty(),
            StepDef::Group(group) => !group.header.ext.is_empty(),
        })
}

/// Whether `json` has an `"ext"` key at any depth.
fn writes_ext(json: &Value) -> bool {
    match json {
        Value::Object(obj) => obj.contains_key("ext") || obj.values().any(writes_ext),
        Value::Array(items) => items.iter().any(writes_ext),
        _ => false,
    }
}

#[test]
fn every_corpus_definition_migrates_to_one_that_parses_with_compat_off_to_an_equal_def() {
    let (mut migrated_count, mut refused, mut rewritten, mut with_ext) = (0, 0, 0, 0);
    let mut saw_publish = false;
    for (name, json) in corpus_definitions() {
        let Ok(m) = migrate_v3(&json) else {
            refused += 1;
            continue;
        };
        migrated_count += 1;
        rewritten += usize::from(!m.rewrites.is_empty());
        let def = compat(&json).def.expect("migrate_v3 succeeded");

        let reparsed = strict(&m.json);
        assert_eq!(rows(&reparsed), vec![], "{name}: {}", m.json);
        assert_eq!(reparsed.def.as_ref(), Some(&def), "{name}");

        if has_ext(&def) {
            with_ext += 1;
            assert!(writes_ext(&m.json), "{name}: {}", m.json);
        }
        // Migrating a migrated definition changes nothing.
        let again = migrate_v3(&m.json).expect("a v4 definition migrates");
        assert_eq!(again.json, m.json, "{name}");
        assert!(again.rewrites.is_empty(), "{name}");

        // I56: the publish case's `data.obj` source is not rewritten; the
        // config keeps v3's fallback (`PublishSource`).
        if name == "builtins/publish#0" {
            assert!(
                m.rewrites
                    .iter()
                    .all(|r| !r.path.to_string().ends_with(".source")),
                "{:?}",
                m.rewrites
            );
            assert_eq!(
                m.json["tasks"][1]["function"]["input"]["source"],
                "data.obj"
            );
            saw_publish = true;
        }
    }
    assert!(saw_publish, "builtins/publish#0 is in the corpus");
    // 143 definitions on 2026-10-12: 130 migrate, 30 of them carrying a v3
    // spelling and 7 host data; 13 are refused, each by a rule v3 refuses
    // too (an alias beside its target, a group's shape, a loop's bounds).
    assert!(migrated_count >= 130, "only {migrated_count} migrated");
    assert!(rewritten >= 30, "only {rewritten} carry a v3 spelling");
    assert!(with_ext >= 7, "only {with_ext} carry host data");
    assert!(refused >= 13, "only {refused} refused");
}
