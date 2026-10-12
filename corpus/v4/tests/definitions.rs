//! Phase 2's check: v4 parses every corpus definition, or refuses it with v3's
//! code at v3's path. A definition v3 accepted must have no Rejected issue and
//! no Defect in v4. Issues are compared as multisets, so one reported twice
//! must be expected twice. Every skip is printed with its reason: a whole
//! definition under a deviation or pending a later phase, and each `checked`
//! issue of a phase 3 code.

use dataflow_corpus::{Case, Json, case_name, load_cases, recording_path};
use dataflow_corpus_v4::{Row, actual, definitions, expected};
use std::fs;

/// Definitions whose refusal comes from a config a later phase builds:
/// (case, definition index, reason).
const PENDING: &[(&str, usize, &str)] = &[(
    "builtins/parse_invalid",
    1,
    "phase 3: the parse_json config",
)];

/// The deviation covering definition `i`, if any: a path equal to
/// `/definitions`, `/definitions/<i>`, or below `/definitions/<i>`.
fn deviation_for(case: &Case, i: usize) -> Option<String> {
    let own = format!("/definitions/{i}");
    let own: Vec<&str> = own.split('/').collect();
    case.deviations
        .iter()
        .find(|d| {
            d.paths.iter().any(|pointer| {
                let segs: Vec<&str> = pointer.split('/').collect();
                segs.get(1) == Some(&"definitions")
                    && segs.iter().zip(&own).all(|(a, b)| *a == "*" || a == b)
            })
        })
        .map(|d| format!("break {}", d.break_row))
}

#[test]
fn v4_parses_every_corpus_definition_as_v3_refused_it() {
    let (mut checked, mut skipped) = (0usize, 0usize);
    let mut failures = Vec::new();
    for (path, case) in load_cases() {
        let name = case_name(&path);
        let recording = Json::from_text(
            &fs::read_to_string(recording_path(&path)).unwrap_or_else(|e| panic!("{name}: {e}")),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let records = match recording.get("definitions") {
            Some(Json::Array(items)) => items,
            _ => panic!("{name}: the recording has no definitions"),
        };
        for (i, definition) in definitions(&case).iter().enumerate() {
            if let Some(why) = deviation_for(&case, i) {
                println!("skip {name}#{i}: deviation, {why}");
                skipped += 1;
                continue;
            }
            if let Some((_, _, why)) = PENDING.iter().find(|(n, j, _)| *n == name && *j == i) {
                println!("skip {name}#{i}: pending, {why}");
                skipped += 1;
                continue;
            }
            checked += 1;
            let (def, got) = actual(definition);
            let want = expected(&records[i], def.as_ref());
            for row in &want.phase_3 {
                let at = row.path.as_deref().unwrap_or_default();
                println!("{name}#{i}: skipped (phase 3): {} at {at}", row.code);
            }
            for row in &want.uncovered {
                failures.push(format!(
                    "{name}#{i}: v3 checked {row:?}, a code this driver neither compares nor \
                     defers to phase 3"
                ));
            }
            if let Some(problem) = compare(&want.rows, &got) {
                failures.push(format!("{name}#{i}: {problem}"));
            }
        }
    }
    println!("definitions checked: {checked}, skipped: {skipped}");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    assert!(checked >= 100, "only {checked} definitions were checked");
}

/// Multiset equality of rows: an issue reported twice must be expected
/// twice. A v3 row with no path (`PARSE_FAILED`) matches any one Rejected v4
/// row the explicit rows leave over, and must match one.
fn compare(want: &[Row], got: &[Row]) -> Option<String> {
    let mut left: Vec<&Row> = got.iter().collect();
    let mut missing: Vec<&Row> = Vec::new();
    for row in want.iter().filter(|r| r.path.is_some()) {
        match left.iter().position(|g| *g == row) {
            Some(at) => {
                left.remove(at);
            }
            None => missing.push(row),
        }
    }
    for row in want.iter().filter(|r| r.path.is_none()) {
        match left.iter().position(|g| g.severity == "REJECTED") {
            Some(at) => {
                left.remove(at);
            }
            None => {
                return Some(format!(
                    "v3 reported {row:?} and v4 refused nothing else\n  v4: {got:?}"
                ));
            }
        }
    }
    if missing.is_empty() && left.is_empty() {
        return None;
    }
    Some(format!("v3 only: {missing:?}\n  v4 only: {left:?}"))
}

fn row(code: &str, path: Option<&str>) -> Row {
    Row {
        code: code.to_owned(),
        severity: "REJECTED".to_owned(),
        path: path.map(str::to_owned),
    }
}

/// Whether `compare` finds the two lists of rows equal.
fn equal(want: &[&Row], got: &[&Row]) -> bool {
    let want: Vec<Row> = want.iter().map(|r| (*r).clone()).collect();
    let got: Vec<Row> = got.iter().map(|r| (*r).clone()).collect();
    compare(&want, &got).is_none()
}

#[test]
fn compare_counts_each_issue_as_often_as_it_is_reported() {
    let a = row("INVALID_PATH", Some("tasks[0].x"));
    assert!(equal(&[&a], &[&a]));
    assert!(!equal(&[&a], &[&a, &a]), "v4 reporting it twice");
    assert!(!equal(&[&a, &a], &[&a]), "v3 reporting it twice");
    assert!(equal(&[&a, &a], &[&a, &a]));

    // A pathless PARSE_FAILED takes one Rejected issue the others leave over.
    let pathless = row("PARSE_FAILED", None);
    let max = row("PARSE_FAILED", Some("loop.max"));
    assert!(equal(&[&pathless], &[&max]));
    assert!(!equal(&[&pathless], &[]));
    assert!(!equal(&[&pathless], &[&max, &max]));
    assert!(equal(&[&pathless, &pathless], &[&max, &max]));
    assert!(!equal(&[&a], &[&a, &max]));
}
