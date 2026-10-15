//! v4 runs every case as v3 recorded it (I69, I94, I109): the built-ins,
//! host calls through `script`, fan-outs, reloads, the ops budget, the
//! observer's events and the error context.
//!
//! A case in which v4 refuses a definition v3's `parse` accepted has no
//! engine: v3's build must have failed too, or a deviation must cover
//! `/build`, whose `expect::RUN_DEVIATED` row v4's build error must match,
//! and `/steps`, when v3 recorded any, since no step is compared. One both
//! refused is left out, as v3's driver left it out. A case whose `requires`
//! this build does not satisfy is counted and skipped. Every other case's
//! `build` and `steps` are compared with v3's recording, ignoring v3's
//! `IGNORED`, the run outcome's message (I62), a refused reload's code, the
//! trace (`tests/traces.rs` compares it) and every path a deviation covers.
//! At each of those, v4's JSON must be `expect::RUN_DEVIATED`'s. The counts
//! are pinned, so nothing drops out silently.

use dataflow_corpus::case::Step;
use dataflow_corpus::compare::{IGNORED, diff};
use dataflow_corpus::{Case, Json, case_name, load_cases, recording_path};
use dataflow_corpus_v4::expect::{Held, RUN_DEVIATED, under_ignored};
use dataflow_corpus_v4::run::{run_case, satisfies};
use std::fs;

/// Cases whose steps run and are compared, in either build: `families_off`
/// and `families_on` exclude each other, and `expressions/budget` runs only
/// on a `budget` build.
const RUN: usize = if cfg!(feature = "budget") { 72 } else { 71 };
/// Cases with a definition v4 refuses and v3's `parse` accepted: a v3 build
/// failure, or breaks 2a, 3 and 15.
const NO_BUILD: usize = 10;
/// Cases that need features this build has the other way: one of
/// `families_on` and `families_off`, and `expressions/budget` without
/// `budget`.
const OTHER_BUILD: usize = if cfg!(feature = "budget") { 1 } else { 2 };
/// Deviated pointers held to their row of `expect::RUN_DEVIATED`, one row
/// each: none is in a case one build skips.
const RUN_DEVIATED_ROWS: usize = 24;

/// What the run comparison never compares, beyond v3's `IGNORED`: the run
/// outcome's message, which is v4's wording (I62); a refused reload's code,
/// v3's own error's where v4's is `REJECTED`, whose message `IGNORED` already
/// skips (a reload refused against one accepted is still compared); and the
/// trace, which `tests/traces.rs` compares.
const RUN_IGNORED: &[&str] = &[
    "/steps/*/run/outcome/error/message",
    "/steps/*/reload/error/code",
    TRACE,
];

/// A run step's trace, and every pointer under it: `tests/traces.rs`'s.
const TRACE: &str = "/steps/*/run/trace";

/// The deviation paths of `case` the run comparison reads: those under
/// `/build` or `/steps`, but not under a run's trace.
fn run_paths(case: &Case) -> Vec<&str> {
    case.deviations
        .iter()
        .flat_map(|d| d.paths.iter().map(String::as_str))
        .filter(|p| *p == "/build" || *p == "/steps" || p.starts_with("/steps/"))
        .filter(|p| !under_ignored(p, &[TRACE]))
        .collect()
}

/// Whether some deviation of `case` covers `/build`.
fn covers_build(case: &Case) -> bool {
    run_paths(case).contains(&"/build")
}

/// Why a case v4 has no build for fails, beyond its `/build` row: v3 built
/// and no deviation covers `/build`; or v3 built and `ran` steps, none of
/// which is compared, and no deviation covers `/steps`.
fn unexplained_no_build(name: &str, case: &Case, v3_built: bool, ran: bool) -> Vec<String> {
    let mut failures = Vec::new();
    if v3_built && !covers_build(case) {
        failures.push(format!(
            "{name}: v4 has no build where v3 built, and no deviation covers /build"
        ));
    }
    if v3_built && ran && !run_paths(case).contains(&"/steps") {
        failures.push(format!(
            "{name}: v4 has no build, so none of the steps v3 recorded is compared, and no \
             deviation covers /steps"
        ));
    }
    failures
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Counts {
    run: usize,
    no_build: usize,
    other_build: usize,
    run_deviated: usize,
}

#[tokio::test]
async fn v4_runs_every_case_as_v3_recorded_it() {
    let base: Vec<&str> = IGNORED.iter().chain(RUN_IGNORED).copied().collect();
    let mut counts = Counts::default();
    let mut held = Held::new("RUN_DEVIATED", RUN_DEVIATED);
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
                held.check(
                    &name,
                    &["/build"],
                    (&expected, &actual),
                    &base,
                    &mut failures,
                );
            }
            let ran =
                matches!(recording.get("steps"), Some(Json::Array(steps)) if !steps.is_empty());
            let v4 = actual.get("build").map_or_else(String::new, Json::to_text);
            failures.extend(
                unexplained_no_build(&name, &case, v3_built, ran)
                    .into_iter()
                    .map(|failure| format!("{failure}\n  v4: {v4}")),
            );
            continue;
        }
        if !v3_built && !covers_build(&case) {
            failures.push(format!(
                "{name}: v4 builds what v3 refused, and no deviation covers /build"
            ));
            continue;
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
        held.check(&name, &deviated, (&expected, &actual), &base, &mut failures);

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
    counts.run_deviated = held.finish(&mut failures);
    println!("{counts:?}");
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
    assert_eq!(
        counts,
        Counts {
            run: RUN,
            no_build: NO_BUILD,
            other_build: OTHER_BUILD,
            run_deviated: RUN_DEVIATED_ROWS,
        }
    );
}

/// A case with one workflow and one run step, deviating at `paths` under
/// break 15.
fn case_deviating(paths: &[&str]) -> Case {
    let deviation = serde_json::json!({"break": "15", "paths": paths, "note": "n"});
    serde_json::from_value(serde_json::json!({
        "description": "d", "covers": [], "workflows": [{"id": "w"}],
        "steps": [{"run": {"message": {}}}], "deviations": [deviation],
    }))
    .unwrap()
}

#[test]
fn no_build_where_v3_built_and_ran_needs_build_and_steps_deviated() {
    let both = case_deviating(&["/build", "/steps"]);
    assert!(unexplained_no_build("c", &both, true, true).is_empty());
    // v3's steps go uncompared: a deviation must say so.
    let build_only = case_deviating(&["/build"]);
    assert_eq!(
        unexplained_no_build("c", &build_only, true, true),
        [
            "c: v4 has no build, so none of the steps v3 recorded is compared, and no deviation \
          covers /steps"
        ]
    );
    // Nothing to explain where v3 refused the build too, or recorded no
    // step.
    assert!(unexplained_no_build("c", &build_only, false, false).is_empty());
    assert!(unexplained_no_build("c", &build_only, true, false).is_empty());
    let neither = case_deviating(&["/definitions"]);
    assert_eq!(
        unexplained_no_build("c", &neither, true, false),
        ["c: v4 has no build where v3 built, and no deviation covers /build"]
    );
}

#[test]
fn a_trace_pointer_is_left_to_the_traces_test() {
    let case = case_deviating(&[
        "/steps/0/run/message/errors/1",
        "/steps/0/run/trace",
        "/steps/0/run/trace/steps/0/message/context/data/partial",
        "/steps/1/run/trace/steps",
        "/steps",
    ]);
    assert_eq!(
        run_paths(&case),
        ["/steps/0/run/message/errors/1", "/steps"]
    );
}
