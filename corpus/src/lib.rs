//! The corpus: workflow cases the engine's tests load, and the tools that read
//! them. See `corpus/README.md`.
//!
//! A case is `cases/<area>/<name>.json`. This crate knows the format, how to
//! normalise a wire message, the dot-path rules on its own JSON value, and how
//! to rebuild a trace's snapshots from its changes. It does not depend on
//! `dataflow-rs`, so it is an independent model the engine's tests check
//! against.

pub mod case;
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
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
}

/// Seeded xorshift64* PRNG shared by the property and path tests: no
/// external dependency, and every failure reproduces from its seed.
#[derive(Clone, Copy, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A generator seeded with `seed` (clamped to at least 1).
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    /// The next pseudo-random `u64`.
    #[expect(clippy::should_implement_trait, reason = "not an iterator")]
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A uniform index in `0..n`.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// True with probability `num / den`.
    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        self.next() % den < num
    }

    /// One element of `items`.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}
