//! Golden files: an output too large for a literal, held as text under
//! `tests/golden/`, key order included. `DATAFLOW_GOLDEN_WRITE=1`, and no
//! other value, rewrites them from the run; review the diff as a behaviour
//! change.

use std::path::{Path, PathBuf};

/// Whether this run rewrites golden files.
pub(crate) fn writing() -> bool {
    std::env::var_os("DATAFLOW_GOLDEN_WRITE").is_some_and(|v| v == "1")
}

/// The last segment of `module_path!()`, naming its golden directory under
/// `tests/golden/`.
pub(crate) fn module_dir(module_path: &str) -> &str {
    module_path.rsplit("::").next().unwrap_or(module_path)
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

#[test]
fn every_golden_file_and_lib_entry_is_claimed_by_a_test() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let golden_root = root.join("tests/golden");
    let known_dirs = [
        "corpus",
        "error_context",
        "fan_out",
        "lib",
        "run_loops",
        "trace",
    ];

    for entry in std::fs::read_dir(&golden_root).expect("tests/golden exists") {
        let path = entry.expect("readable entry").path();
        if path.is_dir() {
            let name = path.file_name().unwrap().to_str().unwrap();
            assert!(
                known_dirs.contains(&name),
                "unexpected directory under tests/golden: {name}"
            );
        }
    }

    for dir in ["error_context", "fan_out", "run_loops", "trace"] {
        let source_path = root.join("tests/it").join(format!("{dir}.rs"));
        let source = std::fs::read_to_string(&source_path)
            .unwrap_or_else(|e| panic!("{}: {e}", source_path.display()));
        for entry in std::fs::read_dir(golden_root.join(dir)).expect("golden dir exists") {
            let path = entry.expect("readable entry").path();
            if path.extension().is_some_and(|e| e == "json") {
                let stem = path.file_stem().unwrap().to_str().unwrap();
                let needle = format!("\"{stem}\"");
                assert!(
                    source.contains(&needle),
                    "orphan golden file {} not referenced in {}",
                    path.display(),
                    source_path.display()
                );
            }
        }
    }

    // Check `tests/golden/lib/**/*.json` files and their top-level test keys.
    let mut lib_sources = String::new();
    collect_rs(&root.join("src"), &mut lib_sources);
    let mut lib_goldens = Vec::new();
    collect_json(&golden_root.join("lib"), &mut lib_goldens);
    assert!(!lib_goldens.is_empty(), "lib goldens exist");
    for path in lib_goldens {
        let text = std::fs::read_to_string(&path).expect("golden file reads");
        let parsed: serde_json::Value =
            serde_json::from_str(&text).expect("lib golden is valid JSON");
        let obj = parsed.as_object().expect("lib golden is a JSON object");
        assert!(!obj.is_empty(), "empty lib golden: {}", path.display());
        for key in obj.keys() {
            let fn_decl = format!("fn {key}(");
            let literal = format!("\"{key}\"");
            assert!(
                lib_sources.contains(&fn_decl) && lib_sources.contains(&literal),
                "orphan lib golden entry `{key}` in {}",
                path.display()
            );
        }
    }
}

fn collect_rs(dir: &Path, out: &mut String) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs")
            && let Ok(text) = std::fs::read_to_string(&path)
        {
            out.push_str(&text);
            out.push('\n');
        }
    }
}

fn collect_json(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_json(&path, out);
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
}
