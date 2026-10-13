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
//! | `publish_json` / `publish_xml` input | `source: "data.<x>"` | `source: "<x>"` |
//!
//! Each rewrite is a `V3_SPELLING` Advisory at the v3 key, and a
//! [`Rewrite`]. Two moves are not rewrites, and are reported as the keys they
//! are:
//!
//! - **A key a built-in's input does not know (I54)** moves into the task's
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
//! A publish `source` is written relative to `data` in v4, and v3 fell back
//! to reading `data.<x>` as `<x>`, so a literal one under `data.` is rewritten
//! to the rest (I56). A computed `source`, `"data"` itself and a `target` are
//! not: `target`'s `data.t` was `data.data.t` in v3 too.
//!
//! The reader refuses a `continue_on_error` that is not a bool
//! (`PARSE_FAILED`), a `halt_on` that is not `"never"` or `"failure"`
//! (`INVALID_HALT_ON`, v3's message), and a v3 key written beside a v4 one
//! (`PARSE_FAILED` at the v3 key, as v3's duplicate field was): on one node,
//! any v3 policy key (`continue_on_error`, a task's `halt_on`) beside any v4
//! policy key (`on_failure`, a task's `on_rejection`), and an alias beside its
//! target. The two flags make one policy, so a v3 flag beside the v4 key of
//! the other axis would be a guess at how the author meant them to combine. A
//! refused key is taken out all the same, so the walk does not report it
//! twice.

use super::MAX_GROUP_DEPTH;
use super::keys::{
    BuiltinInput, Ext, GROUP_V3_KEYS, TASK_POLICIES, TASK_V3_KEYS, WORKFLOW_POLICIES,
    WORKFLOW_V3_KEYS, builtin_input,
};
use crate::base::typo::unknown_key;
use crate::base::{AuthoredPath, Issue, IssueCode, PathSeg, StepKind};
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
/// - a function name or an input key is the bare name: `validate` read as
///   `validation`, `path_logic` read as `path`;
/// - a publish `source` is `source: value`: `source: data.obj` read as
///   `source: obj`.
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
        let wanted: Vec<&'static str> = WORKFLOW_V3_KEYS
            .iter()
            .copied()
            .chain(["loop", "tasks"])
            .collect();
        for key in keys_among(obj, &wanted) {
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
        let wanted: Vec<&'static str> = GROUP_V3_KEYS.iter().copied().chain(["tasks"]).collect();
        for key in keys_among(obj, &wanted) {
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
        self.keep_in_ext(obj, at, moved, code, message, step);
    }

    fn task(&mut self, obj: &mut Map<String, Value>, at: &AuthoredPath, step: &Step) {
        let wanted: Vec<&'static str> = TASK_V3_KEYS.iter().copied().chain(["function"]).collect();
        let order = keys_among(obj, &wanted);
        let has_flags = TASK_V3_KEYS.iter().any(|key| obj.contains_key(*key));
        // Any v4 policy beside any v3 flag refuses every flag.
        let v4_policies = policies_in(obj, TASK_POLICIES);
        let refuse_mixed = |v3: &str| Flag::Refused {
            code: IssueCode::ParseFailed,
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
        let v4 = if has_flags && v4_policies.is_empty() {
            let (on_failure, on_rejection) = match (continue_on_error.is_set(), halt_on.is_set()) {
                (false, false) => ("fail", "continue"),
                (true, false) => ("continue", "continue"),
                (false, true) => ("fail", "halt"),
                (true, true) => ("halt", "halt"),
            };
            obj.insert("on_failure".into(), Value::String(on_failure.into()));
            obj.insert("on_rejection".into(), Value::String(on_rejection.into()));
            format!("on_failure: {on_failure}, on_rejection: {on_rejection}")
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
        // Taken out of the task for the moment, so the task's authored `ext`
        // can be read as its input changes; put back in its place below.
        let mut fields = std::mem::take(slot);
        let task = at;
        let at = at.join("function");
        let name = fields
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // `validate`'s input is `validation`'s, whichever key comes first.
        let builtin = builtin_input(if name == "validate" {
            "validation"
        } else {
            &name
        });
        let publish = name == "publish_json" || name == "publish_xml";
        for key in keys_among(&fields, &["input", "name"]) {
            if key == "name" {
                if name == "validate" {
                    fields.insert("name".into(), Value::String("validation".into()));
                    let (v3, v4) = ("validate".into(), "validation".into());
                    self.rewrite(at.join("name"), v3, v4, Some(step), None);
                }
            } else if let Some(Value::Object(input)) = fields.get_mut("input") {
                match builtin {
                    Some(known) => self.builtin_input(obj, task, input, known, publish, step),
                    None => self.aliases(input, &name, &at.join("input"), step),
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

    /// A built-in's input, in its keys' order (I54, I56). A key the built-in
    /// does not know moves into the `ext` of the task at `task`, and so does
    /// an unknown key of an object element of its nested configs; a publish
    /// `source` under `data.` is rewritten to the rest.
    fn builtin_input(
        &mut self,
        obj: &Map<String, Value>,
        task: &AuthoredPath,
        input: &mut Map<String, Value>,
        known: BuiltinInput,
        publish: bool,
        step: &Step,
    ) {
        let order: Vec<String> = input.keys().cloned().collect();
        for key in order {
            if !known.keys.contains(&key.as_str()) {
                if let Some(value) = take(input, &key) {
                    let below = AuthoredPath::root().join(key.as_str());
                    self.stray(obj, task, &below, known.keys, value, step);
                }
            } else if let Some((array, element_keys)) = known.elements
                && key == array
                && let Some(Value::Array(items)) = input.get_mut(array)
            {
                for (i, item) in items.iter_mut().enumerate() {
                    let Value::Object(element) = item else {
                        continue;
                    };
                    let strays: Vec<String> = element
                        .keys()
                        .filter(|k| !element_keys.contains(&k.as_str()))
                        .cloned()
                        .collect();
                    for stray in strays {
                        if let Some(value) = take(element, &stray) {
                            let below = AuthoredPath::root()
                                .join(array)
                                .join(i)
                                .join(stray.as_str());
                            self.stray(obj, task, &below, element_keys, value, step);
                        }
                    }
                }
            } else if publish && key == "source" {
                self.publish_source(input, task, step);
            }
        }
    }

    /// A built-in input's key at `below` the input that its level does not
    /// know (`known` are the level's keys), moved into the `ext` of the task
    /// at `task` as `function.input` followed by `below`.
    fn stray(
        &mut self,
        obj: &Map<String, Value>,
        task: &AuthoredPath,
        below: &AuthoredPath,
        known: &[&str],
        value: Value,
        step: &Step,
    ) {
        let key_of = |seg: Option<&PathSeg>| match seg {
            Some(PathSeg::Key(key)) => key.to_string(),
            _ => unreachable!("a stray's path starts and ends at a key"),
        };
        let (key, field) = (key_of(below.segs().last()), key_of(below.segs().first()));
        let input = AuthoredPath::root().join("function").join("input");
        let name = input.extend(below).to_string();
        let (code, message) = unknown_key(&key, known);
        let message = format!("{message} (kept in `ext` as `{name}`)");
        let moved = Moved {
            key: &key,
            path: task.extend(&input).extend(below),
            field: Some(&field),
            name,
            value,
        };
        self.keep_in_ext(obj, task, moved, code, message, step);
    }

    /// A publish input's literal `source` under `data.`, rewritten to the
    /// rest (I56): v4 reads the source relative to `data`, and v3 read
    /// `data.<x>` as `<x>` when nothing else matched.
    fn publish_source(&mut self, input: &mut Map<String, Value>, task: &AuthoredPath, step: &Step) {
        let Some(Value::String(source)) = input.get_mut("source") else {
            return;
        };
        let Some(rest) = source.strip_prefix("data.").map(str::to_string) else {
            return;
        };
        let (v3, v4) = (format!("source: {source}"), format!("source: {rest}"));
        *source = rest;
        let path = task.join("function").join("input").join("source");
        self.rewrite(path, v3, v4, Some(step), Some("source"));
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
                self.report(IssueCode::ParseFailed, path, collision, Some(step), field);
            }
        }
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
