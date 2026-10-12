//! Phase 2's check: v4 parses every corpus definition, or refuses it with v3's
//! code at v3's path. A definition v3 accepted must have no Rejected issue and
//! no Defect in v4.

use dataflow_corpus::{Case, Json, case_name, load_cases, recording_path};
use dataflow_corpus_v4::{Row, actual, definitions, expected};
use std::collections::BTreeSet;
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
            if let Some(problem) = compare(&want, &got) {
                failures.push(format!("{name}#{i}: {problem}"));
            }
        }
    }
    println!("definitions checked: {checked}, skipped: {skipped}");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    assert!(checked >= 100, "only {checked} definitions were checked");
}

/// Set equality of rows; a v3 row with no path (`PARSE_FAILED`) matches any
/// Rejected v4 row the explicit rows leave over, and must match one.
fn compare(want: &[Row], got: &[Row]) -> Option<String> {
    let mut want_set: BTreeSet<&Row> = want.iter().filter(|r| r.path.is_some()).collect();
    let mut got_set: BTreeSet<&Row> = got.iter().collect();
    for row in want.iter().filter(|r| r.path.is_none()) {
        let hit = got_set
            .iter()
            .find(|g| g.severity == "REJECTED" && !want_set.contains(**g))
            .copied();
        match hit {
            Some(g) => {
                got_set.remove(g);
            }
            None => {
                return Some(format!(
                    "v3 reported {row:?} and v4 refused nothing else\n  v4: {got:?}"
                ));
            }
        }
    }
    if want_set == got_set {
        return None;
    }
    want_set.retain(|r| !got_set.contains(*r));
    got_set.retain(|r| !want.contains(*r));
    Some(format!("v3 only: {want_set:?}\n  v4 only: {got_set:?}"))
}
