//! `map`'s config: JSONLogic-driven writes to context paths, and removals.
//!
//! The rules between a mapping's keys are v3's (`AuthoredMapping::problem`),
//! message for message, kept for compatibility.

use crate::base::{Change, IssueCode, Result};
use crate::expr::arena::with_arena;
use crate::expr::{CompileCx, Expr, ExprEngine, Path};
use crate::function::{FunctionConfig, Outcome, SyncCtx, SyncFunction};
use crate::state::{Edit, kind};
use ::log::error;
use datavalue::{DataValue, OwnedDataValue};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt::Write as _;
use std::sync::Arc;

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum OnNull {
    /// Leave the path as it was.
    Skip,
    /// Remove the key at the path.
    Unset,
}

/// How a [`Mapping`] writes a non-null result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
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
            let path = self.describe();
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

impl SyncFunction for MapConfig {
    /// Each mapping in order, each against the view as the ones before it
    /// left it. A failing mapping is logged and the rest still run; the call
    /// is then `Status(500)`, as v3's. Under a trace that keeps changes,
    /// each mapping's end in the changes is noted, a failing one's included.
    fn call<'p>(&'p self, cx: &mut SyncCtx<'_, '_, 'p>) -> Result<Outcome> {
        let mut failed = false;
        // Most mappings record one change: sized once, as v3's
        // `with_capacity`, rather than grown by doubling.
        if cx.capture {
            cx.changes.reserve(self.mappings.len());
        }
        for mapping in &self.mappings {
            let (exprs, capture, traced) = (cx.exprs, cx.capture, cx.traced);
            let (changes, marked) = (&mut *cx.changes, &mut cx.marked);
            let ends = cx.mapping_ends.as_deref_mut();
            failed |= !cx.rc.edit(|e| {
                let mut w = Writer {
                    e,
                    exprs,
                    capture,
                    traced,
                    changes,
                    marked,
                    ends,
                };
                let applied = mapping.apply(&mut w);
                w.end();
                applied
            });
        }
        Ok(if failed {
            Outcome::Status(500)
        } else {
            Outcome::Ok
        })
    }
}

/// One mapping's write against the view, inside one edit: what it evaluates
/// with and where its changes go. The edit holds the run context, so a change
/// is pushed here rather than through [`SyncCtx::change`], by its rule: only
/// under capture, and only then is anything converted for it.
struct Writer<'w, 'e, 'q, 'p> {
    e: &'w mut Edit<'e, 'q, 'p>,
    exprs: &'p ExprEngine,
    capture: bool,
    /// The run's trace keeps changes: a write v3's audit has no row for is
    /// recorded for it alone.
    traced: bool,
    changes: &'w mut Vec<Change>,
    /// Set once such a change is recorded ([`SyncCtx::marked`]).
    marked: &'w mut bool,
    /// Where the mapping's end is noted, under a trace that keeps changes
    /// ([`SyncCtx::mapping_ends`]).
    ends: Option<&'w mut Vec<u32>>,
}

/// A mapping's result: its view, and the folded constant it came from.
#[derive(Clone, Copy)]
struct MapValue<'q, 'p> {
    view: DataValue<'q>,
    constant: Option<&'p OwnedDataValue>,
}

impl MapValue<'_, '_> {
    /// The result as an owned tree: the constant cloned, or the view
    /// converted.
    fn owned(self) -> OwnedDataValue {
        self.constant
            .map_or_else(|| self.view.to_owned(), Clone::clone)
    }
}

impl Mapping {
    /// How a log line names the destination: a constant path dotted, a
    /// computed one by its expression, as v3's `describe_path`.
    fn describe(&self) -> String {
        self.path
            .constant()
            .map_or_else(|| self.path.raw().to_string(), ToString::to_string)
    }

    /// Runs the mapping, v3's steps in v3's order: the logic, the `null`
    /// rule, then the destination and the write. `false` when it failed,
    /// after logging why.
    fn apply<'p>(&'p self, w: &mut Writer<'_, '_, '_, 'p>) -> bool {
        if self.unset {
            return self.destination(w).is_some_and(|path| w.remove(&path));
        }
        let logic = self
            .logic
            .as_ref()
            .expect("a mapping that does not unset has logic (`Mapping::problem`)");
        let result = match logic.constant() {
            // The folded constant, viewed: its leaves are the program's.
            Some(c) => MapValue {
                view: c.view_in(w.e.arena()),
                constant: Some(c),
            },
            // Evaluated in the run arena, so the result splices as it is.
            None => match w.exprs.eval_in(logic.logic(), w.e.root_ref(), w.e.arena()) {
                Ok(view) => MapValue {
                    view: *view,
                    constant: None,
                },
                Err(e) => {
                    error!(
                        "Map: Error evaluating logic for path {}: {e}",
                        self.describe()
                    );
                    return false;
                }
            },
        };
        if result.view.is_null() {
            return match self.on_null {
                None | Some(OnNull::Skip) => true,
                Some(OnNull::Unset) => self.destination(w).is_some_and(|path| w.remove(&path)),
            };
        }
        let Some(path) = self.destination(w) else {
            return false;
        };
        match self.mode {
            None | Some(MapMode::Set) => {
                w.set(&path, result);
                true
            }
            Some(mode) => w.append(&path, mode, result),
        }
    }

    /// Where the mapping writes: its constant path, or its computed one
    /// evaluated against the view and parsed. `None`, after logging, when the
    /// evaluation fails or its result is not a path (break 15: v3 wrote any
    /// top-level context key).
    fn destination<'p>(&'p self, w: &Writer<'_, '_, '_, 'p>) -> Option<Cow<'p, Path>> {
        if let Some(path) = self.path.constant() {
            return Some(Cow::Borrowed(path));
        }
        let root = w.e.root_ref();
        let resolved = with_arena(|arena| {
            w.exprs
                .eval_in(self.path.logic(), root, arena)
                .and_then(|view| self.path.convert(view))
        });
        match resolved {
            Ok(path) => Some(Cow::Owned(path)),
            Err(e) => {
                error!(
                    "Map: Error resolving destination path {}: {e}",
                    self.describe()
                );
                None
            }
        }
    }
}

impl<'q, 'p> Writer<'_, '_, 'q, 'p> {
    /// Notes where the mapping's changes end, when the trace lent a list
    /// for it: the count of the call's changes so far, which a mapping that
    /// wrote nothing leaves where the one before it did.
    fn end(&mut self) {
        if let Some(ends) = self.ends.as_deref_mut() {
            #[cfg(test)]
            crate::run::traced_work();
            ends.push(u32::try_from(self.changes.len()).unwrap_or(u32::MAX));
        }
    }

    /// Writes `result` at `path`: a root merges, anything else is assigned.
    /// The change is recorded first, with the old value read from the view;
    /// under capture the mark keeps the owned new value, which the `Change`
    /// needed anyway, so the checkpoint converts nothing twice.
    fn set(&mut self, path: &Path, result: MapValue<'q, 'p>) {
        let owned = self.capture.then(|| result.owned());
        if let Some(new_value) = &owned {
            let old_value = self.old(path);
            self.changes.push(Change::write(
                Arc::clone(path.dotted()),
                old_value,
                new_value.clone(),
            ));
        }
        if path.is_root() {
            self.e.merge_root(path.root().as_str(), result.view, owned);
        } else {
            self.e.set(path, result.view, owned);
        }
    }

    /// Adds `result` to the array at `path` (#69): one element for `append`,
    /// each element of an array for `extend`, recorded one change per
    /// element at its index. A missing or `null` target becomes an array;
    /// a root, another target, or an `extend` of a non-array fails.
    ///
    /// The audit entry records the elements alone, as v3's. A replay of
    /// those alone cannot rebuild an array created where a `null` was, nor
    /// an empty `extend`'s, so a traced run records the array's creation
    /// first, `null` to `[]`, for the trace alone.
    fn append(&mut self, path: &Path, mode: MapMode, result: MapValue<'q, 'p>) -> bool {
        if path.is_root() {
            error!("Map: Refusing to append to context root {path}");
            return false;
        }
        let items: &[DataValue<'q>] = match (mode, &result.view) {
            (MapMode::Extend, DataValue::Array(items)) => items,
            (MapMode::Extend, other) => {
                error!(
                    "Map: `extend` needs an array result for {path}, got {}",
                    kind(other)
                );
                return false;
            }
            _ => std::slice::from_ref(&result.view),
        };
        let (start, creates) = match self.e.get(path) {
            None | Some(DataValue::Null) => (0, true),
            Some(DataValue::Array(old)) => (old.len(), false),
            Some(other) => {
                error!(
                    "Map: Cannot append to {path}: it holds {}, not an array",
                    kind(&other)
                );
                return false;
            }
        };
        let owned = self.capture.then(|| match (mode, result.owned()) {
            (MapMode::Extend, OwnedDataValue::Array(items)) => items,
            (_, item) => vec![item],
        });
        if let Some(items) = &owned {
            if creates && self.traced {
                self.changes.push(Change::for_trace(
                    Arc::clone(path.dotted()),
                    OwnedDataValue::Null,
                    OwnedDataValue::Array(Vec::new()),
                ));
                *self.marked = true;
            }
            // One buffer for every element's path, `{path}.{index}`.
            let mut at = format!("{path}.");
            let base = at.len();
            for (i, item) in items.iter().enumerate() {
                at.truncate(base);
                let _ = write!(at, "{}", start + i);
                self.changes.push(Change::write(
                    at.as_str(),
                    OwnedDataValue::Null,
                    item.clone(),
                ));
            }
        }
        let appended = self.e.append(path, items, owned);
        debug_assert!(appended.is_ok(), "the target was checked: {appended:?}");
        true
    }

    /// Removes the key at `path`, recording the removal when something was
    /// there. A root cannot be removed.
    fn remove(&mut self, path: &Path) -> bool {
        if path.is_root() {
            error!("Map: Refusing to remove context root {path}");
            return false;
        }
        let old_value = self.capture.then(|| self.e.get(path)).flatten();
        if self.e.remove(path)
            && let Some(old_value) = old_value
        {
            self.changes.push(Change::removal(
                Arc::clone(path.dotted()),
                old_value.to_owned(),
            ));
        }
        true
    }

    /// The value at `path` before a write, converted; `null` when absent.
    fn old(&self, path: &Path) -> OwnedDataValue {
        self.e
            .get(path)
            .map_or(OwnedDataValue::Null, |v| v.to_owned())
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

    // -----------------------------------------------------------------------
    // The call
    // -----------------------------------------------------------------------

    use crate::expr::arena;
    use crate::function::builtins::one_task::{
        Golden, Observed, Task, run_once, seed, sorted, task,
    };
    use crate::state::{RunContext, Tuning};
    use std::borrow::Cow;

    /// `map` with `mappings` over `context`, with capture on and off, its
    /// whole observation held by `golden`; the capture-on run.
    fn both(golden: &mut Golden, mappings: Value, context: &Value) -> Observed {
        golden.observe("map", &json!({"mappings": mappings}), &Value::Null, context)
    }

    /// Every combination of `logic` (absent, present, `null`), `unset`
    /// (absent, `true`, `false`), `on_null` (absent, `skip`, `unset`), `mode`
    /// (absent, `set`, `append`, `extend`) and `path` (`data`, `data.x`).
    fn mapping_grid() -> Vec<Value> {
        let logics = [None, Some(json!({"var": "data.y"})), Some(Value::Null)];
        let unsets = [None, Some(true), Some(false)];
        let on_nulls = [None, Some("skip"), Some("unset")];
        let modes = [None, Some("set"), Some("append"), Some("extend")];
        let paths = ["data", "data.x"];
        let mut all = Vec::new();
        for logic in &logics {
            for unset in unsets {
                for on_null in on_nulls {
                    for mode in modes {
                        for path in paths {
                            let mut m = serde_json::Map::new();
                            m.insert("path".into(), json!(path));
                            if let Some(logic) = logic {
                                m.insert("logic".into(), logic.clone());
                            }
                            if let Some(unset) = unset {
                                m.insert("unset".into(), json!(unset));
                            }
                            if let Some(on_null) = on_null {
                                m.insert("on_null".into(), json!(on_null));
                            }
                            if let Some(mode) = mode {
                                m.insert("mode".into(), json!(mode));
                            }
                            all.push(Value::Object(m));
                        }
                    }
                }
            }
        }
        assert_eq!(all.len(), 216);
        all
    }

    /// The grid's contexts: the target `data.x` missing, an array or a
    /// scalar, each with the logic's `data.y` an object, an array, or
    /// missing, so its result is `null`.
    fn grid_contexts() -> Vec<Value> {
        let targets = [None, Some(json!([0])), Some(json!(7))];
        let results = [Some(json!({"k": 1})), Some(json!([1, 2])), None];
        let mut contexts = Vec::new();
        for target in &targets {
            for result in &results {
                let mut data = serde_json::Map::new();
                if let Some(x) = target {
                    data.insert("x".into(), x.clone());
                }
                if let Some(y) = result {
                    data.insert("y".into(), y.clone());
                }
                contexts.push(json!({"data": data, "metadata": {"m": 1}, "temp_data": {}}));
            }
        }
        contexts
    }

    /// Over the grid, a mapping compiles or not, and each one that does
    /// leaves, over each context, what `tests/golden/lib/builtins/map_grid.json` holds: per
    /// mapping, `"refused"` or one observation per context.
    #[test]
    fn map_on_the_mapping_grid() {
        let mut golden = Golden::new("map_grid", "map_on_the_mapping_grid");
        let contexts = grid_contexts();
        let (mut runs, mut refused) = (0, 0);
        for mapping in mapping_grid() {
            let mapping = sorted(&mapping);
            let input = json!({"mappings": [mapping]});
            let name = mapping.to_string();
            let Some(task) = task("map", &input) else {
                golden.case(name, OwnedDataValue::String("refused".to_owned()));
                refused += 1;
                continue;
            };
            let observed = contexts
                .iter()
                .map(|context| {
                    runs += 1;
                    task.run_both(&Value::Null, context).golden()
                })
                .collect();
            golden.case(name, OwnedDataValue::Array(observed));
        }
        // 151 of the 216 are refused, so 65 run, each over every context.
        assert_eq!(refused, 151);
        assert_eq!(runs, 65 * contexts.len());
    }

    #[test]
    fn a_root_write_merges_and_records_one_change_at_the_root() {
        let mut golden = Golden::new(
            "map",
            "a_root_write_merges_and_records_one_change_at_the_root",
        );
        let got = both(
            &mut golden,
            json!([
                {"path": "temp_data", "logic": {"field2": "second"}},
                {"path": "data", "logic": {"var": "temp_data"}}
            ]),
            &json!({"data": {"a": 1}, "temp_data": {"field1": "first"}}),
        );
        assert_eq!(
            got.context,
            r#"{"data":{"a":1,"field1":"first","field2":"second"},"metadata":{},"temp_data":{"field1":"first","field2":"second"}}"#
        );
        assert_eq!(
            got.changes,
            r#"[{"path":"temp_data","old_value":{"field1":"first"},"new_value":{"field2":"second"}},{"path":"data","old_value":{"a":1},"new_value":{"field1":"first","field2":"second"}}]"#
        );

        // Anything but an object replaces the slot.
        both(
            &mut golden,
            json!([{"path": "temp_data", "logic": [1]}]),
            &json!({"temp_data": {"x": 1}}),
        );
    }

    #[test]
    fn a_computed_destination_is_written() {
        let mut golden = Golden::new("map", "a_computed_destination_is_written");
        let got = both(
            &mut golden,
            json!([
                {"path": {"cat": ["data.accounts.", {"var": "data.id"}, ".balance"]}, "logic": 42},
                {"path": {"cat": ["data.slot_", {"var": "data.n"}]}, "logic": true},
                {"path": {"cat": ["temp_data.", "folded"]}, "logic": {"var": "data.id"}}
            ]),
            &json!({"data": {"id": "ACC7", "n": 3}}),
        );
        assert_eq!(
            got.context,
            r#"{"data":{"id":"ACC7","n":3,"accounts":{"ACC7":{"balance":42}},"slot_3":true},"metadata":{},"temp_data":{"folded":"ACC7"}}"#
        );
        // A change names where the value went, not the expression.
        assert!(
            got.changes
                .contains(r#""path":"data.accounts.ACC7.balance""#)
        );
        assert!(got.changes.contains(r#""path":"data.slot_3""#));
    }

    #[test]
    fn later_mappings_see_earlier_writes() {
        let mut golden = Golden::new("map", "later_mappings_see_earlier_writes");
        let got = both(
            &mut golden,
            json!([
                {"path": "data.a", "logic": 1},
                {"path": "data.b", "logic": {"+": [{"var": "data.a"}, 1]}},
                {"path": "data.name", "logic": "n"},
                {"path": {"cat": ["data.by_", {"var": "data.name"}]}, "logic": {"var": "data.b"}},
                {"path": "temp_data.x", "unset": true},
                {"path": "data.missing_after", "logic": {"missing": ["temp_data.x"]}},
                {"path": "data.list", "logic": {"var": "data.a"}, "mode": "append"},
                {"path": "data.first", "logic": {"var": "data.list.0"}},
                {"path": "data", "logic": {"merged": true}},
                {"path": "data.saw_merge", "logic": {"var": "data.merged"}}
            ]),
            &json!({"temp_data": {"x": "stale", "y": 1}}),
        );
        assert_eq!(
            got.context,
            r#"{"data":{"a":1,"b":2,"name":"n","by_n":2,"missing_after":["temp_data.x"],"list":[1],"first":1,"merged":true,"saw_merge":true},"metadata":{},"temp_data":{"y":1}}"#
        );
    }

    #[test]
    fn append_and_extend_record_one_change_per_element() {
        let mut golden = Golden::new("map", "append_and_extend_record_one_change_per_element");
        let got = both(
            &mut golden,
            json!([
                {"path": "data.xs", "logic": {"var": "data.more"}, "mode": "extend"},
                {"path": "data.xs", "logic": {"var": "data.missing"}, "mode": "extend"},
                {"path": "data.ys", "logic": {"var": "data.missing"}, "mode": "append",
                 "on_null": "unset"},
                {"path": "data.log", "logic": {"turn": 0}, "mode": "append"},
                {"path": "data.log", "logic": {"turn": {"var": "data.xs.0"}}, "mode": "append"},
                {"path": "data.fresh", "logic": [4, 5], "mode": "extend"},
                {"path": "data.none", "logic": [], "mode": "extend"}
            ]),
            &json!({"data": {"more": [2, 3], "xs": [1], "ys": [9]}}),
        );
        assert_eq!(
            got.context,
            r#"{"data":{"more":[2,3],"xs":[1,2,3],"log":[{"turn":0},{"turn":1}],"fresh":[4,5],"none":[]},"metadata":{},"temp_data":{}}"#
        );
        let paths: Vec<String> = serde_json::from_str::<Vec<Value>>(&got.changes)
            .unwrap()
            .iter()
            .map(|c| c["path"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            paths,
            [
                "data.xs.1",
                "data.xs.2",
                "data.ys",
                "data.log.0",
                "data.log.1",
                "data.fresh.0",
                "data.fresh.1"
            ]
        );
    }

    #[test]
    fn a_null_result_is_skipped_unless_on_null_unsets() {
        let mut golden = Golden::new("map", "a_null_result_is_skipped_unless_on_null_unsets");
        let mappings = json!([
            {"path": "temp_data.a", "logic": {"if": [{"var": "data.go"}, "new", null]}},
            {"path": "temp_data.b", "logic": {"var": "data.go_missing"}, "on_null": "skip"},
            {"path": "temp_data.c", "logic": null},
            {"path": "temp_data.reason", "logic": {"if": [{"var": "data.ok"}, null, "FAILED"]},
             "on_null": "unset"},
            {"path": "temp_data.gone", "logic": null, "on_null": "unset"},
            {"path": "temp_data.never.there", "unset": true}
        ]);
        let temp = json!({"a": 1, "b": 2, "c": 3, "reason": "old", "gone": 0});
        let got = both(
            &mut golden,
            mappings.clone(),
            &json!({"data": {"go": false, "ok": true}, "temp_data": temp}),
        );
        assert_eq!(
            got.context,
            r#"{"data":{"go":false,"ok":true},"metadata":{},"temp_data":{"a":1,"b":2,"c":3}}"#
        );
        assert_eq!(
            got.changes,
            r#"[{"path":"temp_data.reason","old_value":"old","new_value":null,"removed":true},{"path":"temp_data.gone","old_value":0,"new_value":null,"removed":true}]"#
        );
        both(
            &mut golden,
            mappings,
            &json!({"data": {"go": true, "ok": false}, "temp_data": temp}),
        );
    }

    #[test]
    fn a_computed_path_to_a_root_fails_the_mapping_and_keeps_the_root() {
        let mut golden = Golden::new(
            "map",
            "a_computed_path_to_a_root_fails_the_mapping_and_keeps_the_root",
        );
        let got = both(
            &mut golden,
            json!([
                {"path": {"cat": ["temp", "_data"]}, "unset": true},
                {"path": {"var": "data.root"}, "unset": true},
                {"path": {"cat": ["da", "ta"]}, "logic": 1, "mode": "append"},
                {"path": {"var": "data.root"}, "logic": [1], "mode": "extend"},
                {"path": {"var": "data.root"}, "logic": null, "on_null": "unset"},
                {"path": "data.after", "logic": "ran"}
            ]),
            &json!({"data": {"root": "temp_data"}, "temp_data": {"x": 1}}),
        );
        assert_eq!(
            got.context,
            r#"{"data":{"root":"temp_data","after":"ran"},"metadata":{},"temp_data":{"x":1}}"#
        );
        assert_eq!(got.status, Some(500));
    }

    #[test]
    fn a_failing_mapping_runs_the_rest_and_the_task_returns_500() {
        let mut golden = Golden::new(
            "map",
            "a_failing_mapping_runs_the_rest_and_the_task_returns_500",
        );
        let got = both(
            &mut golden,
            json!([
                {"path": "data.n", "logic": 1, "mode": "append"},
                {"path": "data.xs", "logic": 5, "mode": "extend"},
                {"path": "data.q", "logic": {"/": [1, {"var": "data.zero"}]}},
                {"path": "data.after", "logic": "ran"}
            ]),
            &json!({"data": {"n": 7, "zero": 0}}),
        );
        assert_eq!(
            got.context,
            r#"{"data":{"n":7,"zero":0,"after":"ran"},"metadata":{},"temp_data":{}}"#
        );
        assert_eq!(got.status, Some(500));
        assert_eq!(
            got.error_code, None,
            "a failed mapping is a status, not an `Err`"
        );
        assert_eq!(
            got.errors,
            r#"[{"code":"TASK_STATUS_ERROR","message":"Task t returned status 500","path":null,"workflow_id":"w","task_id":"t","timestamp":"<timestamp>"}]"#
        );
    }

    #[test]
    fn a_computed_path_that_is_not_a_context_path_fails_the_mapping() {
        // Break 15's runtime half: v3 wrote any top-level context key; a
        // computed path that is not a `Path` fails its mapping.
        let input = json!({"mappings": [
            {"path": {"var": "data.where"}, "logic": 1},
            {"path": {"var": "data.n"}, "logic": 1},
            {"path": "data.after", "logic": "ran"}
        ]});
        let got = run_once(
            "map",
            &input,
            &Value::Null,
            &json!({"data": {"n": 5, "where": "extra.x"}}),
            true,
        );
        assert_eq!(
            got.context,
            r#"{"data":{"n":5,"where":"extra.x","after":"ran"},"metadata":{},"temp_data":{}}"#
        );
        assert_eq!(got.status, Some(500));
    }

    /// `task` called once over `context`, with the run context's stats and
    /// arena bytes in use before it finishes, and the context after it.
    fn call(task: &Task, context: &Value, capture: bool) -> (u32, usize, usize, String) {
        let mut slot = seed(context);
        let mut changes = Vec::new();
        let mut errors = Vec::new();
        let (views, live, used) = {
            let mut rc = RunContext::new(&mut slot, Tuning::default());
            let outcome = task.call(
                &mut rc,
                &OwnedDataValue::Null,
                capture,
                &mut changes,
                &mut errors,
            );
            assert_eq!(outcome, (Some(200), None));
            let stats = rc.stats();
            let used = rc.read(|owner, _| arena::used(&owner.arena));
            rc.finish();
            (stats.views, stats.live_max, used)
        };
        (views, live, used, serde_json::to_string(&slot).unwrap())
    }

    #[test]
    fn k_writes_into_one_subtree_splice_only_their_spines() {
        let keys: serde_json::Map<String, Value> =
            (0..10).map(|i| (format!("k{i}"), json!(0))).collect();
        let big: Vec<Value> = (0..10_000).map(|i| json!(format!("row {i}"))).collect();
        let context = json!({"data": {"a": {"b": keys}, "big": big}});
        let grown = |k: usize| {
            let mappings: Vec<Value> = (0..k)
                .map(|i| {
                    let key = format!("data.a.b.k{}", i % 10);
                    json!({"path": key, "logic": {"+": [{"var": key}, 1]}})
                })
                .collect();
            let task = task("map", &json!({"mappings": mappings})).unwrap();
            let (views, live, used, after) = call(&task, &context, false);
            assert_eq!(views, 1, "{k} writes, one view");
            let each = k / 10;
            assert!(
                after.contains(&format!(r#""k9":{each}}}"#)),
                "{k} writes: {after:.80}"
            );
            (live, used - live)
        };
        let (live, ten) = grown(10);
        let (_, hundred) = grown(100);
        // Linear in k: each write costs its spine, whatever came before it.
        let (per_ten, per_hundred) = (ten / 10, hundred / 100);
        assert!(
            per_hundred <= 2 * per_ten && per_ten <= 2 * per_hundred,
            "{per_ten} bytes per write at k = 10, {per_hundred} at k = 100"
        );
        // A spine, not the view: a write costs well under a tenth of it.
        assert!(
            per_hundred * 10 < live,
            "{per_hundred} bytes per write, {live} live"
        );
    }

    #[test]
    fn capture_off_converts_at_the_checkpoint() {
        let input = json!({"mappings": [
            {"path": "data.a", "logic": {"var": "data.src"}},
            {"path": "temp_data", "logic": {"var": "data.src"}},
            {"path": "data.list", "logic": {"var": "data.src"}, "mode": "append"},
            {"path": "data.list", "logic": {"var": "data.more"}, "mode": "extend"}
        ]});
        let context = json!({"data": {"src": {"k": "v", "n": [1, 2]}, "more": [3]},
                             "temp_data": {"t": 1}});
        let task = task("map", &input).unwrap();
        let mut after = Vec::new();
        for capture in [true, false] {
            let mut slot = seed(&context);
            let (mut changes, mut errors) = (Vec::new(), Vec::new());
            let mut rc = RunContext::new(&mut slot, Tuning::default());
            let outcome = task.call(
                &mut rc,
                &OwnedDataValue::Null,
                capture,
                &mut changes,
                &mut errors,
            );
            assert_eq!(outcome, (Some(200), None));
            // Under capture the mark holds the owned value the change was
            // built from; without it, only the view knows the value.
            for path in ["data.a", "temp_data.k", "data.list"] {
                let read = rc.get(&path.parse().unwrap());
                if capture {
                    assert!(matches!(read, Some(Cow::Borrowed(_))), "{path}: {read:?}");
                } else {
                    assert!(matches!(read, Some(Cow::Owned(_))), "{path}: {read:?}");
                }
            }
            let stats = rc.finish();
            assert_eq!(stats.converted == 0, capture, "{stats:?}");
            assert_eq!(changes.is_empty(), !capture);
            after.push(serde_json::to_string(&slot).unwrap());
        }
        assert_eq!(after[0], after[1]);
    }
}
