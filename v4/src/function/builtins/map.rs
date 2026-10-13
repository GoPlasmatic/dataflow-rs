//! `map`'s config: JSONLogic-driven writes to context paths, and removals.
//!
//! The rules between a mapping's keys are v3's (`AuthoredMapping::problem`),
//! message for message, and `tests/map_vs_v3.rs` holds them to it.

use crate::base::IssueCode;
use crate::expr::{CompileCx, Expr, Path};
use crate::function::FunctionConfig;
use datavalue::OwnedDataValue;
use serde::Deserialize;

/// `map`'s input.
#[derive(Debug, FunctionConfig)]
#[non_exhaustive]
pub struct MapConfig {
    /// The mappings, applied in order.
    pub mappings: Vec<Mapping>,
}

/// One write, or one removal.
#[derive(Debug, FunctionConfig)]
#[config(check = Mapping::rules)]
#[non_exhaustive]
pub struct Mapping {
    /// Where the result is written: a context path, or JSONLogic computing one.
    #[config(records)]
    pub path: Expr<Path>,
    /// The value to write. An explicit `null` is a constant `null`, which is
    /// skipped and so never writes.
    #[config(records)]
    pub logic: Option<Expr>,
    /// Remove the key at `path` instead of writing to it. Takes no `logic`.
    #[config(default = false)]
    pub unset: bool,
    /// What a `null` result does: `skip` (the default) or `unset`.
    #[config(serde)]
    pub on_null: Option<OnNull>,
    /// How a non-null result is written: `set` (the default), `append` or
    /// `extend`.
    #[config(serde)]
    pub mode: Option<MapMode>,
}

/// What a [`Mapping`] does when its `logic` evaluates to `null`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum OnNull {
    /// Leave the path as it was.
    Skip,
    /// Remove the key at the path.
    Unset,
}

/// How a [`Mapping`] writes a non-null result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum MapMode {
    /// Replace the value at the path (merging, at a context root).
    Set,
    /// Push the result onto the array at the path.
    Append,
    /// Push each element of an array result onto the array at the path.
    Extend,
}

impl Mapping {
    /// The rules between the keys: the first one broken is `INVALID_MAPPING`
    /// at its key, as v3 reports it. A mapping that breaks none but whose
    /// `logic` is a constant `null` never writes, which is the `NULL_MAPPING`
    /// Advisory.
    fn rules(&self, cx: &mut CompileCx<'_>) {
        if let Some((key, message)) = self.problem() {
            cx.at(key, |cx| cx.issue(IssueCode::InvalidMapping, message));
            return;
        }
        let always_null = self
            .logic
            .as_ref()
            .is_some_and(|logic| matches!(logic.constant(), Some(OwnedDataValue::Null)));
        if always_null && !self.unset && self.on_null != Some(OnNull::Unset) {
            let path = self
                .path
                .constant()
                .map_or_else(|| self.path.raw().to_string(), ToString::to_string);
            cx.at("logic", |cx| {
                cx.issue(
                    IssueCode::NullMapping,
                    format!(
                        "this mapping's logic is always null, and a null result is skipped, \
                         so it never writes '{path}' — to remove the path use `\"unset\": true`"
                    ),
                );
            });
        }
    }

    /// The first rule between the keys this mapping breaks: the key it is
    /// reported at, and v3's message.
    fn problem(&self) -> Option<(&'static str, &'static str)> {
        match (&self.logic, self.unset) {
            (None, false) => {
                return Some((
                    "logic",
                    "a mapping needs `logic`, or `\"unset\": true` to remove the path",
                ));
            }
            (Some(_), true) => {
                return Some((
                    "unset",
                    "`unset` removes the path and takes no `logic` — to remove it only \
                     when the result is null, keep `logic` and use `\"on_null\": \"unset\"`",
                ));
            }
            _ => {}
        }
        if self.logic.is_none() && self.on_null.is_some() {
            return Some((
                "on_null",
                "`on_null` says what a null `logic` result does, and this mapping has no `logic`",
            ));
        }
        if self.logic.is_none() && self.mode.is_some() {
            return Some((
                "mode",
                "`mode` says how a `logic` result is written, and this mapping has no `logic`",
            ));
        }
        // v3's rule: only an authored string naming a root. A computed path
        // that folds to one is left to the write, as v3 left it.
        let literal_root =
            self.path.raw().is_string() && self.path.constant().is_some_and(Path::is_root);
        if literal_root && (self.unset || self.on_null == Some(OnNull::Unset)) {
            return Some((
                "path",
                "a context root (`data`, `metadata`, `temp_data`) cannot be removed — \
                 remove the keys under it instead",
            ));
        }
        if literal_root && matches!(self.mode, Some(MapMode::Append | MapMode::Extend)) {
            return Some((
                "path",
                "a context root (`data`, `metadata`, `temp_data`) is an object and cannot be \
                 appended to — name an array under it instead",
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{AuthoredPath, Issue, Severity};
    use crate::expr::ExprEngine;
    use serde_json::{Value, json};

    fn compile(input: &Value) -> (Option<MapConfig>, Vec<Issue>) {
        let engine = ExprEngine::default();
        let mut issues = Vec::new();
        let config = {
            let mut cx = CompileCx::new(&engine, &mut issues).at_path(AuthoredPath::root());
            MapConfig::compile(input, &mut cx)
        };
        (config, issues)
    }

    fn one(mapping: Value) -> (Option<MapConfig>, Vec<Issue>) {
        compile(&json!({"mappings": [mapping]}))
    }

    #[test]
    fn a_path_outside_the_three_roots_is_invalid_path() {
        // Break 15: v3 let `map` write any top-level context key.
        let (config, issues) = one(json!({"path": "extra.x", "logic": 1}));
        assert!(config.is_none());
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, IssueCode::InvalidPath);
        assert_eq!(issues[0].path.to_string(), "mappings[0].path");
    }

    #[test]
    fn a_literal_path_over_64_segments_is_invalid_path() {
        // I74: a `Path` holds at most 64 segments, the root's included.
        let deep = |n: usize| format!("data{}", ".k".repeat(n - 1));
        let (config, issues) = one(json!({"path": deep(64), "logic": 1}));
        assert!(config.is_some() && issues.is_empty(), "{issues:?}");
        let (config, issues) = one(json!({"path": deep(65), "logic": 1}));
        assert!(config.is_none());
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, IssueCode::InvalidPath);
        assert_eq!(issues[0].path.to_string(), "mappings[0].path");
    }

    #[test]
    fn a_computed_path_compiles() {
        let (config, issues) =
            one(json!({"path": {"cat": ["data.", {"var": "data.k"}]}, "logic": 1}));
        assert!(issues.is_empty(), "{issues:?}");
        let config = config.unwrap();
        assert!(config.mappings[0].path.constant().is_none());
    }

    #[test]
    fn a_mapping_without_a_path_is_missing_one() {
        let (config, issues) = one(json!({"logic": 1}));
        assert!(config.is_none());
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, IssueCode::InputParse);
        assert_eq!(issues[0].path.to_string(), "mappings[0].path");
        assert_eq!(issues[0].message, "missing field `path`");
    }

    #[test]
    fn a_mapping_compiles_with_its_keys() {
        let (config, issues) = one(json!({
            "path": "data.list", "logic": {"var": "data.y"}, "on_null": "unset", "mode": "append"
        }));
        assert!(issues.is_empty(), "{issues:?}");
        let m = &config.unwrap().mappings[0];
        assert_eq!(m.path.constant().unwrap().to_string(), "data.list");
        assert!(m.logic.as_ref().unwrap().constant().is_none());
        assert!(!m.unset);
        assert_eq!(m.on_null, Some(OnNull::Unset));
        assert_eq!(m.mode, Some(MapMode::Append));
    }

    #[test]
    fn an_unknown_spelling_is_input_parse() {
        let (config, issues) = one(json!({"path": "data.x", "logic": 1, "mode": "push"}));
        assert!(config.is_none());
        assert_eq!(issues[0].code, IssueCode::InputParse);
        assert_eq!(issues[0].path.to_string(), "mappings[0].mode");
    }

    #[test]
    fn only_the_first_broken_rule_is_reported() {
        // No `logic` and no `unset`, and `on_null` and `mode` without `logic`.
        let (config, issues) = one(json!({"path": "data", "on_null": "unset", "mode": "extend"}));
        assert!(config.is_none());
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, IssueCode::InvalidMapping);
        assert_eq!(issues[0].path.to_string(), "mappings[0].logic");
        assert_eq!(
            issues[0].message,
            "a mapping needs `logic`, or `\"unset\": true` to remove the path"
        );
    }

    #[test]
    fn a_root_cannot_be_removed_or_appended_to() {
        let (_, issues) = one(json!({"path": "data", "unset": true}));
        assert_eq!(issues[0].path.to_string(), "mappings[0].path");
        assert!(issues[0].message.contains("cannot be removed"));
        let (_, issues) = one(json!({"path": "metadata", "logic": 1, "mode": "append"}));
        assert_eq!(issues[0].path.to_string(), "mappings[0].path");
        assert!(issues[0].message.contains("cannot be appended to"));
        // As in v3, only a literal string names a root here: a computed path
        // that folds to one compiles, and refusing its write is the runtime's.
        let (config, issues) = one(json!({"path": {"cat": ["da", "ta"]}, "unset": true}));
        assert!(issues.is_empty(), "{issues:?}");
        assert!(
            config.unwrap().mappings[0]
                .path
                .constant()
                .unwrap()
                .is_root()
        );
        let (_, issues) = one(json!({"path": {"cat": ["temp", "_data"]}, "logic": 1,
                                     "mode": "extend"}));
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn a_null_mapping_is_an_advisory_naming_the_path() {
        let (config, issues) = one(json!({"path": "data.x", "logic": null}));
        assert!(config.is_some(), "an Advisory refuses nothing");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, IssueCode::NullMapping);
        assert_eq!(issues[0].severity, Severity::Advisory);
        assert_eq!(issues[0].path.to_string(), "mappings[0].logic");
        assert_eq!(
            issues[0].message,
            "this mapping's logic is always null, and a null result is skipped, so it never \
             writes 'data.x' — to remove the path use `\"unset\": true`"
        );

        // A computed path is named by its expression.
        let path = json!({"cat": ["data.", {"var": "data.k"}]});
        let (_, issues) = one(json!({"path": path, "logic": {"if": [false, 1, null]}}));
        assert_eq!(issues[0].code, IssueCode::NullMapping);
        assert!(issues[0].message.contains(&format!("'{path}'")));

        // Silent when a null result removes the path, or the logic can be non-null.
        let (_, issues) = one(json!({"path": "data.x", "logic": null, "on_null": "unset"}));
        assert!(issues.is_empty());
        let (_, issues) = one(json!({"path": "data.x", "logic": {"var": "data.y"}}));
        assert!(issues.is_empty());
    }

    #[test]
    fn the_fields_describe_a_mapping() {
        let fields = MapConfig::fields();
        assert_eq!(fields.len(), 1);
        assert!(fields[0].required);
        let crate::expr::FieldKind::List(element) = fields[0].kind else {
            panic!("a list");
        };
        let crate::expr::FieldKind::Nested(mapping) = element() else {
            panic!("a nested config");
        };
        let names: Vec<&str> = mapping().iter().map(|f| &*f.name).collect();
        assert_eq!(names, ["path", "logic", "unset", "on_null", "mode"]);
        let records: Vec<bool> = mapping().iter().map(|f| f.records).collect();
        assert_eq!(records, [true, true, false, false, false]);
        let required: Vec<bool> = mapping().iter().map(|f| f.required).collect();
        assert_eq!(required, [true, false, false, false, false]);
    }
}
