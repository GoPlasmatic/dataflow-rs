//! Records the corpus on this build of v3: `record [filter]`.
//!
//! The default build writes every case that needs no feature on. A build with
//! features on writes the cases that need one, and checks that every other
//! runnable case records exactly what the default build committed, so a case
//! that silently depends on a feature fails here rather than in CI. Each case
//! runs twice, and a case whose two recordings differ is refused.

use dataflow_corpus::compare::{IGNORED, diff};
use dataflow_corpus::{Json, case_name, load_cases, recording_path};
use dataflow_corpus_v3::{Features, record_case};
use std::fs;
use std::process::ExitCode;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let filter = std::env::args().nth(1);
    let features = Features::detect();
    let (mut written, mut checked, mut skipped) = (0, 0, 0);
    let mut failures = Vec::new();

    for (path, case) in load_cases() {
        let name = case_name(&path);
        if filter.as_ref().is_some_and(|f| !name.contains(f.as_str())) {
            continue;
        }
        if !features.satisfies(&case.requires) {
            skipped += 1;
            continue;
        }
        let first = record_case(&case).await;
        let second = record_case(&case).await;
        let unstable = diff(&first, &second, &[]);
        if !unstable.is_empty() {
            failures.push(format!("{name}: two runs differ\n{}", list(&unstable)));
            continue;
        }
        let at = recording_path(&path);
        if features.is_default() || !case.requires.on.is_empty() {
            fs::write(&at, first.to_pretty()).expect("the recording is writable");
            written += 1;
            continue;
        }
        let committed = fs::read_to_string(&at)
            .ok()
            .and_then(|text| Json::from_text(&text).ok());
        match committed {
            None => failures.push(format!("{name}: no recording from the default build")),
            Some(committed) => {
                let differs = diff(&committed, &first, IGNORED);
                if differs.is_empty() {
                    checked += 1;
                } else {
                    failures.push(format!(
                        "{name}: records differently with {:?} on; declare its requirements\n{}",
                        features.on,
                        list(&differs)
                    ));
                }
            }
        }
    }

    println!("features on: {:?}", features.on);
    println!("{written} written, {checked} checked, {skipped} skipped");
    if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        eprintln!("{}", failures.join("\n\n"));
        ExitCode::FAILURE
    }
}

fn list(diffs: &[dataflow_corpus::compare::Diff]) -> String {
    diffs
        .iter()
        .take(10)
        .map(|d| format!("  {d}"))
        .collect::<Vec<_>>()
        .join("\n")
}
