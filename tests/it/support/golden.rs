//! Golden files: an output too large for a literal, held as text under
//! `tests/golden/`, key order included. `DATAFLOW_GOLDEN_WRITE=1`, and no
//! other value, rewrites them from the run; review the diff as a behaviour
//! change.

use std::path::{Path, PathBuf};

/// Whether this run rewrites golden files.
pub(crate) fn writing() -> bool {
    std::env::var_os("DATAFLOW_GOLDEN_WRITE").is_some_and(|v| v == "1")
}

/// `tests/golden/<dir>/<name>.json`.
pub(crate) fn file(dir: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(dir)
        .join(format!("{name}.json"))
}

/// Holds `text` to `file`, or rewrites the file when [`writing`].
#[track_caller]
pub(crate) fn hold(file: &Path, text: &str) {
    let shown = file.display();
    if writing() {
        std::fs::create_dir_all(file.parent().expect("a golden file has a directory"))
            .expect("the golden directory is writable");
        std::fs::write(file, text).expect("the golden file is writable");
        return;
    }
    let want = std::fs::read_to_string(file)
        .unwrap_or_else(|e| panic!("{shown}: {e}; DATAFLOW_GOLDEN_WRITE=1 writes it"));
    assert_eq!(
        text, want,
        "the run differs from {shown}; DATAFLOW_GOLDEN_WRITE=1 rewrites it"
    );
}
