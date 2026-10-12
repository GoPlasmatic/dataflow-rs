//! The known keys per level, and what an unknown key becomes.
//!
//! An unknown key moves into the owning node's `ext`: at a nested level under
//! `loop.<key>`, `for_each.<key>`, `rollout.<key>` or `function.<key>`. It is a
//! `LIKELY_TYPO` when it is a near miss of a known key at its level, otherwise
//! an `UNKNOWN_KEY`. With the compat reader on, each takes its default
//! severity; with it off, both are Rejected and nothing moves.

use super::parse::Walk;
use crate::base::typo::near_miss;
use crate::base::{AuthoredPath, IssueCode, Severity};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// A workflow's keys.
pub(super) const WORKFLOW_KEYS: &[&str] = &[
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
pub(super) const TASK_KEYS: &[&str] = &[
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
pub(super) const GROUP_KEYS: &[&str] = &[
    "id",
    "name",
    "description",
    "condition",
    "terminal",
    "tasks",
    "ext",
];

/// `loop`'s keys.
pub(super) const LOOP_KEYS: &[&str] = &[
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
pub(super) const FOR_EACH_KEYS: &[&str] = &["over", "as", "max_concurrency", "collect", "into"];

/// `rollout`'s keys.
pub(super) const ROLLOUT_KEYS: &[&str] = &["bucket_start", "bucket_end"];

/// `function`'s keys.
pub(super) const FUNCTION_KEYS: &[&str] = &["name", "input"];

/// A node's `ext` as it is built: the authored object's entries, then the
/// unknown keys moved into it, each remembered with the path it came from.
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

    pub(super) fn into_map(self) -> Map<String, Value> {
        self.map
    }
}

impl Walk<'_> {
    /// Reports each key of `obj` that is neither in `known` nor in `skip`,
    /// in the object's key order, and moves it into `ext` as
    /// `<prefix><key>` when the compat reader is on.
    ///
    /// `skip` names keys refused by a rule of their own, which are not also
    /// unknown.
    pub(super) fn unknown_keys(
        &mut self,
        obj: &Map<String, Value>,
        known: &[&str],
        skip: &[&str],
        at: &AuthoredPath,
        prefix: &str,
        ext: &mut Ext,
    ) {
        for (key, value) in obj {
            if known.contains(&key.as_str()) || skip.contains(&key.as_str()) {
                continue;
            }
            let path = at.join(key.as_str());
            let near = near_miss(key, known);
            let (code, message) = match near {
                Some(near) => (
                    IssueCode::LikelyTypo,
                    format!("unknown key `{key}`, did you mean `{near}`?"),
                ),
                None => (IssueCode::UnknownKey, format!("unknown key `{key}`")),
            };
            if !self.compat {
                self.report_as(code, Severity::Rejected, path, message);
                continue;
            }
            let name = format!("{prefix}{key}");
            if let Some(first) = ext.moved.get(&name) {
                let message = format!(
                    "unknown key `{key}` cannot be kept in `ext` as `{name}`: the unknown key at \
                     `{first}` is already kept there"
                );
                self.parse_failed(path, message);
                continue;
            }
            if ext.map.contains_key(&name) {
                let message = format!(
                    "`{key}` appears both on the node and in its authored `ext`, as `{name}`"
                );
                self.parse_failed(path, message);
                continue;
            }
            self.report(
                code,
                path.clone(),
                format!("{message} (kept in `ext` as `{name}`)"),
            );
            ext.map.insert(name.clone(), value.clone());
            ext.moved.insert(name, path);
        }
    }
}
