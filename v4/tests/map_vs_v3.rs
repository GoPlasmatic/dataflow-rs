extern crate dataflow_rs_v4 as dataflow_rs;

// `map`'s config against v3: every combination of a mapping's keys, and the
// `INVALID_MAPPING` and `NULL_MAPPING` issues each side reports: code, path and
// message, in order.

use dataflow_rs::Issue;
use dataflow_rs::expr::{CompileCx, ExprEngine};
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::function::builtins::MapConfig;
use dataflow_rs::program::IssueCode;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

/// Where a workflow's first task keeps its function's input.
const ROOT: &str = "tasks[0].function.input";

/// Every combination of `logic` (absent, present, `null`), `unset` (absent,
/// `true`, `false`), `on_null` (absent, `skip`, `unset`), `mode` (absent, `set`,
/// `append`, `extend`) and `path` (`data`, `data.x`).
fn mappings() -> Vec<Value> {
    let logics = [None, Some(json!({"var": "data.y"})), Some(Value::Null)];
    let unsets = [None, Some(true), Some(false)];
    let on_nulls = [None, Some("skip"), Some("unset")];
    let modes = [None, Some("set"), Some("append"), Some("extend")];
    let paths = ["data", "data.x"];
    let mut all = Vec::new();
    for logic in &logics {
        for unset in unsets {
            for on_null in on_nulls {
                for mode in modes {
                    for path in paths {
                        let mut m = Map::new();
                        m.insert("path".into(), json!(path));
                        if let Some(logic) = logic {
                            m.insert("logic".into(), logic.clone());
                        }
                        if let Some(unset) = unset {
                            m.insert("unset".into(), json!(unset));
                        }
                        if let Some(on_null) = on_null {
                            m.insert("on_null".into(), json!(on_null));
                        }
                        if let Some(mode) = mode {
                            m.insert("mode".into(), json!(mode));
                        }
                        all.push(Value::Object(m));
                    }
                }
            }
        }
    }
    assert_eq!(all.len(), 216);
    all
}

fn workflow(mappings: &[Value]) -> Value {
    json!({"id": "w", "name": "w", "tasks": [{"id": "t", "name": "t", "function": {
        "name": "map", "input": {"mappings": mappings}
    }}]})
}

fn v4_compile(mappings: &[Value]) -> (Option<MapConfig>, Vec<Issue>) {
    let engine = ExprEngine::default();
    let mut issues = Vec::new();
    let config = {
        let mut cx = CompileCx::new(&engine, &mut issues).at_path(ROOT.parse().unwrap());
        MapConfig::compile(&json!({"mappings": mappings}), &mut cx)
    };
    (config, issues)
}

/// One issue: its code, path and message.
type Row = (String, String, String);

/// Every issue v4 reports for `mappings`, of any code.
fn v4_rows(mappings: &[Value]) -> Vec<Row> {
    v4_compile(mappings)
        .1
        .into_iter()
        .map(|i| (i.code.as_str().to_owned(), i.path.to_string(), i.message))
        .collect()
}

fn v4_issues(mappings: &[Value], code: IssueCode) -> Vec<Row> {
    v4_rows(mappings)
        .into_iter()
        .filter(|(c, _, _)| c == code.as_str())
        .collect()
}

/// Every issue v3's `validate_authored` reports, of any code.
fn v3_authored(mappings: &[Value]) -> Vec<Row> {
    dataflow_rs_v3::Workflow::validate_authored(&workflow(mappings))
        .into_iter()
        .map(|i| (i.code.as_str().to_owned(), i.path.unwrap(), i.message))
        .collect()
}

/// v3's `NULL_MAPPING` message with a literal path named as `describe_path`
/// documents it, in its dotted form: `'data.x'`.
///
/// v3's `check_workflow` runs on an uncompiled `Workflow`, where
/// `PathTemplate::constant_path` is not yet populated, so `describe_path`
/// falls back to the path's JSON and writes `'"data.x"'`. v4 names a folded
/// path dotted. Every path in this test is a literal string.
fn dotted(message: &str) -> String {
    let (head, rest) = message.split_once("'\"").expect("a quoted JSON path");
    let (path, tail) = rest.split_once("\"'").expect("a quoted JSON path");
    format!("{head}'{path}'{tail}")
}

/// Every issue v3's `check_workflow` reports for `mappings`, which v3 must
/// accept, of any code, with the path prefixed `tasks[0].`.
fn v3_checked(mappings: &[Value]) -> Vec<Row> {
    let workflow = dataflow_rs_v3::Workflow::from_json(&workflow(mappings).to_string())
        .expect("v3 accepts every mapping it reported nothing for");
    dataflow_rs_v3::Engine::builder()
        .check_workflow(&workflow)
        .into_iter()
        .map(|i| {
            (
                i.code.as_str().to_owned(),
                format!("tasks[0].{}", i.path.unwrap()),
                dotted(&i.message),
            )
        })
        .collect()
}

/// The index of the mapping an issue path sits in.
fn mapping_index(path: &str) -> usize {
    let rest = path
        .strip_prefix(&format!("{ROOT}.mappings["))
        .unwrap_or_else(|| panic!("not a mapping path: {path}"));
    rest[..rest.find(']').unwrap()].parse().unwrap()
}

/// `path` with its mapping index replaced by `index`.
fn reindexed(path: &str, index: usize) -> String {
    let below = path
        .strip_prefix(&format!("{ROOT}.mappings["))
        .unwrap_or_else(|| panic!("not a mapping path: {path}"));
    let rest = &below[below.find(']').unwrap()..];
    format!("{ROOT}.mappings[{index}{rest}")
}

/// The 216 split by v3's verdict: the indices it refused, and the indices and
/// mappings it accepted.
fn split(all: &[Value]) -> (BTreeSet<usize>, Vec<usize>, Vec<Value>) {
    let refused: BTreeSet<usize> = v3_authored(all)
        .iter()
        .map(|(_, path, _)| mapping_index(path))
        .collect();
    let (indices, accepted) = all
        .iter()
        .enumerate()
        .filter(|(i, _)| !refused.contains(i))
        .map(|(i, m)| (i, m.clone()))
        .unzip();
    (refused, indices, accepted)
}

#[test]
fn invalid_mapping_matches_v3_issue_for_issue() {
    let all = mappings();
    let v3 = v3_authored(&all);
    assert!(!v3.is_empty(), "the comparison must not be vacuous");
    // Nothing but INVALID_MAPPING, so filtering v4's list hides nothing on v3's.
    assert!(v3.iter().all(|(code, _, _)| code == "INVALID_MAPPING"));
    // Exactly one issue per refused mapping: the first rule it breaks.
    let (refused, _, _) = split(&all);
    assert_eq!(refused.len(), v3.len());
    assert_eq!(v4_issues(&all, IssueCode::InvalidMapping), v3);
}

#[test]
fn null_mapping_matches_v3_issue_for_issue() {
    let all = mappings();
    // v3 refuses a definition with an INVALID_MAPPING before `check_workflow`
    // can run, so both sides check the mappings v3 accepts.
    let (refused, _, accepted) = split(&all);
    assert!(!accepted.is_empty() && !refused.is_empty());

    let v3 = v3_checked(&accepted);
    assert!(!v3.is_empty(), "the comparison must not be vacuous");
    assert!(v3.iter().all(|(code, _, _)| code == "NULL_MAPPING"));
    assert_eq!(v4_rows(&accepted), v3);

    // v4 accepts every mapping v3 accepts: nothing it reports is Rejected.
    let (config, issues) = v4_compile(&accepted);
    assert!(config.is_some(), "{issues:?}");
    assert_eq!(config.unwrap().mappings.len(), accepted.len());
}

/// v4's whole issue list over the 216, of every code, is v3's: the
/// `INVALID_MAPPING` row of each mapping v3 refuses, and the `NULL_MAPPING`
/// row of each it accepts but would never write, in mapping order. A stray
/// issue of any other code, or a second issue on one mapping, fails it.
#[test]
fn every_issue_v4_reports_over_the_216_is_one_v3_reports() {
    let all = mappings();
    let (_, indices, accepted) = split(&all);
    let mut expected: Vec<(usize, Row)> = v3_authored(&all)
        .into_iter()
        .map(|row| (mapping_index(&row.1), row))
        .collect();
    for (code, path, message) in v3_checked(&accepted) {
        let index = indices[mapping_index(&path)];
        expected.push((index, (code, reindexed(&path, index), message)));
    }
    expected.sort_by_key(|(index, _)| *index);
    let expected: Vec<Row> = expected.into_iter().map(|(_, row)| row).collect();

    let (config, _) = v4_compile(&all);
    assert!(config.is_none());
    assert_eq!(v4_rows(&all), expected);
}
