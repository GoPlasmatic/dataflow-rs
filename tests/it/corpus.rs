// The corpus end to end: every case under `corpus/cases/` the build
// satisfies, run through the driver (`support/corpus_driver.rs`, which holds
// every run to the rules no case states) and held whole, key order included,
// to its golden file, `tests/golden/corpus/<area>/<name>.json`.
// `DATAFLOW_GOLDEN_WRITE=1` rewrites them; review the diff against each
// case's description, since a golden file only freezes what the engine does.
//
// A case whose `requires` this build does not satisfy is skipped, and the
// skips are pinned in the two builds CI judges whole: the default and every
// feature on.

use crate::support::corpus_driver::{self, Options};
use crate::support::golden;
use dataflow_corpus::{case_name, load_cases};
use futures::FutureExt;
use std::collections::BTreeSet;
use std::panic::AssertUnwindSafe;
use std::path::Path;

/// The cases a build skips for their `requires`: in the default build, and
/// with every feature on.
const SKIPPED_BY_DEFAULT: &[&str] = &["expressions/budget", "expressions/families_on"];
const SKIPPED_WITH_EVERYTHING: &[&str] = &["expressions/families_off"];

/// Whether this build has `datetime`, `tensor` and `budget` on.
const EVERYTHING: bool = cfg!(all(
    feature = "datetime",
    feature = "tensor",
    feature = "budget"
));

/// Whether this build has none on.
const NOTHING: bool = !cfg!(any(
    feature = "datetime",
    feature = "tensor",
    feature = "budget"
));

#[tokio::test]
async fn every_case_runs_as_its_golden_file_holds() {
    let mut skipped = Vec::new();
    let mut held = BTreeSet::new();
    // Every case runs, and every failing one is named, rather than the
    // first alone.
    let mut failed = Vec::new();
    for (path, case) in load_cases() {
        let name = case_name(&path);
        if !corpus_driver::satisfied(&case.requires) {
            skipped.push(name);
            continue;
        }
        let checked = AssertUnwindSafe(async {
            let ran = corpus_driver::run_case(&name, &case, Options::default(), &mut ()).await;
            golden::hold(&golden::file("corpus", &name), &ran.to_pretty());
        });
        if checked.catch_unwind().await.is_err() {
            failed.push(name.clone());
        }
        held.insert(name);
    }
    assert!(
        failed.is_empty(),
        "{} cases failed: {failed:?}",
        failed.len()
    );
    if NOTHING {
        assert_eq!(skipped, SKIPPED_BY_DEFAULT, "skipped in the default build");
    }
    if EVERYTHING {
        assert_eq!(
            skipped, SKIPPED_WITH_EVERYTHING,
            "skipped with every feature on"
        );
    }
    // A golden file names a case: one left behind by a renamed or deleted
    // case holds nothing.
    let dir = golden::file("corpus", "x");
    let dir = dir.parent().expect("the corpus golden directory");
    let mut orphans = Vec::new();
    collect(dir, dir, &mut orphans);
    orphans.retain(|name| !held.contains(name) && !skipped.contains(name));
    assert!(orphans.is_empty(), "golden files with no case: {orphans:?}");
}

/// Every golden file under `dir`, as its case's name.
fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("a readable directory entry").path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "json") {
            let name = path
                .strip_prefix(root)
                .expect("below the root")
                .with_extension("");
            out.push(name.to_string_lossy().replace('\\', "/"));
        }
    }
}
