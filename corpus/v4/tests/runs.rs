//! v4 runs every case as v3 recorded it (I69, I94): the built-ins, host
//! calls through `script`, fan-outs, reloads and the ops budget.
//!
//! A case in which v4 refuses a definition v3's `parse` accepted has no
//! engine: v3's build must have failed too, or a deviation must cover
//! `/build`, whose `expect::RUN_DEVIATED` row v4's build error must match,
//! and no step is compared. One both refused is left out, as v3's driver
//! left it out. A case whose steps wait for phase 6 (`run::waits_for`), or
//! whose `requires` this build does not satisfy, is counted and skipped. Every
//! other case's `build` and `steps` are compared with v3's recording, ignoring
//! v3's `IGNORED`, the run outcome's message (I62), the trace (phase 6) and
//! every path a deviation covers. At each of those, v4's JSON must be
//! `expect::RUN_DEVIATED`'s. The counts are pinned, so nothing drops out
//! silently.

use dataflow_corpus::case::Step;
use dataflow_corpus::compare::{IGNORED, diff, ignored};
use dataflow_corpus::{Case, Json, case_name, load_cases, recording_path};
use dataflow_corpus_v4::expect::{MISSING, RUN_DEVIATED};
use dataflow_corpus_v4::run::{run_case, satisfies, waits_for};
use std::fs;

/// Cases whose steps run and are compared, in either build: `families_off`
/// and `families_on` exclude each other, and `expressions/budget` runs only
/// on a `budget` build.
const RUN: usize = if cfg!(feature = "budget") { 71 } else { 70 };
/// Cases with a definition v4 refuses and v3's `parse` accepted: a v3 build
/// failure, or breaks 2a, 3 and 15.
const NO_BUILD: usize = 10;
/// Cases whose engine has an observer or an error context.
const PHASE_6: usize = 1;
/// Cases that need features this build has the other way: one of
/// `families_on` and `families_off`, and `expressions/budget` without
/// `budget`.
const OTHER_BUILD: usize = if cfg!(feature = "budget") { 1 } else { 2 };
/// Deviated pointers held to their row of `expect::RUN_DEVIATED`, one row
/// each: none is in a case one build skips.
const RUN_DEVIATED_ROWS: usize = 24;

/// What the run comparison never compares, beyond v3's `IGNORED`: the run
/// outcome's message, which is v4's wording (I62), and the trace, which is
/// phase 6's.
const RUN_IGNORED: &[&str] = &["/steps/*/run/outcome/error/message", "/steps/*/run/trace"];

/// Whether `pointer`, or a pointer above it, is one the run comparison
/// ignores.
fn under_ignored(pointer: &str, ignore: &[&str]) -> bool {
    let mut at = String::new();
    pointer.split('/').skip(1).any(|segment| {
        at.push('/');
        at.push_str(segment);
        ignored(&at, ignore)
    })
}

/// Every pointer `pattern` (`*` for any one segment) names in `json`.
fn expand(json: &Json, pattern: &str) -> Vec<String> {
    fn walk(json: &Json, rest: &[&str], at: &mut String, out: &mut Vec<String>) {
        let Some((first, rest)) = rest.split_first() else {
            out.push(at.clone());
            return;
        };
        let children: Vec<(String, &Json)> = match json {
            Json::Object(pairs) => pairs
                .iter()
                .map(|(k, v)| (k.replace('~', "~0").replace('/', "~1"), v))
                .collect(),
            Json::Array(items) => items
                .iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v))
                .collect(),
            _ => Vec::new(),
        };
        for (key, child) in children {
            if *first == "*" || *first == key {
                let len = at.len();
                at.push('/');
                at.push_str(&key);
                walk(child, rest, at, out);
                at.truncate(len);
            }
        }
    }
    let segments: Vec<&str> = pattern.split('/').skip(1).collect();
    let mut out = Vec::new();
    walk(json, &segments, &mut String::new(), &mut out);
    out
}

/// The deviation paths of `case` the run comparison reads: those under
/// `/build` or `/steps`.
fn run_paths(case: &Case) -> Vec<&str> {
    case.deviations
        .iter()
        .flat_map(|d| d.paths.iter().map(String::as_str))
        .filter(|p| *p == "/build" || *p == "/steps" || p.starts_with("/steps/"))
        .collect()
}

/// Whether some deviation of `case` covers `/build`.
fn covers_build(case: &Case) -> bool {
    run_paths(case).contains(&"/build")
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Counts {
    run: usize,
    no_build: usize,
    phase_6: usize,
    other_build: usize,
    run_deviated: usize,
}

/// Holds v4's JSON at every pointer `patterns` name in `expected` (v3's) or
/// `actual` (v4's) to its row of `RUN_DEVIATED`, counting each row it uses.
/// A pointer at which v4's JSON is v3's is stale: the deviation declares a
/// difference that is not there, and would hide one that came back.
fn check_deviated(
    name: &str,
    patterns: &[&str],
    (expected, actual): (&Json, &Json),
    base: &[&str],
    used: &mut [usize],
    failures: &mut Vec<String>,
) {
    for pattern in patterns {
        let mut pointers = expand(expected, pattern);
        for p in expand(actual, pattern) {
            if !pointers.contains(&p) {
                pointers.push(p);
            }
        }
        if pointers.is_empty() {
            failures.push(format!(
                "{name}: the deviation path {pattern} names nothing v3 or v4 recorded"
            ));
        }
        for pointer in pointers {
            if under_ignored(&pointer, base) {
                continue;
            }
            let got = actual
                .pointer(&pointer)
                .map_or(MISSING.to_owned(), Json::to_text);
            let v3 = expected
                .pointer(&pointer)
                .map_or(MISSING.to_owned(), Json::to_text);
            if got == v3 {
                failures.push(format!(
                    "{name}: {pointer} is deviated, and v4 records there what v3 recorded: \
                     the deviation is stale\n  v3 and v4: {got}"
                ));
            }
            let row = RUN_DEVIATED
                .iter()
                .position(|(n, p, _)| *n == name && *p == pointer);
            match row {
                None => failures.push(format!(
                    "{name}: {pointer} is deviated, and expect::RUN_DEVIATED has no row \
                     for it\n  v4: {got}"
                )),
                Some(at) => {
                    used[at] += 1;
                    let want = RUN_DEVIATED[at].2;
                    let want = if want == MISSING {
                        want.to_owned()
                    } else {
                        Json::from_text(want)
                            .unwrap_or_else(|e| panic!("{name}: {pointer}'s row: {e}"))
                            .to_text()
                    };
                    if want != got {
                        failures.push(format!(
                            "{name}: at deviated {pointer}\n  expect::RUN_DEVIATED: \
                             {want}\n  v4: {got}"
                        ));
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn v4_runs_every_case_as_v3_recorded_it() {
    let base: Vec<&str> = IGNORED.iter().chain(RUN_IGNORED).copied().collect();
    let mut counts = Counts::default();
    let mut used = vec![0usize; RUN_DEVIATED.len()];
    let mut failures = Vec::new();
    for (path, case) in load_cases() {
        let name = case_name(&path);
        let recording = Json::from_text(
            &fs::read_to_string(recording_path(&path)).unwrap_or_else(|e| panic!("{name}: {e}")),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let v3_built = matches!(recording.get("build"), Some(Json::String(ok)) if ok == "ok");
        let actual = run_case(&case, &recording).await;
        let built = matches!(actual.get("build"), Some(Json::String(ok)) if ok == "ok");
        let expected = Json::Object(vec![
            (
                "build".to_owned(),
                recording.get("build").cloned().expect("a recording builds"),
            ),
            (
                "steps".to_owned(),
                recording
                    .get("steps")
                    .cloned()
                    .expect("a recording has steps"),
            ),
        ]);
        if !built {
            counts.no_build += 1;
            if covers_build(&case) {
                check_deviated(
                    &name,
                    &["/build"],
                    (&expected, &actual),
                    &base,
                    &mut used,
                    &mut failures,
                );
            } else if v3_built {
                failures.push(format!(
                    "{name}: v4 has no build where v3 built, and no deviation covers /build\n  v4: {}",
                    actual.get("build").map_or_else(String::new, Json::to_text)
                ));
            }
            continue;
        }
        if !v3_built && !covers_build(&case) {
            failures.push(format!(
                "{name}: v4 builds what v3 refused, and no deviation covers /build"
            ));
            continue;
        }
        match waits_for(&case) {
            Some("phase 6") => {
                counts.phase_6 += 1;
                continue;
            }
            Some(other) => panic!("{name}: waits for {other}"),
            None => {}
        }
        if !satisfies(&case.requires) {
            counts.other_build += 1;
            continue;
        }
        counts.run += 1;

        let deviated = run_paths(&case);
        let ignore: Vec<&str> = base.iter().chain(&deviated).copied().collect();
        let differs = diff(&expected, &actual, &ignore);
        if !differs.is_empty() {
            let shown: Vec<String> = differs.iter().take(12).map(|d| format!("  {d}")).collect();
            failures.push(format!(
                "{name}: {} differences\n{}",
                differs.len(),
                shown.join("\n")
            ));
        }
        check_deviated(
            &name,
            &deviated,
            (&expected, &actual),
            &base,
            &mut used,
            &mut failures,
        );

        for (i, step) in case.steps.iter().enumerate() {
            if let Step::Run(run) = step
                && let Some(n) = run.same_as
            {
                let at = |k: usize| actual.pointer(&format!("/steps/{k}/run/message"));
                match (at(n), at(i)) {
                    (Some(earlier), Some(now)) if diff(earlier, now, IGNORED).is_empty() => {}
                    _ => failures.push(format!(
                        "{name}: step {i}'s message is not step {n}'s (same_as)"
                    )),
                }
            }
        }
    }
    for ((case, pointer, _), n) in RUN_DEVIATED.iter().zip(&used) {
        if *n != 1 {
            failures.push(format!(
                "expect::RUN_DEVIATED's row for {case} at {pointer} matched {n} deviated \
                 pointers, not 1"
            ));
        }
    }
    counts.run_deviated = used.iter().sum();
    println!("{counts:?}");
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
    assert_eq!(
        counts,
        Counts {
            run: RUN,
            no_build: NO_BUILD,
            phase_6: PHASE_6,
            other_build: OTHER_BUILD,
            run_deviated: RUN_DEVIATED_ROWS,
        }
    );
}

#[test]
fn a_pattern_expands_to_every_pointer_it_names() {
    let json = Json::from_text(r#"{"steps":[{"run":{"a":1}},{"run":{"a":2,"b/c":3}}]}"#).unwrap();
    assert_eq!(
        expand(&json, "/steps/*/run/a"),
        ["/steps/0/run/a", "/steps/1/run/a"]
    );
    assert_eq!(
        expand(&json, "/steps/1/run/*"),
        ["/steps/1/run/a", "/steps/1/run/b~1c"]
    );
    assert!(expand(&json, "/steps/2").is_empty());
}

#[test]
fn a_pointer_below_an_ignored_one_is_ignored() {
    assert!(under_ignored("/steps/0/run/trace", RUN_IGNORED));
    assert!(under_ignored("/steps/3/run/trace/steps/0", RUN_IGNORED));
    assert!(!under_ignored("/steps/0/run/message", RUN_IGNORED));
}
