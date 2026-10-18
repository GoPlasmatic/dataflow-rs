//! `loop`: its rules in v3's `check_loop` order, and the slot predicates it
//! shares with `for_each`, copied from v3.

use super::keys::{Ext, LOOP};
use super::parse::Walk;
use super::types::{Logic, LoopDef, defaults};
use crate::base::{AuthoredPath, IssueCode};
use serde_json::{Map, Value};

/// Whether `path` is a well-formed `temp_data` field path: non-empty, with no
/// empty segment. The one rule `counter`, `as` and `scratch` share.
pub(super) fn is_slot_path(path: &str) -> bool {
    !path.is_empty() && !path.split('.').any(str::is_empty)
}

/// Whether two slot paths name the same key or one lies inside the other:
/// `it` and `it.item` overlap, `it` and `item` do not. A reset of the outer
/// slot would wipe the inner one.
pub(super) fn slots_overlap(a: &str, b: &str) -> bool {
    fn within(outer: &str, inner: &str) -> bool {
        inner
            .strip_prefix(outer)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    }
    within(a, b) || within(b, a)
}

/// Why a literal `over` could never yield an array, if it is one. An object
/// is an expression and an array is itself; any other literal is a constant
/// that is not an array.
pub(super) fn over_literal_problem(over: &Value) -> Option<String> {
    let kind = match over {
        Value::Object(_) | Value::Array(_) => return None,
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
    };
    Some(format!(
        "loop over must be a JSONLogic expression or an array literal — {kind} can never \
         evaluate to an array"
    ))
}

/// The loop's bound rules, each broken one as `(code, path, message)`,
/// in v3's order: an `increment` below 1, then a `max` at or below `init`.
/// `parse` checks what it read, an unreadable `increment` or `max` as `None`.
/// A definition cannot be edited after it, so the interpreter's termination
/// rests here alone.
fn bound_issues(
    init: i64,
    increment: Option<i64>,
    max: Option<i64>,
) -> impl Iterator<Item = (IssueCode, AuthoredPath, String)> {
    let at = AuthoredPath::root().join("loop");
    let increment = increment.filter(|&i| i < 1).map(|increment| {
        (
            IssueCode::LoopIncrementTooSmall,
            at.join("increment"),
            format!(
                "loop increment must be >= 1, got {increment} (a non-advancing counter would \
                 never reach max)"
            ),
        )
    });
    let bound = max.filter(|&max| max <= init).map(|max| {
        (
            IssueCode::LoopBoundEmpty,
            at.join("max"),
            format!(
                "loop max ({max}) must be greater than init ({init}) — the bound is half-open, \
                 so this could never run a sweep"
            ),
        )
    });
    increment.into_iter().chain(bound)
}

impl<'j> Walk<'j> {
    /// Rule 5, then rule 6: the loop's own rules and unknown keys, then the
    /// steps of its `setup`.
    pub(super) fn r#loop(&mut self, config: &'j Map<String, Value>, ext: &mut Ext) -> LoopDef {
        let at = AuthoredPath::root().join("loop");

        let max = match config.get("max") {
            None => {
                self.parse_failed(at.join("max"), "missing field `max`");
                None
            }
            Some(raw) => self.integer(raw, &at, "max"),
        };
        let init = self.optional_integer(config, &at, "init");
        let increment = self.optional_integer(config, &at, "increment");

        // v3 bounds an unreadable `init` at its default.
        let init_or_default = init.flatten().unwrap_or(defaults::INIT);
        for (code, path, message) in bound_issues(init_or_default, increment.flatten(), max) {
            self.report(code, path, message);
        }

        let counter = self.slot(config, &at, "counter", IssueCode::LoopCounterInvalid);
        let item = self.slot(config, &at, "as", IssueCode::LoopSlotInvalid);
        let scratch = self.slot(config, &at, "scratch", IssueCode::LoopSlotInvalid);

        // `get` rather than a typed read: an explicit `"over": null` is
        // present, and refused.
        let over = config.get("over");
        if item.is_some() && over.is_none() {
            self.report(
                IssueCode::LoopItemWithoutOver,
                at.join("as"),
                "loop `as` names a slot for the current element, but there is no `over` to take \
                 elements from",
            );
        }
        if let Some(over) = over {
            if let Some(problem) = over_literal_problem(over) {
                self.report(IssueCode::LoopOverInvalid, at.join("over"), problem);
            }
            if init_or_default < 0 {
                self.report(
                    IssueCode::LoopOverInvalid,
                    at.join("init"),
                    format!(
                        "loop init must be >= 0 when over is set — the counter indexes the \
                         array, got {init_or_default}"
                    ),
                );
            }
        }

        let slots = [
            ("counter", counter.as_deref()),
            ("as", item.as_deref()),
            ("scratch", scratch.as_deref()),
        ];
        for (i, (name_a, a)) in slots.iter().enumerate() {
            for (name_b, b) in &slots[i + 1..] {
                if let (Some(a), Some(b)) = (a, b)
                    && slots_overlap(a, b)
                {
                    self.report(
                        IssueCode::LoopSlotCollision,
                        at.join(*name_b),
                        format!(
                            "loop {name_a} ({a:?}) and {name_b} ({b:?}) overlap — each \
                             engine-owned slot must be its own temp_data path"
                        ),
                    );
                }
            }
        }

        self.unknown_keys(config, &LOOP, &[], &at, "loop.", ext);

        // Rule 6.
        let setup_at = at.join("setup");
        let setup = match config.get("setup") {
            None => Vec::new(),
            Some(list @ Value::Array(_)) => self.steps(Some(list), &setup_at, true, 0),
            Some(_) => {
                self.parse_failed(setup_at, "expected an array of steps");
                Vec::new()
            }
        };

        LoopDef {
            counter,
            init: init_or_default,
            increment: increment.flatten().unwrap_or(defaults::INCREMENT),
            max: max.unwrap_or(0),
            setup,
            over: over.cloned().map(Logic),
            r#as: item,
            scratch,
        }
    }

    /// An integer, or `PARSE_FAILED` at `loop.<key>`.
    fn integer(&mut self, raw: &Value, at: &AuthoredPath, key: &str) -> Option<i64> {
        let value = raw.as_i64();
        if value.is_none() {
            self.parse_failed(at.join(key), "expected an integer");
        }
        value
    }

    /// `None` when absent; `Some(None)` when present and not an integer.
    fn optional_integer(
        &mut self,
        config: &Map<String, Value>,
        at: &AuthoredPath,
        key: &str,
    ) -> Option<Option<i64>> {
        config.get(key).map(|raw| self.integer(raw, at, key))
    }

    /// A slot path: absent or `null` is `None`, a non-string is
    /// `PARSE_FAILED`, and a string that is not a slot path is `invalid`.
    /// A malformed string is still returned, so the collision rule sees it,
    /// as v3's does.
    fn slot(
        &mut self,
        config: &Map<String, Value>,
        at: &AuthoredPath,
        key: &str,
        invalid: IssueCode,
    ) -> Option<String> {
        match config.get(key) {
            None | Some(Value::Null) => None,
            Some(Value::String(path)) => {
                if !is_slot_path(path) {
                    self.report(
                        invalid,
                        at.join(key),
                        format!(
                            "loop {key} must be a non-empty temp_data field path, got {path:?}"
                        ),
                    );
                }
                Some(path.clone())
            }
            Some(_) => {
                self.parse_failed(at.join(key), "expected a string");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn slot_paths_have_no_empty_segment() {
        for good in ["i", "cursor.index", "a.b.c"] {
            assert!(is_slot_path(good), "{good}");
        }
        for bad in ["", ".", "a.", ".a", "a..b"] {
            assert!(!is_slot_path(bad), "{bad}");
        }
    }

    #[test]
    fn slots_overlap_is_segment_aware() {
        assert!(slots_overlap("it", "it"));
        assert!(slots_overlap("it", "it.item"));
        assert!(slots_overlap("it.item", "it"));
        assert!(!slots_overlap("it", "item"));
        assert!(!slots_overlap("p", "p_index"));
    }

    #[test]
    fn only_an_object_or_an_array_can_yield_an_array() {
        assert_eq!(over_literal_problem(&json!({"var": "data.x"})), None);
        assert_eq!(over_literal_problem(&json!([1])), None);
        for (literal, kind) in [
            (json!(null), "null"),
            (json!(true), "a boolean"),
            (json!(3), "a number"),
            (json!("data.x"), "a string"),
        ] {
            let problem = over_literal_problem(&literal).unwrap();
            assert!(
                problem.contains(&format!("— {kind} can never")),
                "{problem}"
            );
        }
    }
}
