//! The known keys per level, and what an unknown key becomes.
//!
//! An unknown key moves into the owning node's `ext`: at a nested level under
//! `loop.<key>`, `for_each.<key>`, `rollout.<key>` or `function.<key>`. It is a
//! `LIKELY_TYPO` when it is a near miss of a known key at its level, otherwise
//! an `UNKNOWN_KEY`. With the compat reader on, each takes its default
//! severity, and the level's v3 spellings are near-miss candidates too; with
//! it off, both are Rejected, nothing moves, and no v3 spelling is a
//! candidate: a level's v3 key is then itself an unknown key. Nothing here
//! looks inside `function.input`, so with the reader off the v3 spellings
//! there stay for the function's config to refuse.

use super::parse::Walk;
use crate::base::typo::unknown_key;
use crate::base::{AuthoredPath, IssueCode, Severity};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

/// A workflow's keys.
const WORKFLOW_KEYS: &[&str] = &[
    "id",
    "name",
    "description",
    "priority",
    "condition",
    "tasks",
    "on_failure",
    "channel",
    "status",
    "version",
    "rollout",
    "loop",
    "tags",
    "created_at",
    "updated_at",
    "ext",
];

/// A task's keys.
const TASK_KEYS: &[&str] = &[
    "id",
    "name",
    "description",
    "condition",
    "function",
    "on_failure",
    "on_rejection",
    "terminal",
    "for_each",
    "ext",
];

/// A group's keys.
const GROUP_KEYS: &[&str] = &[
    "id",
    "name",
    "description",
    "condition",
    "terminal",
    "tasks",
    "ext",
];

/// `loop`'s keys.
const LOOP_KEYS: &[&str] = &[
    "counter",
    "init",
    "increment",
    "max",
    "setup",
    "over",
    "as",
    "scratch",
];

/// `for_each`'s keys.
const FOR_EACH_KEYS: &[&str] = &["over", "as", "max_concurrency", "collect", "into"];

/// `rollout`'s keys.
const ROLLOUT_KEYS: &[&str] = &["bucket_start", "bucket_end"];

/// `function`'s keys.
const FUNCTION_KEYS: &[&str] = &["name", "input"];

/// A workflow's v3 spellings, which the compat reader rewrites.
const WORKFLOW_V3_KEYS: &[&str] = &["continue_on_error"];

/// A task's v3 spellings, which the compat reader rewrites.
pub(super) const TASK_V3_KEYS: &[&str] = &["continue_on_error", "halt_on"];

/// A group's v3 spelling, which the compat reader moves into its `ext`.
const GROUP_V3_KEYS: &[&str] = &["continue_on_error"];

/// A workflow's v4 policy, which its v3 spelling becomes.
pub(super) const WORKFLOW_POLICIES: &[&str] = &["on_failure"];

/// A task's v4 policies, which its v3 spellings become.
pub(super) const TASK_POLICIES: &[&str] = &["on_failure", "on_rejection"];

/// The keys of a `map` input.
const MAP_KEYS: &[&str] = &["mappings"];

/// The keys of one of `map`'s mappings.
const MAPPING_KEYS: &[&str] = &["path", "logic", "unset", "on_null", "mode"];

/// The keys of a `validation` input.
const VALIDATION_KEYS: &[&str] = &["rules"];

/// The keys of one of `validation`'s rules.
const RULE_KEYS: &[&str] = &["logic", "message"];

/// The keys of a `filter` input.
const FILTER_KEYS: &[&str] = &["condition", "on_reject"];

/// The keys of a `log` input.
const LOG_KEYS: &[&str] = &["level", "message", "fields"];

/// The keys of a `parse_json` or `parse_xml` input.
const PARSE_KEYS: &[&str] = &["source", "target"];

/// The keys of a `publish_json` or `publish_xml` input.
const PUBLISH_KEYS: &[&str] = &["source", "target", "pretty", "root_element"];

/// What the compat reader knows of a built-in's input: its keys and,
/// for an input holding an array of nested configs, that array's key and the
/// keys of its elements. It moves any other key into the task's `ext`.
///
/// Each table equals its config's derived fields, which the built-in
/// configs' tests check.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BuiltinInput {
    /// The input's keys.
    pub(crate) keys: &'static [&'static str],
    /// The array of nested configs, and their keys.
    pub(crate) elements: Option<(&'static str, &'static [&'static str])>,
}

/// The input of the built-in called `name`; `None` for any other name.
pub(crate) fn builtin_input(name: &str) -> Option<BuiltinInput> {
    let (keys, elements) = match name {
        "map" => (MAP_KEYS, Some(("mappings", MAPPING_KEYS))),
        super::VALIDATION => (VALIDATION_KEYS, Some(("rules", RULE_KEYS))),
        "filter" => (FILTER_KEYS, None),
        "log" => (LOG_KEYS, None),
        "parse_json" | "parse_xml" => (PARSE_KEYS, None),
        "publish_json" | "publish_xml" => (PUBLISH_KEYS, None),
        _ => return None,
    };
    Some(BuiltinInput { keys, elements })
}

/// One level's table: its known keys, and the v3 spellings the compat reader
/// takes there.
pub(super) struct Level {
    known: &'static [&'static str],
    v3: &'static [&'static str],
}

impl Level {
    const fn new(known: &'static [&'static str]) -> Self {
        Self { known, v3: &[] }
    }

    /// Whether `key` is one of the level's known keys.
    pub(super) fn knows(&self, key: &str) -> bool {
        self.known.contains(&key)
    }

    /// The level's known keys, for the tests that hold the table to the
    /// walk, the serializer and the types.
    #[cfg(test)]
    pub(super) fn known(&self) -> &'static [&'static str] {
        self.known
    }
}

/// A workflow.
pub(super) const WORKFLOW: Level = Level {
    known: WORKFLOW_KEYS,
    v3: WORKFLOW_V3_KEYS,
};
/// A task.
pub(super) const TASK: Level = Level {
    known: TASK_KEYS,
    v3: TASK_V3_KEYS,
};
/// A group.
pub(super) const GROUP: Level = Level {
    known: GROUP_KEYS,
    v3: GROUP_V3_KEYS,
};
/// `loop`.
pub(super) const LOOP: Level = Level::new(LOOP_KEYS);
/// `for_each`.
pub(super) const FOR_EACH: Level = Level::new(FOR_EACH_KEYS);
/// `rollout`.
pub(super) const ROLLOUT: Level = Level::new(ROLLOUT_KEYS);
/// `function`.
pub(super) const FUNCTION: Level = Level::new(FUNCTION_KEYS);

/// The known keys at each level of a definition, as the reader, the walk and
/// the serializer read them. A tool that shows definitions — the debugger
/// reads it through the wasm package — holds its own view to these rather
/// than keeping a copy that drifts when a key is added.
#[derive(Clone, Copy, Debug, Serialize)]
#[non_exhaustive]
pub struct DefinitionKeys {
    /// A workflow's keys.
    pub workflow: &'static [&'static str],
    /// A task's keys.
    pub task: &'static [&'static str],
    /// A group's keys.
    pub group: &'static [&'static str],
    /// A workflow `loop`'s keys.
    pub r#loop: &'static [&'static str],
    /// A task `for_each`'s keys.
    pub for_each: &'static [&'static str],
    /// A workflow `rollout`'s keys.
    pub rollout: &'static [&'static str],
    /// A function reference's keys.
    pub function: &'static [&'static str],
}

/// The known keys at each level of a definition.
#[must_use]
pub fn definition_keys() -> DefinitionKeys {
    DefinitionKeys {
        workflow: WORKFLOW_KEYS,
        task: TASK_KEYS,
        group: GROUP_KEYS,
        r#loop: LOOP_KEYS,
        for_each: FOR_EACH_KEYS,
        rollout: ROLLOUT_KEYS,
        function: FUNCTION_KEYS,
    }
}

/// A node's `ext` as it is built: the authored object's entries, then the
/// keys moved into it (by the compat reader, then by the walk), each
/// remembered with the path it came from.
pub(super) struct Ext {
    map: Map<String, Value>,
    moved: HashMap<String, AuthoredPath>,
}

impl Ext {
    /// The authored `ext`'s entries. One that is not an object is reported
    /// with the node's other keys, and contributes nothing.
    pub(super) fn authored(raw: Option<&Value>) -> Self {
        Self {
            map: raw.and_then(Value::as_object).cloned().unwrap_or_default(),
            moved: HashMap::new(),
        }
    }

    /// Keeps `value`, moved from the key `key` at `from`, as `name`. The one
    /// collision rule for every move, the reader's and the walk's: `Err` with
    /// the message when `ext` already holds `name`, naming where that entry
    /// came from.
    pub(super) fn keep(
        &mut self,
        key: &str,
        name: String,
        value: Value,
        from: AuthoredPath,
    ) -> Result<(), String> {
        if self.map.contains_key(&name) {
            return Err(match self.moved.get(&name) {
                Some(first) => format!(
                    "`{key}` cannot be kept in `ext` as `{name}`: the key at `{first}` is \
                     already kept there"
                ),
                None => {
                    format!(
                        "`{key}` appears both on the node and in its authored `ext`, as `{name}`"
                    )
                }
            });
        }
        self.map.insert(name.clone(), value);
        self.moved.insert(name, from);
        Ok(())
    }

    pub(super) fn into_map(self) -> Map<String, Value> {
        self.map
    }
}

impl Walk<'_> {
    /// Reports each key of `obj` that is neither known at `level` nor in
    /// `skip`, in the object's key order, and moves it into `ext` as
    /// `<prefix><key>` when the compat reader is on.
    ///
    /// `skip` names keys refused by a rule of their own, which are not also
    /// unknown. Under compat, a near miss of one of the level's v3 spellings
    /// is a `LIKELY_TYPO` naming it; the reader has already taken the
    /// spelling itself.
    pub(super) fn unknown_keys(
        &mut self,
        obj: &Map<String, Value>,
        level: &Level,
        skip: &[&str],
        at: &AuthoredPath,
        prefix: &str,
        ext: &mut Ext,
    ) {
        // Built on the first unknown key: the known keys, then (under compat)
        // the v3 spellings, so a tie goes to the v4 key.
        let mut candidates: Option<Vec<&str>> = None;
        for (key, value) in obj {
            if level.knows(key) || skip.contains(&key.as_str()) {
                continue;
            }
            let path = at.join(key.as_str());
            let candidates = candidates.get_or_insert_with(|| {
                let v3 = if self.compat { level.v3 } else { &[] };
                level.known.iter().chain(v3).copied().collect()
            });
            let (code, message) = unknown_key(key, candidates);
            if !self.compat {
                self.report_as(code, Severity::Rejected, path, message);
                continue;
            }
            let name = format!("{prefix}{key}");
            let kept = format!("{message} (kept in `ext` as `{name}`)");
            match ext.keep(key, name, value.clone(), path.clone()) {
                Ok(()) => self.report(code, path, kept),
                Err(collision) => self.report(IssueCode::ExtCollision, path, collision),
            }
        }
    }
}
