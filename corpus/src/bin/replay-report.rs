//! What v3's trace changes do not rebuild: `cargo run -p dataflow-corpus --bin replay-report`.
//!
//! For every recorded case with a trace, replays each run's steps from its
//! input context and lists the snapshot paths the changes do not reproduce,
//! then a summary of every distinct path and how many cases show it. v4's
//! trace is the initial context plus each step's changes (R17), so every path
//! here is something v4 must record, or its runner must derive, to rebuild a
//! snapshot.

use dataflow_corpus::replay::unrebuilt;
use dataflow_corpus::{Json, case_name, load_cases, recording_path};
use std::collections::BTreeMap;

fn main() {
    let mut summary: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (path, case) in load_cases() {
        let Ok(text) = std::fs::read_to_string(recording_path(&path)) else {
            continue;
        };
        let recording = Json::from_text(&text).expect("a recording parses");
        let name = case_name(&path);
        let paths = unrebuilt(&case, &recording);
        if !paths.is_empty() {
            println!(
                "{name}: {}",
                paths.iter().cloned().collect::<Vec<_>>().join(", ")
            );
        }
        for p in paths {
            summary.entry(p).or_default().push(name.clone());
        }
    }
    println!("\nSummary ({} paths):", summary.len());
    for (p, cases) in &summary {
        println!("  {p:<48} {} cases", cases.len());
    }
}
