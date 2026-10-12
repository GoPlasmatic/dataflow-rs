//! The corpus covers its contract: every row of 02, every break of 06 and
//! every code of 04 has a case, every case has a recording, and every
//! reference a case makes resolves.

use dataflow_corpus::case::FEATURES;
use dataflow_corpus::contract::load_contract;
use dataflow_corpus::{Case, Json, case_name, load_cases, recording_path};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn cases() -> Vec<(PathBuf, Case)> {
    load_cases()
}

fn recording(path: &std::path::Path) -> Option<Json> {
    let text = std::fs::read_to_string(recording_path(path)).ok()?;
    Some(Json::from_text(&text).expect("a recording parses"))
}

#[test]
fn every_v3_row_has_a_case() {
    let covered: BTreeSet<String> = cases().iter().flat_map(|(_, c)| c.covers.clone()).collect();
    let missing: Vec<String> = load_contract()
        .into_iter()
        .filter(|row| row.v3 && !covered.contains(&row.id))
        .map(|row| row.id)
        .collect();
    assert!(missing.is_empty(), "rows with no case: {missing:?}");
}

#[test]
fn every_cover_names_a_contract_row() {
    let ids: BTreeSet<String> = load_contract().into_iter().map(|row| row.id).collect();
    for (path, case) in cases() {
        for id in &case.covers {
            assert!(
                ids.contains(id),
                "{}: covers unknown row {id}",
                case_name(&path)
            );
        }
        assert!(
            !case.covers.is_empty(),
            "{}: covers nothing",
            case_name(&path)
        );
    }
}

#[test]
fn every_case_has_a_recording() {
    let missing: Vec<String> = cases()
        .iter()
        .filter(|(path, _)| recording(path).is_none())
        .map(|(path, _)| case_name(path))
        .collect();
    assert!(
        missing.is_empty(),
        "no recording; run corpus/record.sh: {missing:?}"
    );
}

#[test]
fn every_case_runs_in_one_ci_build() {
    for (path, case) in cases() {
        let name = case_name(&path);
        let r = &case.requires;
        assert!(
            r.on.is_empty() || r.off.is_empty(),
            "{name}: needs features both on and off, so neither CI build runs it"
        );
        for feature in r.on.iter().chain(&r.off) {
            assert!(
                FEATURES.contains(&feature.as_str()),
                "{name}: unknown feature {feature}"
            );
        }
    }
}

#[test]
fn every_code_row_shows_its_code() {
    let cases = cases();
    for row in load_contract() {
        let Some(code) = row.expect_code.as_deref().filter(|_| row.v3) else {
            continue;
        };
        let shown = cases.iter().any(|(path, case)| {
            case.covers.contains(&row.id) && recording(path).is_some_and(|r| has_code(&r, code))
        });
        assert!(
            shown,
            "{}: no covering case records the code {code}",
            row.id
        );
    }
}

#[test]
fn every_deviation_names_a_break() {
    let ids: BTreeSet<String> = load_contract().into_iter().map(|row| row.id).collect();
    for (path, case) in cases() {
        let name = case_name(&path);
        let recorded = recording(&path);
        for deviation in &case.deviations {
            let row = format!("break.{}", deviation.break_row);
            assert!(
                ids.contains(&row),
                "{name}: deviation names {row}, which is not a break"
            );
            assert!(
                !deviation.paths.is_empty(),
                "{name}: a deviation with no paths"
            );
            if let Some(recorded) = &recorded {
                for pointer in &deviation.paths {
                    assert!(
                        resolves(recorded, pointer),
                        "{name}: deviation path {pointer} is not in the recording"
                    );
                }
            }
        }
    }
}

/// Does any object in `json` carry `"code": code`?
fn has_code(json: &Json, code: &str) -> bool {
    match json {
        Json::Object(pairs) => pairs
            .iter()
            .any(|(k, v)| (k == "code" && v.as_str() == Some(code)) || has_code(v, code)),
        Json::Array(items) => items.iter().any(|item| has_code(item, code)),
        _ => false,
    }
}

/// Does `pointer`, with `*` matching any one segment, name something in `json`?
fn resolves(json: &Json, pointer: &str) -> bool {
    let Some(rest) = pointer.strip_prefix('/') else {
        return pointer.is_empty();
    };
    let (head, tail) = match rest.split_once('/') {
        Some((head, tail)) => (head, format!("/{tail}")),
        None => (rest, String::new()),
    };
    match json {
        Json::Object(pairs) => pairs
            .iter()
            .any(|(k, v)| (head == "*" || head == k) && resolves(v, &tail)),
        Json::Array(items) => items
            .iter()
            .enumerate()
            .any(|(i, v)| (head == "*" || head == i.to_string()) && resolves(v, &tail)),
        _ => false,
    }
}
