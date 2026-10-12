//! The compat reader: a pre-pass that rewrites v3 spellings into v4's, so
//! `parse`'s walk reads one spelling.
//!
//! It runs over a copy of the authored JSON, only when
//! [`ParseOptions::compat`](super::ParseOptions::compat) is on, and reports
//! as it goes. Its issues come before the walk's, in document order: each
//! object's keys in the order the object holds them (sorted, with
//! `serde_json`'s default map), the workflow first, then the steps of `loop`
//! and `tasks` as their keys come, each node before its members.
//!
//! | Where | v3 spelling | v4 spelling |
//! | --- | --- | --- |
//! | Workflow | `continue_on_error` | `on_failure: continue` or `fail` |
//! | Task | `continue_on_error`, `halt_on` | `on_failure` and `on_rejection`, by 06's policy table |
//! | `function.name` | `validate` | `validation` |
//! | `http_call` input | `path_logic`, `body_logic`, `output` | `path`, `body`, `response_path` |
//! | `enrich` input | `path_logic` | `path` |
//! | `publish_kafka` input | `key_logic`, `value_logic` | `key`, `value` |
//!
//! Each rewrite is a `V3_SPELLING` Advisory at the v3 key, and a
//! [`Rewrite`]. Two moves are not rewrites, and are reported as the keys they
//! are: a key of a `publish_json` or `publish_xml` input other than its four
//! moves into the task's `ext` as `function.input.<key>`, an `UNKNOWN_KEY` or
//! a `LIKELY_TYPO`; and a group's `continue_on_error` moves into the group's
//! `ext`, a `GROUP_CONTINUE_ON_ERROR` when it is `true` (v3 reported it and
//! never honoured it) and an `UNKNOWN_KEY` otherwise.
//!
//! The reader refuses a `continue_on_error` that is not a bool
//! (`PARSE_FAILED`), a `halt_on` that is not `"never"` or `"failure"`
//! (`INVALID_HALT_ON`, v3's message), and a v3 key written beside its v4
//! counterpart (`PARSE_FAILED` at the v3 key, as v3's duplicate field was):
//! `continue_on_error` with `on_failure`, `halt_on` with `on_rejection`, an
//! alias with its target. A refused key is taken out all the same, so the walk
//! does not report it twice.

use super::MAX_GROUP_DEPTH;
use super::keys::PUBLISH_KEYS;
use crate::base::typo::near_miss;
use crate::base::{AuthoredPath, Issue, IssueCode, StepKind};
use serde_json::{Map, Value};

/// One v3 spelling the compat reader rewrote.
///
/// `v3` and `v4` are short human-readable spellings, not JSON:
///
/// - a policy flag is `key: value`, and its v4 side lists the policies the
///   reader wrote, such as `continue_on_error: true` read as
///   `on_failure: continue, on_rejection: continue`. A task's two flags are
///   read together, so each of its rewrites names the same policies; a policy
///   the task already spelled in v4 is kept and not listed;
/// - a function name or an input key is the bare name: `validate` read as
///   `validation`, `path_logic` read as `path`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Rewrite {
    /// The v3 key's authored path, such as `tasks[0].continue_on_error`,
    /// `tasks[0].function.name` or `tasks[0].function.input.path_logic`.
    pub path: AuthoredPath,
    /// The v3 spelling.
    pub v3: String,
    /// What it was read as.
    pub v4: String,
}

/// The integration configs' v3 aliases: the function, the alias, and the key
/// it is read as.
const ALIASES: &[(&str, &str, &str)] = &[
    ("http_call", "path_logic", "path"),
    ("http_call", "body_logic", "body"),
    ("http_call", "output", "response_path"),
    ("enrich", "path_logic", "path"),
    ("publish_kafka", "key_logic", "key"),
    ("publish_kafka", "value_logic", "value"),
];

/// What the reader found: its issues, in document order, and its rewrites.
pub(super) struct Read {
    pub(super) issues: Vec<Issue>,
    pub(super) rewrites: Vec<Rewrite>,
}

/// Rewrites the v3 spellings in `workflow`, in place.
pub(super) fn read(workflow: &mut Map<String, Value>) -> Read {
    let mut reader = Read {
        issues: Vec::new(),
        rewrites: Vec::new(),
    };
    reader.workflow(workflow);
    reader
}

/// The step an issue sits in, which stamps it as the walk's issues are.
struct Step {
    kind: StepKind,
    id: Option<String>,
}

/// A v3 policy flag as read.
enum Flag {
    Absent,
    /// A spelling the reader rewrites: its `key: value` text and whether it
    /// is set (`true`, `"failure"`).
    Valid {
        text: String,
        set: bool,
    },
    /// Refused, with the issue that says why.
    Refused {
        code: IssueCode,
        message: String,
    },
}

impl Flag {
    fn is_set(&self) -> bool {
        matches!(self, Self::Valid { set: true, .. })
    }
}

/// Removes `key` from `obj`, keeping the order of the keys after it: with
/// `serde_json`'s `preserve_order`, `Map::remove` would swap the last key into
/// its place, and the walk reports a node's unknown keys in its order.
fn take(obj: &mut Map<String, Value>, key: &str) -> Option<Value> {
    if !obj.contains_key(key) {
        return None;
    }
    let mut taken = None;
    obj.retain(|k, value| {
        if k == key {
            taken = Some(std::mem::take(value));
        }
        k != key
    });
    taken
}

/// The keys of `obj` among `wanted`, in the object's order.
fn keys_among(obj: &Map<String, Value>, wanted: &[&'static str]) -> Vec<&'static str> {
    obj.keys()
        .filter_map(|key| wanted.iter().copied().find(|w| *w == key))
        .collect()
}

/// Both spellings of one key written at once.
fn both(v3: &str, v4: &str) -> String {
    format!("`{v3}` is the v3 spelling of `{v4}`, and both are written: keep `{v4}`")
}

impl Read {
    fn report(
        &mut self,
        code: IssueCode,
        path: AuthoredPath,
        message: impl Into<String>,
        step: Option<&Step>,
        field: Option<&str>,
    ) {
        let mut issue = Issue::new(code, path, message);
        if let Some(step) = step {
            issue = issue.with_step(step.kind, step.id.as_deref());
        }
        if let Some(field) = field {
            issue = issue.with_field(field);
        }
        self.issues.push(issue);
    }

    /// A rewrite: its `V3_SPELLING` and its record.
    fn rewrite(
        &mut self,
        path: AuthoredPath,
        v3: String,
        v4: String,
        step: Option<&Step>,
        field: Option<&str>,
    ) {
        let message = format!("`{v3}` is a v3 spelling, read as `{v4}`");
        self.report(IssueCode::V3Spelling, path.clone(), message, step, field);
        self.rewrites.push(Rewrite { path, v3, v4 });
    }

    fn workflow(&mut self, obj: &mut Map<String, Value>) {
        let root = AuthoredPath::root();
        for key in keys_among(obj, &["continue_on_error", "loop", "tasks"]) {
            match key {
                "continue_on_error" => self.workflow_flag(obj, &root),
                "loop" => {
                    if let Some(Value::Object(config)) = obj.get_mut("loop")
                        && let Some(Value::Array(setup)) = config.get_mut("setup")
                    {
                        self.steps(setup, &root.join("loop").join("setup"), true, 0);
                    }
                }
                "tasks" => {
                    if let Some(Value::Array(tasks)) = obj.get_mut("tasks") {
                        self.steps(tasks, &root.join("tasks"), false, 0);
                    }
                }
                _ => {}
            }
        }
    }

    /// A workflow's `continue_on_error`: its `on_failure`.
    fn workflow_flag(&mut self, obj: &mut Map<String, Value>, at: &AuthoredPath) {
        let Some(raw) = take(obj, "continue_on_error") else {
            return;
        };
        let path = at.join("continue_on_error");
        let Value::Bool(flag) = raw else {
            self.report(
                IssueCode::ParseFailed,
                path,
                "expected a boolean",
                None,
                None,
            );
            return;
        };
        if obj.contains_key("on_failure") {
            let message = both("continue_on_error", "on_failure");
            self.report(IssueCode::ParseFailed, path, message, None, None);
            return;
        }
        let policy = if flag { "continue" } else { "fail" };
        obj.insert("on_failure".into(), Value::String(policy.into()));
        let (v3, v4) = (
            format!("continue_on_error: {flag}"),
            format!("on_failure: {policy}"),
        );
        self.rewrite(path, v3, v4, None, None);
    }

    /// The steps of a list, as the walk visits them.
    fn steps(&mut self, items: &mut [Value], at: &AuthoredPath, setup: bool, depth: usize) {
        for (i, node) in items.iter_mut().enumerate() {
            if let Value::Object(obj) = node {
                self.step(obj, at.join(i), setup, depth);
            }
        }
    }

    fn step(&mut self, obj: &mut Map<String, Value>, at: AuthoredPath, setup: bool, depth: usize) {
        let group = obj.contains_key("tasks");
        let step = Step {
            kind: if setup {
                StepKind::Setup
            } else if group {
                StepKind::Group
            } else {
                StepKind::Task
            },
            id: obj
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string),
        };
        if group {
            self.group(obj, &at, &step, setup, depth);
        } else {
            self.task(obj, &at, &step);
        }
    }

    fn group(
        &mut self,
        obj: &mut Map<String, Value>,
        at: &AuthoredPath,
        step: &Step,
        setup: bool,
        depth: usize,
    ) {
        // A group too deep is not descended into, as the walk does not.
        for key in keys_among(obj, &["continue_on_error", "tasks"]) {
            if key == "continue_on_error" {
                self.group_flag(obj, at, step);
            } else if depth < MAX_GROUP_DEPTH
                && let Some(Value::Array(members)) = obj.get_mut("tasks")
            {
                self.steps(members, &at.join("tasks"), setup, depth + 1);
            }
        }
    }

    /// A group's `continue_on_error`: moved into its `ext`, never honoured.
    fn group_flag(&mut self, obj: &mut Map<String, Value>, at: &AuthoredPath, step: &Step) {
        const KEY: &str = "continue_on_error";
        let Some(value) = take(obj, KEY) else {
            return;
        };
        let (code, message) = if value == Value::Bool(true) {
            // v3's message, from `check_group_continue_on_error`.
            let message = format!(
                "group '{}' carries continue_on_error, which the engine does not honour — error \
                 handling is per task and per workflow, and a group only gates a span. Put it \
                 on the tasks inside the group, or on the workflow",
                step.id.as_deref().unwrap_or_default()
            );
            (IssueCode::GroupContinueOnError, message)
        } else {
            let message = format!(
                "unknown key `{KEY}`: a group has no failure policy (kept in `ext` as `{KEY}`)"
            );
            (IssueCode::UnknownKey, message)
        };
        let moved = Moved {
            key: KEY,
            name: KEY.to_string(),
            value,
            path: at.join(KEY),
            field: None,
        };
        self.keep_in_ext(obj, moved, code, message, step);
    }

    fn task(&mut self, obj: &mut Map<String, Value>, at: &AuthoredPath, step: &Step) {
        let order = keys_among(obj, &["continue_on_error", "halt_on", "function"]);
        let has_flags = obj.contains_key("continue_on_error") || obj.contains_key("halt_on");
        let continue_on_error = match take(obj, "continue_on_error") {
            None => Flag::Absent,
            Some(Value::Bool(_)) if obj.contains_key("on_failure") => Flag::Refused {
                code: IssueCode::ParseFailed,
                message: both("continue_on_error", "on_failure"),
            },
            Some(Value::Bool(set)) => Flag::Valid {
                text: format!("continue_on_error: {set}"),
                set,
            },
            Some(_) => Flag::Refused {
                code: IssueCode::ParseFailed,
                message: "expected a boolean".into(),
            },
        };
        let halt_on = match take(obj, "halt_on") {
            None => Flag::Absent,
            Some(raw) => match raw.as_str() {
                Some(spelling @ ("never" | "failure")) if !obj.contains_key("on_rejection") => {
                    Flag::Valid {
                        text: format!("halt_on: {spelling}"),
                        set: spelling == "failure",
                    }
                }
                Some("never" | "failure") => Flag::Refused {
                    code: IssueCode::ParseFailed,
                    message: both("halt_on", "on_rejection"),
                },
                // v3's message, from `check_shape`.
                _ => Flag::Refused {
                    code: IssueCode::InvalidHaltOn,
                    message: "halt_on must be one of \"never\", \"failure\"".into(),
                },
            },
        };

        // 06's policy table: the two flags make one policy.
        let v4 = if has_flags {
            let (on_failure, on_rejection) = match (continue_on_error.is_set(), halt_on.is_set()) {
                (false, false) => ("fail", "continue"),
                (true, false) => ("continue", "continue"),
                (false, true) => ("fail", "halt"),
                (true, true) => ("halt", "halt"),
            };
            let mut written = Vec::new();
            for (key, policy) in [("on_failure", on_failure), ("on_rejection", on_rejection)] {
                if !obj.contains_key(key) {
                    obj.insert(key.into(), Value::String(policy.into()));
                    written.push(format!("{key}: {policy}"));
                }
            }
            written.join(", ")
        } else {
            String::new()
        };

        for key in order {
            match key {
                "function" => self.function(obj, at, step),
                _ => {
                    let flag = if key == "halt_on" {
                        &halt_on
                    } else {
                        &continue_on_error
                    };
                    let path = at.join(key);
                    match flag {
                        Flag::Absent => {}
                        Flag::Valid { text, .. } => {
                            self.rewrite(path, text.clone(), v4.clone(), Some(step), None);
                        }
                        Flag::Refused { code, message } => {
                            self.report(*code, path, message.clone(), Some(step), None);
                        }
                    }
                }
            }
        }
    }

    /// A task's function: `validate`, then its input by function name.
    fn function(&mut self, obj: &mut Map<String, Value>, at: &AuthoredPath, step: &Step) {
        let Some(Value::Object(slot)) = obj.get_mut("function") else {
            return;
        };
        // Taken out of the task for the moment, so a stray input key can be
        // put in the task's `ext`; put back in its place below.
        let mut fields = std::mem::take(slot);
        let at = at.join("function");
        let name = fields
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        for key in keys_among(&fields, &["input", "name"]) {
            if key == "name" {
                if name == "validate" {
                    fields.insert("name".into(), Value::String("validation".into()));
                    let (v3, v4) = ("validate".into(), "validation".into());
                    self.rewrite(at.join("name"), v3, v4, Some(step), None);
                }
            } else if let Some(Value::Object(input)) = fields.get_mut("input") {
                let at = at.join("input");
                if name == "publish_json" || name == "publish_xml" {
                    for (key, value) in strays(input) {
                        self.stray(obj, &at, key, value, step);
                    }
                } else {
                    self.aliases(input, &name, &at, step);
                }
            }
        }
        if let Some(Value::Object(slot)) = obj.get_mut("function") {
            *slot = fields;
        }
    }

    /// The input's v3 aliases for `function`, each renamed in its place.
    fn aliases(
        &mut self,
        input: &mut Map<String, Value>,
        function: &str,
        at: &AuthoredPath,
        step: &Step,
    ) {
        let alias_of = |key: &str| {
            ALIASES
                .iter()
                .find(|(f, alias, _)| *f == function && *alias == key)
                .map(|(_, alias, target)| (*alias, *target))
        };
        if !input.keys().any(|key| alias_of(key).is_some()) {
            return;
        }
        let authored = std::mem::take(input);
        let conflicts: Vec<&str> = authored
            .keys()
            .filter_map(|key| alias_of(key))
            .filter(|(_, target)| authored.contains_key(*target))
            .map(|(alias, _)| alias)
            .collect();
        for (key, value) in authored {
            let Some((alias, target)) = alias_of(&key) else {
                input.insert(key, value);
                continue;
            };
            let path = at.join(alias);
            if conflicts.contains(&alias) {
                let message = both(alias, target);
                self.report(
                    IssueCode::ParseFailed,
                    path,
                    message,
                    Some(step),
                    Some(alias),
                );
            } else {
                input.insert(target.into(), value);
                let (v3, v4) = (alias.to_string(), target.to_string());
                self.rewrite(path, v3, v4, Some(step), Some(alias));
            }
        }
    }

    /// A publish input's key other than its four, moved into the task's
    /// `ext` as `function.input.<key>`.
    fn stray(
        &mut self,
        obj: &mut Map<String, Value>,
        at: &AuthoredPath,
        key: String,
        value: Value,
        step: &Step,
    ) {
        let name = format!("function.input.{key}");
        let (code, message) = match near_miss(&key, PUBLISH_KEYS) {
            Some(near) => (
                IssueCode::LikelyTypo,
                format!("unknown key `{key}`, did you mean `{near}`? (kept in `ext` as `{name}`)"),
            ),
            None => (
                IssueCode::UnknownKey,
                format!("unknown key `{key}` (kept in `ext` as `{name}`)"),
            ),
        };
        let moved = Moved {
            key: &key,
            path: at.join(key.as_str()),
            field: Some(&key),
            name,
            value,
        };
        self.keep_in_ext(obj, moved, code, message, step);
    }

    /// Puts a moved key into the node's `ext`, creating it, and reports it.
    /// A name the authored `ext` already holds is refused. An `ext` that is
    /// not an object keeps nothing; the walk refuses it.
    fn keep_in_ext(
        &mut self,
        obj: &mut Map<String, Value>,
        moved: Moved<'_>,
        code: IssueCode,
        message: String,
        step: &Step,
    ) {
        let Moved {
            key,
            name,
            value,
            path,
            field,
        } = moved;
        let ext = obj
            .entry("ext")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(ext) = ext {
            if ext.contains_key(&name) {
                let message = format!(
                    "`{key}` appears both on the node and in its authored `ext`, as `{name}`"
                );
                self.report(IssueCode::ParseFailed, path, message, Some(step), field);
                return;
            }
            ext.insert(name, value);
        }
        self.report(code, path, message, Some(step), field);
    }
}

/// A key moving into a node's `ext`.
struct Moved<'k> {
    /// The key as authored.
    key: &'k str,
    /// Its name in `ext`.
    name: String,
    value: Value,
    path: AuthoredPath,
    /// The config field, for a key under `function.input`.
    field: Option<&'k str>,
}

/// Takes the keys of a publish input other than its four out of it, in its
/// order.
fn strays(input: &mut Map<String, Value>) -> Vec<(String, Value)> {
    if input.keys().all(|key| PUBLISH_KEYS.contains(&key.as_str())) {
        return Vec::new();
    }
    let mut strays = Vec::new();
    for (key, value) in std::mem::take(input) {
        if PUBLISH_KEYS.contains(&key.as_str()) {
            input.insert(key, value);
        } else {
            strays.push((key, value));
        }
    }
    strays
}
