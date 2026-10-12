extern crate dataflow_rs_v4 as dataflow_rs;

use dataflow_corpus::{Json, cases_dir, is_recording};
use dataflow_rs::run::ErrorRecord;
use std::fs;
use std::path::{Path, PathBuf};

fn recordings(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("a readable directory") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            recordings(&path, out);
        } else if is_recording(&path) {
            out.push(path);
        }
    }
}

/// Every record under `errors` arrays: `steps[*].run.message.errors`, and the
/// message snapshot of every trace step, `steps[*].run.trace.steps[*].message.errors`.
fn error_arrays(recording: &Json) -> Vec<&Json> {
    let mut out = Vec::new();
    let Some(Json::Array(steps)) = recording.get("steps") else {
        return out;
    };
    for step in steps {
        let Some(run) = step.get("run") else { continue };
        if let Some(errors) = run.get("message").and_then(|m| m.get("errors")) {
            out.push(errors);
        }
        if let Some(Json::Array(trace_steps)) = run.get("trace").and_then(|t| t.get("steps")) {
            for ts in trace_steps {
                if let Some(errors) = ts.get("message").and_then(|m| m.get("errors")) {
                    out.push(errors);
                }
            }
        }
    }
    out
}

#[test]
fn every_recorded_error_record_round_trips_byte_for_byte() {
    let mut files = Vec::new();
    recordings(&cases_dir(), &mut files);
    files.sort();
    assert!(!files.is_empty(), "no recordings found");

    let mut checked = 0usize;
    for file in &files {
        let text = fs::read_to_string(file).expect("a readable recording");
        let recording = Json::from_text(&text).expect("a recording is JSON");
        for errors in error_arrays(&recording) {
            let Json::Array(records) = errors else {
                panic!("{}: errors is not an array", file.display());
            };
            for record in records {
                let want = record.to_text();
                let parsed: ErrorRecord = serde_json::from_str(&want)
                    .unwrap_or_else(|e| panic!("{}: {e}: {want}", file.display()));
                let got = Json::from_text(&serde_json::to_string(&parsed).unwrap())
                    .unwrap()
                    .to_text();
                assert_eq!(got, want, "{}", file.display());
                checked += 1;
            }
        }
    }
    println!(
        "checked {checked} ErrorRecords in {} recordings",
        files.len()
    );
    assert!(checked > 0, "no ErrorRecord was found: the test is vacuous");
}
