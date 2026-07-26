//! Guards the hand-maintained page list in `src/lib.rs`: every Markdown page
//! under `docs/src` must either appear in a `doc_pages!` entry or be named in
//! `SKIPPED` with a reason. A new docs page that is neither fails this test
//! instead of silently shipping unverified examples.

use std::fs;
use std::path::{Path, PathBuf};

/// Pages deliberately not compiled as doctests.
const SKIPPED: &[&str] = &[
    // Navigation index, not a content page.
    "SUMMARY.md",
    // Interactive wasm playground; contains no Rust snippets.
    "playground.md",
];

fn collect_md(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read docs/src directory") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            collect_md(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            out.push(path);
        }
    }
}

#[test]
fn every_docs_page_is_listed() {
    let docs_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/src");
    let lib_rs = include_str!("../src/lib.rs");

    let mut pages = Vec::new();
    collect_md(&docs_src, &mut pages);
    assert!(!pages.is_empty(), "no pages found under docs/src");

    let rel_paths: Vec<String> = pages
        .iter()
        .map(|page| {
            let rel = page.strip_prefix(&docs_src).expect("page under docs/src");
            rel.to_str().expect("utf-8 path").replace('\\', "/")
        })
        .collect();

    for skip in SKIPPED {
        assert!(
            rel_paths.iter().any(|rel| rel == skip),
            "stale SKIPPED entry {skip:?}: no such page under docs/src"
        );
    }

    let mut missing: Vec<&String> = rel_paths
        .iter()
        .filter(|rel| {
            !SKIPPED.contains(&rel.as_str()) && !lib_rs.contains(&format!("../../docs/src/{rel}"))
        })
        .collect();
    missing.sort();

    assert!(
        missing.is_empty(),
        "docs pages missing from doc_pages! in docs-tests/src/lib.rs \
         (add them, or add them to SKIPPED with a reason): {missing:?}"
    );
}
