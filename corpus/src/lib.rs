//! The golden corpus: cases recorded from v3, and the version-independent tools
//! that read them. See `corpus/README.md`.
//!
//! A case is `cases/<area>/<name>.json`; what v3 did with it is
//! `cases/<area>/<name>.v3.json`, beside it. This crate knows the format, how
//! to normalise and compare a recording, v3's path rules, and how to rebuild a
//! trace's snapshots from its changes. It does not depend on `dataflow-rs`:
//! v3's driver is `corpus/v3`, and v4's will sit beside it.

pub mod case;
pub mod compare;
pub mod json;
pub mod normalise;
pub mod path;
pub mod replay;

pub use case::Case;
pub use json::Json;

use std::fs;
use std::path::{Path, PathBuf};

/// `corpus/cases`.
pub fn cases_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("cases")
}

/// `x.json` to `x.v3.json`.
pub fn recording_path(case: &Path) -> PathBuf {
    case.with_extension("v3.json")
}

/// Is this file a recording rather than a case?
pub fn is_recording(path: &Path) -> bool {
    path.to_string_lossy().ends_with(".v3.json")
}

/// Every case under `cases/`, sorted by path. Panics naming the file when one
/// does not parse, because a case that silently drops out tests nothing.
pub fn load_cases() -> Vec<(PathBuf, Case)> {
    let mut paths = Vec::new();
    collect(&cases_dir(), &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text =
                fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let case =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            (path, case)
        })
        .collect()
}

/// The case's path relative to `cases/`, without `.json`: its name in reports.
pub fn case_name(path: &Path) -> String {
    let relative = path.strip_prefix(cases_dir()).unwrap_or(path);
    relative.with_extension("").to_string_lossy().into_owned()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("a readable directory entry").path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "json") && !is_recording(&path) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_sits_beside_its_case() {
        let case = Path::new("/c/cases/run/x.json");
        assert_eq!(recording_path(case), Path::new("/c/cases/run/x.v3.json"));
        assert!(is_recording(&recording_path(case)));
        assert!(!is_recording(case));
    }
}
