//! The compat reader: a pre-pass that rewrites v3 spellings into v4's, so
//! `parse`'s walk reads one spelling.
//!
//! It runs over a copy of the authored JSON, only when
//! [`ParseOptions::compat`](super::ParseOptions::compat) is on, and reports
//! as it goes. Its issues come before the walk's, in document order: each
//! object's keys in the order they were written (the crate turns on
//! serde_json's `preserve_order`), the workflow first, then the steps of `loop`
//! and `tasks` as their keys come, each node before its members.
//!
//! | Where | v3 spelling | v4 spelling |
//! | --- | --- | --- |
//! | Workflow | `continue_on_error` | `on_failure: continue` or `fail` |
//! | Task | `continue_on_error`, `halt_on` | `on_failure` and `on_rejection`, by the policy table below |
//! | `function.name` | `validate` | `validation` |
//!
//! Each rewrite is a `V3_SPELLING` Advisory at the v3 key, and a
//! [`Rewrite`]. The integration configs (`http_call`, `enrich`,
//! `publish_kafka`) are their hosts': their inputs are left as written. Two
//! moves are not rewrites, and are reported as the keys they are:
//!
//! - **A key a built-in's input does not know** moves into the task's
//!   `ext` as `function.input.<path>`, an `UNKNOWN_KEY` or a `LIKELY_TYPO`.
//!   So does an unknown key of a `map` mapping or a `validation` rule that is
//!   an object, as `function.input.mappings[0].<key>`. v3's configs ignored
//!   them. The known keys are `keys.rs`'s tables, which equal the configs'
//!   fields. An input or an element that is not an object is left for the
//!   config to refuse.
//! - **A group's `continue_on_error`** moves into the group's `ext`, a
//!   `GROUP_CONTINUE_ON_ERROR` when it is `true` (v3 reported it and never
//!   honoured it) and an `UNKNOWN_KEY` otherwise.
//!
//! The reader refuses a `continue_on_error` that is not a bool
//! (`PARSE_FAILED`), a `halt_on` that is not `"never"` or `"failure"`
//! (`INVALID_HALT_ON`, v3's message), and a v3 key written beside a v4 one
//! (`MIXED_SPELLING` at the v3 key): on one node, any v3 policy key
//! (`continue_on_error`, a task's `halt_on`) beside any v4 policy key
//! (`on_failure`, a task's `on_rejection`). The two flags make one policy,
//! so a v3 flag beside the v4 key of the other axis would be a guess at how
//! the author meant them to combine. A refused key is taken out all the
//! same, so the walk does not report it twice.

use super::keys::{
    BuiltinInput, Ext, TASK_POLICIES, TASK_V3_KEYS, WORKFLOW_POLICIES, builtin_input,
};
use super::parse::{descends, is_group, step_kind};
use super::{V3_VALIDATE, VALIDATION};
use crate::base::typo::unknown_key;
use crate::base::{AuthoredPath, Issue, IssueCode, StepKind};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// One v3 spelling the compat reader rewrote.
///
/// `v3` and `v4` are short human-readable spellings, not JSON:
///
/// - a policy flag is `key: value`, and its v4 side lists the policies the
///   reader wrote, such as `continue_on_error: true` read as
///   `on_failure: continue, on_rejection: continue`. A task's two flags are
///   read together, so each of its rewrites names the same policies (a flag
///   beside a v4 policy is refused, not rewritten);
/// - a function name is the bare name: `validate` read as `validation`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Rewrite {
    /// The v3 key's authored path, such as `tasks[0].continue_on_error`
    /// or `tasks[0].function.name`.
    pub path: AuthoredPath,
    /// What it was read as, in a form a host can apply.
    pub kind: RewriteKind,
}

/// What a [`Rewrite`] changed: the v4 keys and values the reader wrote, so
/// a host that migrates a stored definition applies them instead of parsing
/// [`Display`](std::fmt::Display)'s prose.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RewriteKind {
    /// A v3 policy flag, read as the v4 policy keys written in its place.
    Policy {
        /// The v3 spelling as authored, such as `continue_on_error: true`;
        /// display only.
        v3: String,
        /// The v4 keys written and their values, in the definition's order.
        v4: Vec<(String, String)>,
    },
    /// A v3 function name, read as v4's: `validate` as `validation`.
    Function {
        /// The v3 name.
        v3: String,
        /// The v4 name.
        v4: String,
    },
}

impl std::fmt::Display for RewriteKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Policy { v3, v4 } => {
                let v4 = v4
                    .iter()
                    .map(|(key, value)| format!("{key}: {value}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "`{v3}` is a v3 spelling, read as `{v4}`")
            }
            Self::Function { v3, v4 } => {
                write!(f, "`{v3}` is a v3 spelling, read as `{v4}`")
            }
        }
    }
}

impl std::fmt::Display for Rewrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.kind.fmt(f)
    }
}

/// What the reader found: its issues, in document order, its rewrites, and
/// the `ext` of each node it moved a key into, by the node's path, for the
/// walk to go on building.
pub(super) struct Read {
    pub(super) issues: Vec<Issue>,
    pub(super) rewrites: Vec<Rewrite>,
    pub(super) exts: HashMap<AuthoredPath, Ext>,
}

/// Rewrites the v3 spellings in `workflow`, in place.
pub(super) fn read(workflow: &mut Map<String, Value>) -> Read {
    let mut reader = Read::new();
    reader.workflow(workflow);
    reader
}

/// Rewrites the v3 spellings in the steps of a list at `at`, in place, as
/// [`read`] rewrites a workflow's: under `loop.setup` when `setup` is set,
/// inside `depth` groups.
pub(super) fn read_steps(
    items: &mut [Value],
    at: &AuthoredPath,
    setup: bool,
    depth: usize,
) -> Read {
    let mut reader = Read::new();
    reader.steps(items, at, setup, depth);
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

/// Removes `key` from `obj`, keeping the order of the keys after it:
/// `Map::remove` would swap the last key into its place, and the walk reports
/// a node's unknown keys in its order.
fn take(obj: &mut Map<String, Value>, key: &str) -> Option<Value> {
    obj.shift_remove(key)
}

/// The keys of `obj` among `wanted`, in the object's order.
fn keys_among(obj: &Map<String, Value>, wanted: &[&'static str]) -> Vec<&'static str> {
    obj.keys()
        .filter_map(|key| wanted.iter().copied().find(|w| *w == key))
        .collect()
}

/// The v4 policy keys of `policies` that `obj` holds, in the table's order.
fn policies_in(obj: &Map<String, Value>, policies: &[&'static str]) -> Vec<&'static str> {
    policies
        .iter()
        .copied()
        .filter(|key| obj.contains_key(*key))
        .collect()
}

/// A v3 policy key written beside v4 policy keys on one `node`, whose v4
/// spelling is its `policies`.
fn mixed(v3: &str, v4: &[&str], node: &str, policies: &str) -> String {
    let v4: Vec<String> = v4.iter().map(|key| format!("`{key}`")).collect();
    format!(
        "v3's `{v3}` and v4's {} on one {node}; write only the v4 {policies}",
        v4.join(" and ")
    )
}

impl Read {
    fn new() -> Self {
        Self {
            issues: Vec::new(),
            rewrites: Vec::new(),
            exts: HashMap::new(),
        }
    }

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
        kind: RewriteKind,
        step: Option<&Step>,
        field: Option<&str>,
    ) {
        let message = kind.to_string();
        self.report(IssueCode::V3Spelling, path.clone(), message, step, field);
        self.rewrites.push(Rewrite { path, kind });
    }

    fn workflow(&mut self, obj: &mut Map<String, Value>) {
        const WANTED: &[&str] = &["continue_on_error", "loop", "tasks"];
        let root = AuthoredPath::root();
        for key in keys_among(obj, WANTED) {
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
        let v4 = policies_in(obj, WORKFLOW_POLICIES);
        if !v4.is_empty() {
            let message = mixed("continue_on_error", &v4, "workflow", "policy");
            self.report(IssueCode::MixedSpelling, path, message, None, None);
            return;
        }
        let policy = if flag { "continue" } else { "fail" };
        obj.insert("on_failure".into(), Value::String(policy.into()));
        let (v3, v4) = (
            format!("continue_on_error: {flag}"),
            vec![("on_failure".to_string(), policy.to_string())],
        );
        self.rewrite(path, RewriteKind::Policy { v3, v4 }, None, None);
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
        let group = is_group(obj);
        let step = Step {
            kind: step_kind(obj, setup),
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
        const WANTED: &[&str] = &["continue_on_error", "tasks"];
        for key in keys_among(obj, WANTED) {
            if key == "continue_on_error" {
                self.group_flag(obj, at, step);
            } else if descends(depth)
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
        self.keep_in_ext(obj, at, moved, code, message, step);
    }

    fn task(&mut self, obj: &mut Map<String, Value>, at: &AuthoredPath, step: &Step) {
        if !TASK_V3_KEYS.iter().any(|key| obj.contains_key(*key)) {
            self.function(obj, at, step);
            return;
        }
        const WANTED: &[&str] = &["continue_on_error", "halt_on", "function"];
        let order = keys_among(obj, WANTED);
        // Any v4 policy beside any v3 flag refuses every flag.
        let v4_policies = policies_in(obj, TASK_POLICIES);
        let refuse_mixed = |v3: &str| Flag::Refused {
            code: IssueCode::MixedSpelling,
            message: mixed(v3, &v4_policies, "task", "policies"),
        };
        let continue_on_error = match take(obj, "continue_on_error") {
            None => Flag::Absent,
            Some(Value::Bool(_)) if !v4_policies.is_empty() => refuse_mixed("continue_on_error"),
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
                Some("never" | "failure") if !v4_policies.is_empty() => refuse_mixed("halt_on"),
                Some(spelling @ ("never" | "failure")) => Flag::Valid {
                    text: format!("halt_on: {spelling}"),
                    set: spelling == "failure",
                },
                // v3's message, from `check_shape`.
                _ => Flag::Refused {
                    code: IssueCode::InvalidHaltOn,
                    message: "halt_on must be one of \"never\", \"failure\"".into(),
                },
            },
        };

        // 06's policy table: the two flags make one policy. A flag beside a
        // v4 policy was refused above, so here neither policy is written.
        let v4: Vec<(String, String)> = if v4_policies.is_empty() {
            let (on_failure, on_rejection) = match (continue_on_error.is_set(), halt_on.is_set()) {
                (false, false) => ("fail", "continue"),
                (true, false) => ("continue", "continue"),
                (false, true) => ("fail", "halt"),
                (true, true) => ("halt", "halt"),
            };
            obj.insert("on_failure".into(), Value::String(on_failure.into()));
            obj.insert("on_rejection".into(), Value::String(on_rejection.into()));
            vec![
                ("on_failure".to_string(), on_failure.to_string()),
                ("on_rejection".to_string(), on_rejection.to_string()),
            ]
        } else {
            Vec::new()
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
                            self.rewrite(
                                path,
                                RewriteKind::Policy {
                                    v3: text.clone(),
                                    v4: v4.clone(),
                                },
                                Some(step),
                                None,
                            );
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
        // Taken out of the task for the moment, so the task's authored `ext`
        // can be read as its input changes; put back in its place below.
        let mut fields = std::mem::take(slot);
        let task = at;
        let at = at.join("function");
        let (is_validate, builtin) = match fields.get("name").and_then(Value::as_str) {
            Some(V3_VALIDATE) => (true, builtin_input(VALIDATION)),
            Some(name) => (false, builtin_input(name)),
            None => (false, None),
        };
        for key in keys_among(&fields, &["input", "name"]) {
            if key == "name" {
                if is_validate {
                    fields.insert("name".into(), Value::String(VALIDATION.into()));
                    self.rewrite(
                        at.join("name"),
                        RewriteKind::Function {
                            v3: V3_VALIDATE.into(),
                            v4: VALIDATION.into(),
                        },
                        Some(step),
                        None,
                    );
                }
            } else if let Some(Value::Object(input)) = fields.get_mut("input")
                && let Some(known) = builtin
            {
                self.builtin_input(obj, task, input, known, step);
            }
        }
        if let Some(Value::Object(slot)) = obj.get_mut("function") {
            *slot = fields;
        }
    }

    /// A built-in's input, in its keys' order. A key the built-in does
    /// not know moves into the `ext` of the task at `task`, and so does an
    /// unknown key of an object element of its nested configs.
    fn builtin_input(
        &mut self,
        obj: &Map<String, Value>,
        task: &AuthoredPath,
        input: &mut Map<String, Value>,
        known: BuiltinInput,
        step: &Step,
    ) {
        input.retain(|key, value| {
            if !known.keys.contains(&key.as_str()) {
                let at = StrayKey {
                    key,
                    field: key,
                    below: AuthoredPath::root().join(key.as_str()),
                };
                self.stray(obj, task, at, known.keys, std::mem::take(value), step);
                return false;
            }
            if let Some((array, element_keys)) = known.elements
                && key == array
                && let Value::Array(items) = value
            {
                for (i, item) in items.iter_mut().enumerate() {
                    let Value::Object(element) = item else {
                        continue;
                    };
                    element.retain(|stray, val| {
                        if element_keys.contains(&stray.as_str()) {
                            return true;
                        }
                        let at = StrayKey {
                            key: stray,
                            field: array,
                            below: AuthoredPath::root()
                                .join(array)
                                .join(i)
                                .join(stray.as_str()),
                        };
                        self.stray(obj, task, at, element_keys, std::mem::take(val), step);
                        false
                    });
                }
            }
            true
        });
    }

    /// A built-in input's key that its level does not know (`known` are the
    /// level's keys), moved into the `ext` of the task at `task` as
    /// `function.input` followed by the key's path below the input.
    fn stray(
        &mut self,
        obj: &Map<String, Value>,
        task: &AuthoredPath,
        at: StrayKey<'_>,
        known: &[&str],
        value: Value,
        step: &Step,
    ) {
        let StrayKey { key, field, below } = at;
        let rel = AuthoredPath::root()
            .join("function")
            .join("input")
            .extend(&below);
        let name = rel.to_string();
        let (code, message) = unknown_key(key, known);
        let message = format!("{message} (kept in `ext` as `{name}`)");
        let moved = Moved {
            key,
            path: task.extend(&rel),
            field: Some(field),
            name,
            value,
        };
        self.keep_in_ext(obj, task, moved, code, message, step);
    }

    /// Puts a moved key into the `ext` of the node `obj` at `node`, which the
    /// walk goes on building, and reports it; a collision is refused by
    /// [`Ext::keep`], the walk's own rule.
    fn keep_in_ext(
        &mut self,
        obj: &Map<String, Value>,
        node: &AuthoredPath,
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
        let ext = self
            .exts
            .entry(node.clone())
            .or_insert_with(|| Ext::authored(obj.get("ext")));
        match ext.keep(key, name, value, path.clone()) {
            Ok(()) => self.report(code, path, message, Some(step), field),
            Err(collision) => {
                self.report(IssueCode::ExtCollision, path, collision, Some(step), field);
            }
        }
    }
}

/// A built-in input's key that its level does not know.
struct StrayKey<'k> {
    /// The key as authored.
    key: &'k str,
    /// The input's field it sits in: the key itself, or the array holding
    /// the element it sits in.
    field: &'k str,
    /// Its path below the input.
    below: AuthoredPath,
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
