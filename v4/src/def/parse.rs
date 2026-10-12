//! `parse`: one walk over the authored JSON that builds a [`WorkflowDef`] and
//! is also the linter.
//!
//! The issues come out in v3's rule order, which is not the key order:
//!
//! 1. the workflow `id`;
//! 2. `name` and `description`;
//! 3. `tasks`, a non-empty array;
//! 4. the steps of `tasks`, pre-order. Per node, in v3's order: the id, then
//!    `terminal`, `halt_on` on a group, `for_each` (and its unknown keys), then
//!    by kind: the function (and its unknown keys), an empty group, or a group
//!    too deep. Then the node's other keys, its unknown keys, and its members;
//! 5. `loop`, in v3's `check_loop` order, then its unknown keys;
//! 6. the steps of `loop.setup`, walked like `tasks`;
//! 7. the workflow's other keys, `rollout` with its unknown keys, then the
//!    workflow's own unknown keys.

use super::keys::{Ext, FUNCTION_KEYS, GROUP_KEYS, ROLLOUT_KEYS, TASK_KEYS, WORKFLOW_KEYS};
use super::types::{
    FunctionRef, GroupDef, Logic, NodeHeader, Spelling, StepDef, TaskDef, WorkflowDef,
};
use super::{MAX_GROUP_DEPTH, Rollout};
use crate::base::{AuthoredPath, Issue, IssueCode, Severity, StepKind};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::LazyLock;

/// How [`parse`] reads a definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParseOptions {
    /// Whether the compat reader runs. With it on (the default), an unknown
    /// key takes its default severity and moves into the owning node's `ext`.
    /// With it off, every unknown key is Rejected and nothing moves.
    pub compat: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self { compat: true }
    }
}

impl ParseOptions {
    /// Turns the compat reader on or off.
    #[must_use]
    pub fn compat(mut self, on: bool) -> Self {
        self.compat = on;
        self
    }
}

/// What [`parse`] found.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Parsed {
    /// The definition: `Some` exactly when no issue is Rejected.
    pub def: Option<WorkflowDef>,
    /// Every issue, in rule order, each at its authored path.
    pub issues: Vec<Issue>,
}

/// Reads one authored workflow definition. Never fails: every problem is an
/// issue, and the definition is built exactly when none is Rejected.
///
/// Every structural rule is here, at v3's codes and authored paths. A missing
/// or ill-typed key is `PARSE_FAILED` at its own path.
pub fn parse(json: &Value, opts: ParseOptions) -> Parsed {
    let Some(object) = json.as_object() else {
        return Parsed {
            def: None,
            issues: vec![Issue::new(
                IssueCode::ParseFailed,
                AuthoredPath::root(),
                "a workflow definition must be a JSON object",
            )],
        };
    };
    let mut walk = Walk {
        compat: opts.compat,
        issues: Vec::new(),
        seen: HashMap::new(),
        step: None,
    };
    let def = walk.workflow(object);
    let rejected = walk.issues.iter().any(|i| i.severity == Severity::Rejected);
    Parsed {
        def: (!rejected).then_some(def),
        issues: walk.issues,
    }
}

/// The walk's state: the issues so far, the step ids seen, and the step the
/// walk is in, which stamps every issue reported inside it.
pub(super) struct Walk<'j> {
    pub(super) compat: bool,
    issues: Vec<Issue>,
    /// Step id → the path that first claimed it. One namespace for `tasks`
    /// and `loop.setup`.
    seen: HashMap<&'j str, AuthoredPath>,
    /// The kind and id of the step being walked.
    step: Option<(StepKind, Option<&'j str>)>,
}

impl<'j> Walk<'j> {
    /// Reports at the code's default severity.
    pub(super) fn report(
        &mut self,
        code: IssueCode,
        path: AuthoredPath,
        message: impl Into<String>,
    ) {
        self.report_as(code, code.severity(), path, message);
    }

    /// Reports at `severity`, stamped with the step being walked.
    pub(super) fn report_as(
        &mut self,
        code: IssueCode,
        severity: Severity,
        path: AuthoredPath,
        message: impl Into<String>,
    ) {
        let mut issue = Issue::new(code, path, message).with_severity(severity);
        if let Some((kind, id)) = self.step {
            issue = issue.with_step(kind, id);
        }
        self.issues.push(issue);
    }

    /// A key of the wrong type, or a required key missing.
    pub(super) fn parse_failed(&mut self, path: AuthoredPath, message: impl Into<String>) {
        self.report(IssueCode::ParseFailed, path, message);
    }

    fn workflow(&mut self, obj: &'j Map<String, Value>) -> WorkflowDef {
        let root = AuthoredPath::root();
        let mut ext = Ext::authored(obj.get("ext"));

        // 1.
        let id = non_empty_str(obj.get("id")).map(str::to_string);
        if id.is_none() {
            self.report(
                IssueCode::EmptyWorkflowId,
                root.join("id"),
                "workflow id must be a non-empty string",
            );
        }

        // 2.
        let name = self.opt_string(obj, "name", &root);
        let description = self.opt_string(obj, "description", &root);

        // 3.
        let tasks = obj.get("tasks");
        if !tasks
            .and_then(Value::as_array)
            .is_some_and(|tasks| !tasks.is_empty())
        {
            self.report(
                IssueCode::NoTasks,
                root.join("tasks"),
                "workflow must have at least one task",
            );
        }

        // 4.
        let tasks = self.steps(tasks, &root.join("tasks"), false, 0);
        self.step = None;

        // 5 and 6.
        let r#loop = match obj.get("loop") {
            None | Some(Value::Null) => None,
            Some(Value::Object(config)) => Some(self.r#loop(config, &mut ext)),
            Some(_) => {
                self.parse_failed(root.join("loop"), "expected an object");
                None
            }
        };
        self.step = None;

        // 7.
        let priority = self.u32_or(obj, "priority", &root, 0);
        let version = self.u32_or(obj, "version", &root, 1);
        let channel = self.string_or(obj, "channel", &root, "default");
        let status = self.spelled(obj, "status", &root);
        let on_failure = self.spelled(obj, "on_failure", &root);
        let rollout = self.rollout(obj.get("rollout"), &mut ext);
        let tags = self.tags(obj.get("tags"));
        let created_at = self.timestamp(obj, "created_at");
        let updated_at = self.timestamp(obj, "updated_at");
        self.ext_is_object(obj, &root);
        self.unknown_keys(obj, WORKFLOW_KEYS, &[], &root, "", &mut ext);

        WorkflowDef {
            id: id.unwrap_or_default(),
            header: NodeHeader {
                name,
                description,
                ext: ext.into_map(),
            },
            priority,
            condition: obj.get("condition").cloned().map(Logic),
            tasks,
            on_failure,
            channel,
            status,
            version,
            rollout,
            r#loop,
            tags,
            created_at,
            updated_at,
        }
    }

    /// The steps of a list, `tasks` or `loop.setup`, at `at`. A list that is
    /// not an array yields none; its owner reports the shape.
    pub(super) fn steps(
        &mut self,
        list: Option<&'j Value>,
        at: &AuthoredPath,
        setup: bool,
        depth: usize,
    ) -> Vec<StepDef> {
        let Some(items) = list.and_then(Value::as_array) else {
            return Vec::new();
        };
        items
            .iter()
            .enumerate()
            .map(|(i, node)| self.step(node, at.join(i), setup, depth))
            .collect()
    }

    /// One step, then its members. An element that is not an object is a
    /// task with neither id nor function, as v3's walker reads it.
    fn step(&mut self, node: &'j Value, at: AuthoredPath, setup: bool, depth: usize) -> StepDef {
        static EMPTY: LazyLock<Map<String, Value>> = LazyLock::new(Map::new);
        let obj: &'j Map<String, Value> = node.as_object().unwrap_or(&EMPTY);
        let group = obj.contains_key("tasks");
        let id = non_empty_str(obj.get("id"));
        let kind = if setup {
            StepKind::Setup
        } else if group {
            StepKind::Group
        } else {
            StepKind::Task
        };
        self.step = Some((kind, id));
        let mut ext = Ext::authored(obj.get("ext"));

        // 4.2: the id and its namespace.
        match id {
            None => self.report(
                IssueCode::MissingStepId,
                at.join("id"),
                "every step needs a non-empty id",
            ),
            Some(id) => {
                if let Some(first) = self.seen.get(id) {
                    let message = format!(
                        "step id '{id}' is already used at {first} — task groups share the \
                         task id namespace"
                    );
                    self.report(IssueCode::DuplicateStepId, at.join("id"), message);
                } else {
                    self.seen.insert(id, at.clone());
                }
            }
        }

        // 4.3
        let terminal = match obj.get("terminal") {
            None => false,
            Some(Value::Bool(terminal)) => *terminal,
            Some(other) => {
                // `"on_failure"` is the natural wrong guess for an outcome
                // rule; answer it rather than report a bare type error.
                let message = if other.as_str() == Some("on_failure") {
                    "terminal must be a boolean — for \"halt if this task failed\" use \
                     `\"halt_on\": \"failure\"`, which is the outcome axis"
                } else {
                    "terminal must be a boolean"
                };
                self.report(IssueCode::InvalidTerminal, at.join("terminal"), message);
                false
            }
        };

        let id = id.unwrap_or_default().to_string();
        if group {
            let head = GroupHead {
                at,
                id,
                terminal,
                ext,
                setup,
                depth,
            };
            self.group(obj, head)
        } else {
            // 4.5
            let function_name = obj
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str);
            let for_each = self.for_each(obj.get("for_each"), function_name, &at, &mut ext);
            // 4.6
            let function = self.function(obj, &at, &mut ext);
            // 4.8
            let header = self.header(obj, &at);
            let condition = obj.get("condition").cloned().map(Logic);
            let on_failure = self.spelled(obj, "on_failure", &at);
            let on_rejection = self.spelled(obj, "on_rejection", &at);
            self.ext_is_object(obj, &at);
            self.unknown_keys(obj, TASK_KEYS, &[], &at, "", &mut ext);
            StepDef::Task(TaskDef {
                id,
                header: header.with_ext(ext),
                condition,
                function,
                on_failure,
                on_rejection,
                terminal,
                for_each,
            })
        }
    }

    /// The rest of a group, from 4.4 on, then its members.
    fn group(&mut self, obj: &'j Map<String, Value>, head: GroupHead) -> StepDef {
        let GroupHead {
            at,
            id,
            terminal,
            mut ext,
            setup,
            depth,
        } = head;
        // 4.4
        if obj.contains_key("halt_on") {
            self.report(
                IssueCode::InvalidHaltOn,
                at.join("halt_on"),
                "halt_on is a per-task outcome rule and a group has no outcome of its own — \
                 put it on the task that can fail",
            );
        }
        // 4.5
        if obj.contains_key("for_each") {
            self.report(
                IssueCode::InvalidForEach,
                at.join("for_each"),
                "a task group cannot carry for_each — for_each runs one task's function once \
                 per element; put it on the task",
            );
        }
        // 4.1 (v3 reports the depth after the node's other rules) and 4.7.
        let too_deep = depth >= MAX_GROUP_DEPTH;
        let members = obj.get("tasks");
        if too_deep {
            self.report(
                IssueCode::GroupTooDeep,
                at.clone(),
                format!("task groups nested deeper than {MAX_GROUP_DEPTH} levels"),
            );
        } else if !members
            .and_then(Value::as_array)
            .is_some_and(|members| !members.is_empty())
        {
            self.report(
                IssueCode::EmptyGroup,
                at.join("tasks"),
                "a task group's tasks must be a non-empty array — an empty group can only be \
                 a mistake",
            );
        }
        // 4.8
        let header = self.header(obj, &at);
        let condition = obj.get("condition").cloned().map(Logic);
        self.ext_is_object(obj, &at);
        // `halt_on` and `for_each` were refused above, not unknown.
        self.unknown_keys(obj, GROUP_KEYS, &["halt_on", "for_each"], &at, "", &mut ext);

        // A group too deep is not descended into.
        let tasks = if too_deep {
            Vec::new()
        } else {
            self.steps(members, &at.join("tasks"), setup, depth + 1)
        };
        StepDef::Group(GroupDef {
            id,
            header: header.with_ext(ext),
            condition,
            terminal,
            tasks,
        })
    }

    /// 4.6: a task's function, and its unknown keys.
    fn function(
        &mut self,
        obj: &Map<String, Value>,
        at: &AuthoredPath,
        ext: &mut Ext,
    ) -> FunctionRef {
        let missing = FunctionRef {
            name: String::new(),
            input: Value::Object(Map::new()),
        };
        let Some(raw) = obj.get("function") else {
            self.report(
                IssueCode::MissingFunction,
                at.join("function"),
                "a task needs a function — an element with neither `function` nor `tasks` is \
                 neither a task nor a group",
            );
            return missing;
        };
        let at = at.join("function");
        let Some(function) = raw.as_object() else {
            self.invalid_function_name(&at);
            return missing;
        };
        let name = non_empty_str(function.get("name"));
        if name.is_none() {
            self.invalid_function_name(&at);
        }
        self.unknown_keys(function, FUNCTION_KEYS, &[], &at, "function.", ext);
        FunctionRef {
            name: name.unwrap_or_default().to_string(),
            input: function
                .get("input")
                .cloned()
                .unwrap_or_else(|| Value::Object(Map::new())),
        }
    }

    fn invalid_function_name(&mut self, at: &AuthoredPath) {
        self.report(
            IssueCode::InvalidFunctionName,
            at.join("name"),
            "function must be an object with a non-empty name",
        );
    }

    /// A node's `name` and `description`, with an empty `ext` for now.
    fn header(&mut self, obj: &Map<String, Value>, at: &AuthoredPath) -> NodeHeader {
        NodeHeader {
            name: self.opt_string(obj, "name", at),
            description: self.opt_string(obj, "description", at),
            ext: Map::new(),
        }
    }

    /// `ext`, when present, is an object. Its entries were read up front.
    fn ext_is_object(&mut self, obj: &Map<String, Value>, at: &AuthoredPath) {
        if obj.get("ext").is_some_and(|ext| !ext.is_object()) {
            self.parse_failed(at.join("ext"), "expected an object");
        }
    }

    /// An optional string: absent or `null` is `None`.
    pub(super) fn opt_string(
        &mut self,
        obj: &Map<String, Value>,
        key: &str,
        at: &AuthoredPath,
    ) -> Option<String> {
        match obj.get(key) {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => Some(text.clone()),
            Some(_) => {
                self.parse_failed(at.join(key), "expected a string");
                None
            }
        }
    }

    fn string_or(
        &mut self,
        obj: &Map<String, Value>,
        key: &str,
        at: &AuthoredPath,
        default: &str,
    ) -> String {
        match obj.get(key) {
            None => default.to_string(),
            Some(Value::String(text)) => text.clone(),
            Some(_) => {
                self.parse_failed(at.join(key), "expected a string");
                default.to_string()
            }
        }
    }

    fn u32_or(
        &mut self,
        obj: &Map<String, Value>,
        key: &str,
        at: &AuthoredPath,
        default: u32,
    ) -> u32 {
        let Some(raw) = obj.get(key) else {
            return default;
        };
        raw.as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or_else(|| {
                self.parse_failed(
                    at.join(key),
                    "expected a non-negative integer that fits in 32 bits",
                );
                default
            })
    }

    /// An enum read from its spelling; absent is the default.
    fn spelled<T: Spelling>(
        &mut self,
        obj: &Map<String, Value>,
        key: &str,
        at: &AuthoredPath,
    ) -> T {
        let Some(raw) = obj.get(key) else {
            return T::default();
        };
        raw.as_str().and_then(T::from_spelling).unwrap_or_else(|| {
            let spellings: Vec<String> = T::ALL
                .iter()
                .map(|v| format!("\"{}\"", v.spelling()))
                .collect();
            self.parse_failed(
                at.join(key),
                format!("expected one of {}", spellings.join(", ")),
            );
            T::default()
        })
    }

    fn tags(&mut self, raw: Option<&Value>) -> Vec<String> {
        let at = AuthoredPath::root().join("tags");
        match raw {
            None => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| match item {
                    Value::String(tag) => Some(tag.clone()),
                    _ => {
                        self.parse_failed(at.join(i), "expected a string");
                        None
                    }
                })
                .collect(),
            Some(_) => {
                self.parse_failed(at, "expected an array of strings");
                Vec::new()
            }
        }
    }

    /// A timestamp, read as v3's serde read it: chrono's `FromStr`, which
    /// takes RFC 3339 and its relaxed forms (a space for the `T`, an offset
    /// without its colon, and chrono's own `Display`, `… UTC`).
    fn timestamp(&mut self, obj: &Map<String, Value>, key: &str) -> Option<DateTime<Utc>> {
        match obj.get(key) {
            None | Some(Value::Null) => None,
            Some(raw) => {
                let parsed = raw
                    .as_str()
                    .and_then(|text| text.parse::<DateTime<Utc>>().ok());
                if parsed.is_none() {
                    self.parse_failed(
                        AuthoredPath::root().join(key),
                        "expected a timestamp such as \"2026-01-02T03:04:05Z\"",
                    );
                }
                parsed
            }
        }
    }

    /// `rollout`, then its unknown keys. Absent or `null` is no rollout.
    fn rollout(&mut self, raw: Option<&Value>, ext: &mut Ext) -> Option<Rollout> {
        let at = AuthoredPath::root().join("rollout");
        let obj = match raw {
            None | Some(Value::Null) => return None,
            Some(Value::Object(obj)) => obj,
            Some(_) => {
                self.parse_failed(at, "expected an object");
                return None;
            }
        };
        let start = self.bucket(obj, "bucket_start", &at);
        let end = self.bucket(obj, "bucket_end", &at);
        self.unknown_keys(obj, ROLLOUT_KEYS, &[], &at, "rollout.", ext);
        Some(Rollout::new(start?, end?))
    }

    fn bucket(&mut self, obj: &Map<String, Value>, key: &str, at: &AuthoredPath) -> Option<u8> {
        let Some(raw) = obj.get(key) else {
            self.parse_failed(at.join(key), format!("missing field `{key}`"));
            return None;
        };
        let bucket = raw.as_u64().and_then(|n| u8::try_from(n).ok());
        if bucket.is_none() {
            self.parse_failed(at.join(key), "expected an integer from 0 to 255");
        }
        bucket
    }
}

/// What `step` read of a group before handing it to `group`.
struct GroupHead {
    at: AuthoredPath,
    id: String,
    terminal: bool,
    ext: Ext,
    /// Whether the group sits under `loop.setup`.
    setup: bool,
    /// Its enclosing groups.
    depth: usize,
}

impl NodeHeader {
    fn with_ext(mut self, ext: Ext) -> Self {
        self.ext = ext.into_map();
        self
    }
}

/// The value as a non-empty string, if it is one.
fn non_empty_str(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|s| !s.is_empty())
}
