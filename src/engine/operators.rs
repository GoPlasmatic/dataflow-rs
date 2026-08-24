//! The operator vocabulary this build evaluates.
//!
//! The engine always runs datalogic in *templating* mode, where an unknown
//! operator name is not an error — the object echoes back as literal data.
//! That is the right execution semantics, and it makes one authoring-side
//! question both essential and otherwise unanswerable: **is this single-key
//! object a live operator call, or inert data?**
//!
//! A lint that warns on a probably-misspelled expression needs the exact
//! vocabulary of the running build, which depends on compiled cargo features
//! and on operators registered via
//! [`EngineBuilder::with_datalogic_operator`](crate::EngineBuilder::with_datalogic_operator).
//! [`Engine::operator_names`](crate::Engine::operator_names) reports all of it.
//!
//! # Why the core list lives here
//!
//! datalogic-rs keeps its name table (`OPCODE_NAMES`) private and exposes no
//! accessor, so the core vocabulary has no reachable source of truth outside
//! that crate. Mirroring it here replaces *N* host-side copies with one, beside
//! the `#[cfg]` gates that decide which families are live — and
//! `operator_names_are_all_live` checks every name in it against the running
//! engine, so a name that stops being an operator fails a test here rather than
//! silently weakening a downstream lint.
//!
//! The real fix is upstream: a `builtin_operator_names()` on datalogic's own
//! engine, next to the table it describes. The signature here does not change
//! when that lands.

/// Names datalogic evaluates with no extension family enabled.
const CORE: &[&str] = &[
    "val",
    "var",
    "==",
    "===",
    "!=",
    "!==",
    ">",
    ">=",
    "<",
    "<=",
    "!",
    "!!",
    "and",
    "or",
    "if",
    "?:",
    "+",
    "-",
    "*",
    "/",
    "%",
    "max",
    "min",
    "cat",
    "substr",
    "in",
    "merge",
    "filter",
    "map",
    "reduce",
    "all",
    "some",
    "none",
    "missing",
    "missing_some",
];

#[cfg(feature = "datetime")]
const DATETIME: &[&str] = &[
    "datetime",
    "timestamp",
    "parse_date",
    "format_date",
    "date_diff",
    "now",
];
#[cfg(not(feature = "datetime"))]
const DATETIME: &[&str] = &[];

#[cfg(feature = "ext-string")]
const EXT_STRING: &[&str] = &[
    "length",
    "starts_with",
    "ends_with",
    "upper",
    "lower",
    "trim",
    "split",
];
#[cfg(not(feature = "ext-string"))]
const EXT_STRING: &[&str] = &[];

#[cfg(feature = "ext-array")]
const EXT_ARRAY: &[&str] = &["sort", "slice", "group_by", "distinct"];
#[cfg(not(feature = "ext-array"))]
const EXT_ARRAY: &[&str] = &[];

#[cfg(feature = "ext-object")]
const EXT_OBJECT: &[&str] = &["keys", "values", "entries"];
#[cfg(not(feature = "ext-object"))]
const EXT_OBJECT: &[&str] = &[];

#[cfg(feature = "ext-control")]
const EXT_CONTROL: &[&str] = &["exists", "??", "switch", "match", "type"];
#[cfg(not(feature = "ext-control"))]
const EXT_CONTROL: &[&str] = &[];

#[cfg(feature = "ext-math")]
const EXT_MATH: &[&str] = &["abs", "ceil", "floor"];
#[cfg(not(feature = "ext-math"))]
const EXT_MATH: &[&str] = &[];

#[cfg(feature = "error-handling")]
const ERROR_HANDLING: &[&str] = &["try", "throw"];
#[cfg(not(feature = "error-handling"))]
const ERROR_HANDLING: &[&str] = &[];

/// Every operator name this build evaluates, before custom registrations.
///
/// The extension families are `#[cfg]`-gated exactly as they are in the crate's
/// `Cargo.toml`, so enabling `ext-string` adds `length` here and makes
/// `{"length": …}` a live call rather than inert data — the same switch, seen
/// from the authoring side.
///
/// Generic over the lifetime so the result chains with names borrowed from an
/// engine's custom-operator registry: `&'static str` coerces to any `&'a str`,
/// but an opaque `impl Iterator<Item = &'static str>` does not.
pub(crate) fn builtin_operator_names<'a>() -> impl Iterator<Item = &'a str> {
    CORE.iter()
        .chain(DATETIME)
        .chain(EXT_STRING)
        .chain(EXT_ARRAY)
        .chain(EXT_OBJECT)
        .chain(EXT_CONTROL)
        .chain(EXT_MATH)
        .chain(ERROR_HANDLING)
        .map(|name| -> &'a str { name })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn no_name_is_listed_twice() {
        let mut seen = HashSet::new();
        for name in builtin_operator_names() {
            assert!(
                seen.insert(name),
                "'{name}' appears in more than one family"
            );
        }
    }

    #[test]
    fn the_core_family_is_never_gated_away() {
        let names: HashSet<&str> = builtin_operator_names().collect();
        for expected in ["var", "if", "==", "map", "missing"] {
            assert!(names.contains(expected), "'{expected}' is core");
        }
    }

    /// Both directions of the family gates, as the repo requires — never test
    /// only `--all-features`.
    #[test]
    fn family_names_track_their_feature() {
        let names: HashSet<&str> = builtin_operator_names().collect();

        #[cfg(feature = "ext-string")]
        assert!(
            names.contains("length"),
            "ext-string is on, so `length` is live"
        );
        #[cfg(not(feature = "ext-string"))]
        assert!(
            !names.contains("length"),
            "ext-string is off, so `length` is inert data and must not be listed"
        );

        #[cfg(feature = "datetime")]
        assert!(names.contains("now"));
        #[cfg(not(feature = "datetime"))]
        assert!(!names.contains("now"));

        #[cfg(feature = "ext-control")]
        assert!(names.contains("switch"));
        #[cfg(not(feature = "ext-control"))]
        assert!(!names.contains("switch"));
    }
}
