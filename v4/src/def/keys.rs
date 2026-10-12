//! The known keys per level, and what an unknown key becomes.
//!
//! An unknown key moves into the owning node's `ext`: at a nested level under
//! `loop.<key>`, `for_each.<key>`, `rollout.<key>` or `function.<key>`. It is a
//! `LIKELY_TYPO` when it is a near miss of a known key at its level, otherwise
//! an `UNKNOWN_KEY`. With the compat reader on, each takes its default
//! severity, and the level's v3 spellings are near-miss candidates too; with
//! it off, both are Rejected, nothing moves, and no v3 spelling is a
//! candidate.

use super::parse::Walk;
use crate::base::typo::near_miss;
use crate::base::{AuthoredPath, IssueCode, Severity};
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
const TASK_V3_KEYS: &[&str] = &["continue_on_error", "halt_on"];

/// A group's v3 spelling, which the compat reader moves into its `ext`.
const GROUP_V3_KEYS: &[&str] = &["continue_on_error"];

/// The keys of a `publish_json` or `publish_xml` input. The compat reader
/// moves any other into the task's `ext`.
pub(super) const PUBLISH_KEYS: &[&str] = &["source", "target", "pretty", "root_element"];

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
            let near = near_miss(key, candidates);
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
