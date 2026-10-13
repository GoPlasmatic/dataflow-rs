//! Phase 3's check: v4 compiles every corpus definition as v3 built it.
//!
//! What v4 reports for a definition is `parse`'s issues, then, when it
//! parsed, the case's compiler's. A definition a deviation covers is held,
//! unfiltered, to its row of `expect::DEVIATED`. Any other is held to v3's
//! `authored` and `checked` issues, comparing every v4 issue of one of v3's
//! codes and every Rejected or Defect one of v4's own. Issues are compared as
//! multisets, so one reported twice must be expected twice. Nothing is
//! skipped.

use dataflow_corpus::{Case, Json, case_name, load_cases, recording_path};
use dataflow_corpus_v4::expect::{DEVIATED, compared};
use dataflow_corpus_v4::{Row, actual, compiler, definitions, expected};
use std::fs;

/// The corpus's definitions, every one of which is checked.
const DEFINITIONS: usize = 134;

/// Whether the deviation path `pointer` (JSON pointer, `*` for any segment)
/// and `target` agree on every segment both have: one is the other, or lies
/// below it.
fn overlaps(pointer: &str, target: &str) -> bool {
    pointer
        .split('/')
        .zip(target.split('/'))
        .all(|(a, b)| a == "*" || a == b)
}

/// The breaks of the deviations covering `target`.
fn breaks_at<'c>(case: &'c Case, target: &str) -> Vec<&'c str> {
    case.deviations
        .iter()
        .filter(|d| d.paths.iter().any(|pointer| overlaps(pointer, target)))
        .map(|d| d.break_row.as_str())
        .collect()
}

/// The breaks of the deviations covering definition `i`: a path equal to
/// `/definitions`, `/definitions/<i>`, or below `/definitions/<i>`.
fn deviation_for(case: &Case, i: usize) -> Vec<&str> {
    breaks_at(case, &format!("/definitions/{i}"))
}

#[test]
fn v4_compiles_every_corpus_definition_as_v3_built_it() {
    let cases = load_cases();
    let corpus: usize = cases.iter().map(|(_, case)| case.workflows.len()).sum();
    let mut checked = 0usize;
    let mut used = vec![0usize; DEVIATED.len()];
    let mut failures = Vec::new();
    for (path, case) in &cases {
        let name = case_name(path);
        let recording = Json::from_text(
            &fs::read_to_string(recording_path(path)).unwrap_or_else(|e| panic!("{name}: {e}")),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let records = match recording.get("definitions") {
            Some(Json::Array(items)) => items,
            _ => panic!("{name}: the recording has no definitions"),
        };
        let compiler = compiler(case);
        // Whether some definition no deviation covers expects a Rejected row.
        let mut rejects = false;
        for (i, definition) in definitions(case).iter().enumerate() {
            let (def, got) = actual(&compiler, definition);
            let breaks = deviation_for(case, i);
            let problem = if breaks.is_empty() {
                let want = expected(&records[i], def.as_ref());
                rejects |= want.iter().any(|row| row.severity == "REJECTED");
                let got: Vec<Row> = got.into_iter().filter(compared).collect();
                compare(&want, &got)
            } else {
                match DEVIATED
                    .iter()
                    .position(|(n, j, _, _)| *n == name && *j == i)
                {
                    None => Some(format!(
                        "deviated (break {}), and expect::DEVIATED has no row for it\n  v4: {got:?}",
                        breaks.join(", ")
                    )),
                    Some(at) => {
                        used[at] += 1;
                        let (_, _, allowed, rows) = DEVIATED[at];
                        let allowed: Vec<&str> = allowed.split(", ").collect();
                        match breaks.iter().find(|b| !allowed.contains(b)) {
                            Some(b) => Some(format!(
                                "deviated by break {b}, which its expect::DEVIATED row does \
                                 not name"
                            )),
                            None => compare(&rows.iter().map(Row::from).collect::<Vec<_>>(), &got),
                        }
                    }
                }
            };
            if let Some(problem) = problem {
                failures.push(format!("{name}#{i}: {problem}"));
            }
            checked += 1;
        }
        let built = matches!(recording.get("build"), Some(Json::String(ok)) if ok == "ok");
        if !built && breaks_at(case, "/build").is_empty() && !rejects {
            failures.push(format!(
                "{name}: v3's build failed, and no definition expects the Rejected issue \
                 explaining it"
            ));
        }
    }
    for ((case, i, _, _), n) in DEVIATED.iter().zip(&used) {
        if *n != 1 {
            failures.push(format!(
                "expect::DEVIATED's row for {case}#{i} matched {n} deviated definitions, not 1"
            ));
        }
    }
    let skipped = corpus - checked;
    println!("definitions checked: {checked}, skipped: {skipped}");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    assert_eq!(skipped, 0);
    assert_eq!(
        checked, DEFINITIONS,
        "the corpus's definition count changed"
    );
}

/// Multiset equality of rows: an issue reported twice must be expected
/// twice. A v3 row with no path (`PARSE_FAILED`) matches any one Rejected v4
/// row the explicit rows leave over whose code and path no explicit row
/// names, and must match one.
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
    let named = |g: &Row| {
        want.iter()
            .any(|w| w.path.is_some() && w.code == g.code && w.path == g.path)
    };
    for row in want.iter().filter(|r| r.path.is_none()) {
        match left
            .iter()
            .position(|g| g.severity == "REJECTED" && !named(g))
        {
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

#[test]
fn a_pathless_row_cannot_absorb_a_duplicate_of_an_explicit_one() {
    let a = row("INVALID_PATH", Some("tasks[0].x"));
    let pathless = row("PARSE_FAILED", None);
    assert!(!equal(&[&a, &pathless], &[&a, &a]));
    // The same code at another path is still free to take.
    let b = row("INVALID_PATH", Some("tasks[0].y"));
    assert!(equal(&[&a, &pathless], &[&a, &b]));
}

#[test]
fn a_deviation_covers_its_definition_and_everything_below_it() {
    for (pointer, target, covers) in [
        ("/definitions", "/definitions/3", true),
        ("/definitions/3", "/definitions/3", true),
        ("/definitions/3/checked", "/definitions/3", true),
        ("/definitions/*/authored", "/definitions/3", true),
        ("/definitions/2", "/definitions/3", false),
        ("/build", "/build", true),
        ("/steps", "/build", false),
        ("/definitions/0", "/build", false),
    ] {
        assert_eq!(overlaps(pointer, target), covers, "{pointer} over {target}");
    }
}
