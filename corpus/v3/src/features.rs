//! Which features this build has, read from the engine rather than `cfg!`:
//! in a `--workspace` build, `wasm/` turns `all-operators` on in dataflow-rs
//! whatever this crate asks for. `budget` adds no operator, and
//! `with_ops_budget` is only callable under this crate's own `budget`, so it
//! alone is read with `cfg!`.

use dataflow_corpus::case::Requires;
use dataflow_rs::Engine;
use std::collections::BTreeSet;

/// Each family, and an operator only it adds.
const PROBES: &[(&str, &str)] = &[
    ("datetime", "datetime"),
    ("ext-string", "starts_with"),
    ("ext-array", "sort"),
    ("ext-math", "abs"),
    ("ext-control", "exists"),
    ("ext-object", "keys"),
    ("error-handling", "try"),
    ("tensor", "shape"),
];

/// What `all-operators` turns on: every family but `tensor`.
const ALL_OPERATORS: &[&str] = &[
    "datetime",
    "ext-string",
    "ext-array",
    "ext-math",
    "ext-control",
    "ext-object",
    "error-handling",
];

#[derive(Debug, PartialEq, Eq)]
pub struct Features {
    pub on: BTreeSet<&'static str>,
}

impl Features {
    pub fn detect() -> Self {
        let engine = Engine::builder().build().expect("an empty engine builds");
        let names: BTreeSet<&str> = engine.operator_names().collect();
        let mut on: BTreeSet<&'static str> = PROBES
            .iter()
            .filter(|(_, probe)| names.contains(probe))
            .map(|(family, _)| *family)
            .collect();
        if cfg!(feature = "budget") {
            on.insert("budget");
        }
        Self { on }
    }

    /// Nothing on: the build `cargo add dataflow-rs` delivers.
    pub fn is_default(&self) -> bool {
        self.on.is_empty()
    }

    pub fn satisfies(&self, requires: &Requires) -> bool {
        let expand = |names: &[String]| -> Vec<String> {
            names
                .iter()
                .flat_map(|name| match name.as_str() {
                    "all-operators" => ALL_OPERATORS.iter().map(|f| (*f).to_owned()).collect(),
                    other => vec![other.to_owned()],
                })
                .collect()
        };
        expand(&requires.on)
            .iter()
            .all(|f| self.on.contains(f.as_str()))
            && expand(&requires.off)
                .iter()
                .all(|f| !self.on.contains(f.as_str()))
    }
}
