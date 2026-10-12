//! The corpus is green on v3: every case this build can run records exactly
//! what is committed beside it. CI runs this in the all-features build (with
//! `--workspace`) and in the default build (`-p dataflow-corpus-v3`), so every
//! case runs in one of them.

use dataflow_corpus::compare::{IGNORED, diff};
use dataflow_corpus::{Json, case_name, load_cases, recording_path};
use dataflow_corpus_v3::{Features, record_case};
use std::fs;

#[tokio::test]
async fn every_runnable_case_matches_its_recording() {
    let features = Features::detect();
    let (mut run, mut skipped) = (0, 0);
    let mut failures = Vec::new();
    for (path, case) in load_cases() {
        if !features.satisfies(&case.requires) {
            skipped += 1;
            continue;
        }
        run += 1;
        let name = case_name(&path);
        let Ok(text) = fs::read_to_string(recording_path(&path)) else {
            failures.push(format!("{name}: no recording; run corpus/record.sh"));
            continue;
        };
        let expected = Json::from_text(&text).expect("a recording parses");
        let actual = record_case(&case).await;
        let differs = diff(&expected, &actual, IGNORED);
        if !differs.is_empty() {
            let shown: Vec<String> = differs.iter().take(10).map(|d| format!("  {d}")).collect();
            failures.push(format!(
                "{name}: {} differences\n{}",
                differs.len(),
                shown.join("\n")
            ));
        }
    }
    println!(
        "features on: {:?}; {run} run, {skipped} skipped",
        features.on
    );
    assert!(run > 0, "no case ran");
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}
