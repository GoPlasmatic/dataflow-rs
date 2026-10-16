//! The two requirements on the published v3 name one exact version.
//!
//! `corpus/record.sh` records against `corpus/v3`'s pin, and a v3 fix that
//! re-records the corpus bumps it. The root's `dataflow_rs_v3` (v4's
//! differential tests and `host_call_floor`) moves with it. A bump that
//! misses one leaves `Cargo.lock` quietly carrying two 3.x versions, and
//! holds part of the repository to a v3 the corpus was not recorded from.
//! `wasm/` left the list in phase 7d and `docs-tests` in 7e, each when it
//! moved onto the root crate.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// The manifests, relative to the repository's root, and the dependency key
/// each names v3 by.
const PINS: [(&str, &str); 2] = [
    ("Cargo.toml", "dataflow_rs_v3"),
    ("corpus/v3/Cargo.toml", "dataflow-rs"),
];

/// The version requirement of every line that declares `key`, in either
/// spelling: `key = "req"` or `key = { ..., version = "req", ... }`.
fn requirements(manifest: &str, key: &str) -> Vec<String> {
    manifest
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once('=')?;
            if name.trim() != key {
                return None;
            }
            let value = value.trim();
            let quoted = if value.starts_with('{') {
                let (_, rest) = value.split_once("version")?;
                rest.trim_start().strip_prefix('=')?.trim_start()
            } else {
                value
            };
            let (req, _) = quoted.strip_prefix('"')?.split_once('"')?;
            Some(req.to_owned())
        })
        .collect()
}

/// `=X.Y.Z`, with three numeric parts.
fn is_exact(req: &str) -> bool {
    req.strip_prefix('=').is_some_and(|version| {
        let parts: Vec<&str> = version.split('.').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    })
}

#[test]
fn the_v3_pins_name_one_exact_version() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut by_requirement: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut problems = Vec::new();
    for (file, key) in PINS {
        let path = root.join(file);
        let manifest = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let found = requirements(&manifest, key);
        if found.is_empty() {
            problems.push(format!("{file} declares no `{key}` with a version"));
        }
        for req in found {
            if !is_exact(&req) {
                problems.push(format!(
                    "{file}'s `{key}` is \"{req}\", not an exact \"=X.Y.Z\""
                ));
            }
            by_requirement
                .entry(req)
                .or_default()
                .push(format!("{file} (`{key}`)"));
        }
    }
    if by_requirement.len() > 1 {
        let named: Vec<String> = by_requirement
            .iter()
            .map(|(req, files)| format!("\"{req}\" in {}", files.join(", ")))
            .collect();
        problems.push(format!(
            "the v3 pins differ, so bump them together: {}",
            named.join("; ")
        ));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
