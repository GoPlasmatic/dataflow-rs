//! `parse`: one walk over the authored JSON that builds a [`WorkflowDef`] and
//! is also the linter. With the compat reader on, the reader's pre-pass
//! rewrites the v3 spellings first, and its issues come first.
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
//!
//! A key whose value is `null` is read by its own rule, and the rules are:
//!
//! - `name`, `description`, `loop`, `rollout` and `for_each` treat `null` as
//!   absent, since a missing value and an explicit one mean the same;
//! - `condition` keeps it, as the literal expression `null`, which is falsy:
//!   the step never runs, and a `NULL_CONDITION` Advisory says so;
//! - `over` is refused;
//! - `channel`, `priority`, `tags` and `ext` are a `PARSE_FAILED`, since
//!   `null` is not the type they name.

use super::keys::{Ext, FUNCTION, GROUP, ROLLOUT, TASK, WORKFLOW};
use super::reader::{self, Rewrite};
use super::types::{
    FunctionRef, GroupDef, Logic, NodeHeader, Spelling, StepDef, TaskDef, WorkflowDef, defaults,
};
use super::{MAX_GROUP_DEPTH, Rollout};
use crate::base::any_rejected;
use crate::base::{AuthoredPath, Issue, IssueCode, PathSeg, Severity, StepKind};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

/// How [`parse`] reads a definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParseOptions {
    /// Whether the compat reader runs. With it on (the default), each v3
    /// spelling is rewritten and reported as a `V3_SPELLING` (see
    /// [`compat`](crate::compat)), and an unknown key takes its default
    /// severity and moves into the owning node's `ext`.
    ///
    /// With it off, nothing is rewritten and nothing moves. Every unknown key
    /// is Rejected, and so are the v3 keys of a workflow, a task and a group
    /// (`continue_on_error`, `halt_on`), which are unknown keys to v4. The
    /// v3 spellings inside a task's `function` are not `parse`'s to refuse:
    /// `validate` stays the function's name, and a key a built-in's input
    /// does not know stays in its `input`, for the
    /// function's lookup and config to refuse when the workflow is compiled.
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
/// or ill-typed key is `PARSE_FAILED` at its own path. With the compat reader
/// on, the v3 spellings are rewritten first, each reported as a
/// `V3_SPELLING` at the v3 key, and the reader's issues come before the
/// walk's.
pub fn parse(json: &Value, opts: ParseOptions) -> Parsed {
    parse_with_rewrites(json, opts).0
}

/// Reads a definition by value. The compatibility reader rewrites the owned
/// JSON in place, avoiding the full-document copy required by [`parse`].
/// The resulting definition and issue ordering are identical to `parse`.
pub fn parse_owned(json: Value, opts: ParseOptions) -> Parsed {
    match json {
        Value::Object(object) => parse_object(Cow::Owned(object), opts).0,
        other => parse(&other, opts),
    }
}

/// [`parse`], keeping the compat reader's rewrites for
/// [`migrate_v3`](crate::compat::migrate_v3).
pub(crate) fn parse_with_rewrites(json: &Value, opts: ParseOptions) -> (Parsed, Vec<Rewrite>) {
    let Some(object) = json.as_object() else {
        let parsed = Parsed {
            def: None,
            issues: vec![Issue::new(
                IssueCode::ParseFailed,
                AuthoredPath::root(),
                "a workflow definition must be a JSON object",
            )],
        };
        return (parsed, Vec::new());
    };
    parse_object(Cow::Borrowed(object), opts)
}

fn parse_object(
    mut object: Cow<'_, Map<String, Value>>,
    opts: ParseOptions,
) -> (Parsed, Vec<Rewrite>) {
    if !opts.compat {
        return (walk(&object, false, Vec::new(), HashMap::new()), Vec::new());
    }
    // Borrowed input is copied for the reader; owned input is rewritten in
    // place. The walk builds on the `ext`s the reader moved keys into.
    let read = reader::read(object.to_mut());
    (walk(&object, true, read.issues, read.exts), read.rewrites)
}

/// Reads a fragment: the steps at `root`, as [`parse`] reads a workflow's,
/// with the compat reader when `opts` has it on and every issue under `root`.
///
/// A root under `loop.setup` makes setup steps. Each `tasks` key in `root` is
/// a step list, and every one but a body's first sits inside a group, so the
/// depth limit counts the groups above the fragment: `tasks[2].tasks` starts
/// one group deep, `loop.setup` at the top. Step ids are checked within the
/// fragment. The steps are `Some` exactly when no issue is Rejected, and a
/// fragment that is not an array is `PARSE_FAILED` at `root`.
pub(crate) fn parse_steps(
    steps: &Value,
    root: &AuthoredPath,
    opts: ParseOptions,
) -> (Option<Vec<StepDef>>, Vec<Issue>) {
    let Some(items) = steps.as_array() else {
        let issue = Issue::new(
            IssueCode::ParseFailed,
            root.clone(),
            "steps must be a JSON array",
        );
        return (None, vec![issue]);
    };
    let (setup, depth) = fragment_place(root).unwrap_or((false, 0));
    let rewritten: Value;
    let (list, issues, exts) = if opts.compat {
        // As `parse_with_rewrites` does: the reader rewrites a copy, and the
        // walk reads the copy and goes on building the `ext`s it began.
        let mut copy = items.clone();
        let read = reader::read_steps(&mut copy, root, setup, depth);
        rewritten = Value::Array(copy);
        (&rewritten, read.issues, read.exts)
    } else {
        (steps, Vec::new(), HashMap::new())
    };
    let mut walk = Walk::new(opts.compat, issues, exts);
    let steps = walk.steps(Some(list), root, setup, depth);
    walk.finish();
    let rejected = walk.rejected();
    ((!rejected).then_some(steps), walk.issues)
}

/// Whether `seg` is the key `name`.
fn is(seg: &PathSeg, name: &str) -> bool {
    matches!(seg, PathSeg::Key(key) if &**key == name)
}

/// Where a fragment at `root` sits: under `loop.setup` or not, and inside how
/// many groups. `None` when `root` cannot hold steps: a fragment sits at a
/// `tasks` list or at `loop.setup`, with any number of groups below, so the
/// grammar is `(loop.setup | tasks)([i].tasks)*`. A path that only ends in
/// `tasks`, such as `data.tasks`, names no list of steps.
pub(crate) fn fragment_place(root: &AuthoredPath) -> Option<(bool, usize)> {
    let (setup, mut rest) = match root.segs() {
        [loop_, setup, rest @ ..] if is(loop_, "loop") && is(setup, "setup") => (true, rest),
        [tasks, rest @ ..] if is(tasks, "tasks") => (false, rest),
        _ => return None,
    };
    let mut depth = 0;
    loop {
        match rest {
            [] => return Some((setup, depth)),
            [index, tasks, tail @ ..]
                if matches!(index, PathSeg::Index(_)) && is(tasks, "tasks") =>
            {
                depth += 1;
                rest = tail;
            }
            _ => return None,
        }
    }
}

/// Whether a fragment can sit at `root`: at a `tasks` list, or at
/// `loop.setup`, with any number of groups below.
pub(crate) fn holds_steps(root: &AuthoredPath) -> bool {
    fragment_place(root).is_some()
}

/// Whether a step object is a group: it carries a `tasks` key, whatever its
/// value. The walk and the compat reader classify a step with this alone.
pub(super) fn is_group(obj: &Map<String, Value>) -> bool {
    obj.contains_key("tasks")
}

/// The kind every issue inside a step is stamped with: [`StepKind::Setup`]
/// for any step of a `loop.setup` list, `setup`, else a group's or a task's.
pub(super) fn step_kind(obj: &Map<String, Value>, setup: bool) -> StepKind {
    if setup {
        StepKind::Setup
    } else if is_group(obj) {
        StepKind::Group
    } else {
        StepKind::Task
    }
}

/// Whether a group `depth` groups deep has its members walked: the walk
/// refuses a deeper one as `GROUP_TOO_DEEP` and the reader leaves it as
/// written.
pub(super) fn descends(depth: usize) -> bool {
    depth < MAX_GROUP_DEPTH
}

/// The walk over `object`, its issues following `issues`, with the `ext`s the
/// reader began.
fn walk(
    object: &Map<String, Value>,
    compat: bool,
    issues: Vec<Issue>,
    exts: HashMap<AuthoredPath, Ext>,
) -> Parsed {
    let mut walk = Walk::new(compat, issues, exts);
    let def = walk.workflow(object);
    walk.finish();
    let rejected = walk.rejected();
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
    /// The `ext` of each node the compat reader moved a key into, by path.
    exts: HashMap<AuthoredPath, Ext>,
}

impl<'j> Walk<'j> {
    /// A walk whose issues follow `issues`, with the `ext`s the reader began.
    fn new(compat: bool, issues: Vec<Issue>, exts: HashMap<AuthoredPath, Ext>) -> Self {
        Self {
            compat,
            issues,
            seen: HashMap::new(),
            step: None,
            exts,
        }
    }

    /// Ends the walk: every `ext` the reader began must have been taken by
    /// the node at its path. One left over is a key the reader moved and the
    /// walk never reached, so the Advisory "kept in `ext`" would be false:
    /// Rejected, so the definition is not loaded without it.
    fn finish(&mut self) {
        debug_assert!(
            self.exts.is_empty(),
            "the reader began an ext the walk never took: {:?}",
            self.exts.keys().collect::<Vec<_>>()
        );
        let mut lost: Vec<AuthoredPath> = self.exts.drain().map(|(at, _)| at).collect();
        lost.sort_by_key(ToString::to_string);
        for at in lost {
            self.parse_failed(
                at,
                "the compat reader moved keys into this node's ext, but the walk never \
                 reached the node",
            );
        }
    }

    /// Whether any issue so far is Rejected.
    fn rejected(&self) -> bool {
        any_rejected(&self.issues)
    }

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

    /// The `ext` of the node at `at`: the one the reader began, else the
    /// authored one.
    fn ext(&mut self, at: &AuthoredPath, obj: &Map<String, Value>) -> Ext {
        self.exts
            .remove(at)
            .unwrap_or_else(|| Ext::authored(obj.get("ext")))
    }

    fn workflow(&mut self, obj: &'j Map<String, Value>) -> WorkflowDef {
        let root = AuthoredPath::root();
        let mut ext = self.ext(&root, obj);

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
        let header = self.header(obj, &root);

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
        let priority = self.u32_or(obj, "priority", &root, defaults::PRIORITY);
        let version = self.u32_or(obj, "version", &root, defaults::VERSION);
        let channel = self.string_or(obj, "channel", &root, defaults::CHANNEL);
        let status = self.spelled(obj, "status", &root);
        let on_failure = self.spelled(obj, "on_failure", &root);
        let rollout = self.rollout(obj.get("rollout"), &mut ext);
        let tags = self.tags(obj.get("tags"));
        let created_at = self.timestamp(obj, "created_at");
        let updated_at = self.timestamp(obj, "updated_at");
        self.ext_is_object(obj, &root);
        self.unknown_keys(obj, &WORKFLOW, &[], &root, "", &mut ext);

        WorkflowDef {
            id: id.unwrap_or_default(),
            header: header.with_ext(ext),
            priority,
            condition: self.condition(obj, &root),
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
        let group = is_group(obj);
        let id = non_empty_str(obj.get("id"));
        let kind = step_kind(obj, setup);
        self.step = Some((kind, id));
        let mut ext = self.ext(&at, obj);

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
            let condition = self.condition(obj, &at);
            let on_failure = self.spelled(obj, "on_failure", &at);
            let on_rejection = self.spelled(obj, "on_rejection", &at);
            self.ext_is_object(obj, &at);
            self.unknown_keys(obj, &TASK, &[], &at, "", &mut ext);
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
        let too_deep = !descends(depth);
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
        let condition = self.condition(obj, &at);
        self.ext_is_object(obj, &at);
        // `halt_on` and `for_each` were refused above, not unknown.
        self.unknown_keys(obj, &GROUP, &["halt_on", "for_each"], &at, "", &mut ext);

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
        self.unknown_keys(function, &FUNCTION, &[], &at, "function.", ext);
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

    /// A node's `condition`: absent is none, and a literal `null` is kept as
    /// the falsy expression it is, with an Advisory.
    fn condition(&mut self, obj: &Map<String, Value>, at: &AuthoredPath) -> Option<Logic> {
        match obj.get("condition") {
            None => None,
            Some(Value::Null) => {
                self.report(
                    IssueCode::NullCondition,
                    at.join("condition"),
                    "a literal `null` condition is falsy, so the step never runs; omit it for no \
                     condition",
                );
                Some(Logic(Value::Null))
            }
            Some(value) => Some(Logic(value.clone())),
        }
    }

    /// `ext`, when present, is an object. Its entries were read up front.
    fn ext_is_object(&mut self, obj: &Map<String, Value>, at: &AuthoredPath) {
        if obj.get("ext").is_some_and(|ext| !ext.is_object()) {
            self.parse_failed(at.join("ext"), "expected an object");
        }
    }

    /// An optional string: absent or `null` is `None`.
    fn opt_string(
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
        self.unknown_keys(obj, &ROLLOUT, &[], &at, "rollout.", ext);
        let rollout = Rollout::new(start?, end?);
        if let Err(error) = rollout.check() {
            self.report(IssueCode::InvalidRollout, at, error.to_string());
        }
        Some(rollout)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    type Row = (IssueCode, Severity, String);

    fn rows(issues: &[Issue]) -> Vec<Row> {
        issues
            .iter()
            .map(|i| (i.code, i.severity, i.path.to_string()))
            .collect()
    }

    fn row(code: IssueCode, severity: Severity, path: &str) -> Row {
        (code, severity, path.to_string())
    }

    fn leaf(id: &str) -> Value {
        json!({"id": id, "function": {"name": "f"}})
    }

    fn steps_at(steps: &Value, root: &str) -> (Option<Vec<StepDef>>, Vec<Issue>) {
        crate::def::parse_steps(steps, &root.parse().unwrap(), ParseOptions::default())
    }

    /// `count` groups, each the only member of the one before, around a leaf.
    fn nested(count: usize) -> Value {
        let mut node = leaf("leaf");
        for level in (0..count).rev() {
            node = json!({"id": format!("g{level}"), "tasks": [node]});
        }
        json!([node])
    }

    #[test]
    fn parse_steps_reports_under_its_root_with_the_compat_reader_first() {
        let steps = json!([
            {"function": {"name": "f"}},
            {"id": "b", "continue_on_error": true, "function": {"name": "f"}},
        ]);
        let (def, issues) = steps_at(&steps, "tasks[2].tasks");
        assert!(def.is_none());
        assert_eq!(
            rows(&issues),
            [
                row(
                    IssueCode::V3Spelling,
                    Severity::Advisory,
                    "tasks[2].tasks[1].continue_on_error"
                ),
                row(
                    IssueCode::MissingStepId,
                    Severity::Rejected,
                    "tasks[2].tasks[0].id"
                ),
            ]
        );
        assert_eq!(issues[0].step_kind, Some(StepKind::Task));
        assert_eq!(issues[0].step_id.as_deref(), Some("b"));

        // With the reader off, the v3 key is a Rejected unknown key.
        let (def, issues) = parse_steps(
            &steps,
            &"tasks[2].tasks".parse().unwrap(),
            ParseOptions::default().compat(false),
        );
        assert!(def.is_none());
        assert!(issues.iter().all(|i| i.code != IssueCode::V3Spelling));
        assert!(issues.iter().any(|i| i.severity == Severity::Rejected
            && i.path.to_string() == "tasks[2].tasks[1].continue_on_error"));
    }

    #[test]
    fn a_fragment_sits_only_at_a_tasks_list_or_loop_setup() {
        for root in [
            "tasks",
            "tasks[0].tasks",
            "tasks[0].tasks[1].tasks",
            "loop.setup",
            "loop.setup[0].tasks",
        ] {
            let root: crate::base::AuthoredPath = root.parse().unwrap();
            assert!(holds_steps(&root), "{root}");
        }
        for root in [
            "data.tasks",
            "loop.tasks",
            "loop.setup[0]",
            "tasks[0]",
            "tasks[0].data.tasks",
        ] {
            let root: crate::base::AuthoredPath = root.parse().unwrap();
            assert!(!holds_steps(&root), "{root}");
        }
        assert!(!holds_steps(&crate::base::AuthoredPath::root()));
    }

    #[test]
    fn a_null_condition_is_kept_falsy_with_an_advisory() {
        let steps = json!([{"id": "t", "function": {"name": "f"}, "condition": null}]);
        let (def, issues) = steps_at(&steps, "tasks");
        let def = def.expect("a step");
        assert_eq!(
            rows(&issues),
            [row(
                IssueCode::NullCondition,
                Severity::Advisory,
                "tasks[0].condition"
            )]
        );
        let condition = match &def[0] {
            StepDef::Task(task) => task.condition(),
            StepDef::Group(group) => group.condition(),
        };
        assert!(matches!(condition, Some(Logic(Value::Null))));
    }

    #[test]
    fn parse_steps_under_loop_setup_makes_setup_steps() {
        let steps = json!([{"id": "g", "tasks": [{"id": "s"}]}]);
        let (def, issues) = steps_at(&steps, "loop.setup");
        assert!(def.is_none());
        assert_eq!(
            rows(&issues),
            [row(
                IssueCode::MissingFunction,
                Severity::Rejected,
                "loop.setup[0].tasks[0].function"
            )]
        );
        assert_eq!(issues[0].step_kind, Some(StepKind::Setup));
        assert_eq!(issues[0].step_id.as_deref(), Some("s"));
    }

    #[test]
    fn parse_steps_checks_duplicate_ids_within_the_fragment() {
        let steps = json!([leaf("a"), {"id": "g", "tasks": [leaf("a")]}]);
        let (def, issues) = steps_at(&steps, "tasks");
        assert!(def.is_none());
        assert_eq!(
            rows(&issues),
            [row(
                IssueCode::DuplicateStepId,
                Severity::Rejected,
                "tasks[1].tasks[0].id"
            )]
        );
    }

    #[test]
    fn parse_steps_counts_the_roots_groups_into_the_depth() {
        let deepest = |root: &str, count: usize| {
            let mut path = root.to_string();
            for _ in 1..count {
                path.push_str("[0].tasks");
            }
            path.push_str("[0]");
            path
        };
        // Eight groups deep is accepted under `tasks`, and too deep one
        // group further in.
        let eight = nested(MAX_GROUP_DEPTH);
        let (def, issues) = steps_at(&eight, "tasks");
        assert!(def.is_some() && issues.is_empty(), "{issues:?}");
        let (def, issues) = steps_at(&eight, "tasks[0].tasks");
        assert!(def.is_none());
        assert_eq!(
            rows(&issues),
            [row(
                IssueCode::GroupTooDeep,
                Severity::Rejected,
                &deepest("tasks[0].tasks", MAX_GROUP_DEPTH)
            )]
        );
        // Under `tasks`, the same report takes one group more.
        let (_, issues) = steps_at(&nested(MAX_GROUP_DEPTH + 1), "tasks");
        assert_eq!(
            rows(&issues),
            [row(
                IssueCode::GroupTooDeep,
                Severity::Rejected,
                &deepest("tasks", MAX_GROUP_DEPTH + 1)
            )]
        );
        // `loop.setup` is the top of the setup list, as `tasks` is of the body.
        let (def, issues) = steps_at(&eight, "loop.setup");
        assert!(def.is_some() && issues.is_empty(), "{issues:?}");
        let (_, issues) = steps_at(&eight, "loop.setup[0].tasks");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, IssueCode::GroupTooDeep);
    }

    #[test]
    fn parse_steps_refuses_a_fragment_that_is_not_an_array() {
        for steps in [json!({"id": "a"}), json!(null), json!("tasks")] {
            let (def, issues) = steps_at(&steps, "tasks[2].tasks");
            assert!(def.is_none());
            assert_eq!(
                rows(&issues),
                [row(
                    IssueCode::ParseFailed,
                    Severity::Rejected,
                    "tasks[2].tasks"
                )]
            );
        }
        let (def, issues) = steps_at(&json!([]), "tasks");
        assert_eq!(def, Some(Vec::new()));
        assert!(issues.is_empty());
    }

    #[test]
    fn parse_steps_reads_steps_as_parse_does() {
        // A v3 flag, a stray publish key the reader moves into `ext`, an
        // unknown key the walk moves, and a group's `continue_on_error`.
        let steps = json!([
            {"id": "a", "halt_on": "failure", "function": {"name": "validate"}},
            {"id": "p", "colour": 1, "function": {"name": "publish_json",
                "input": {"source": "x", "target": "y", "pretty": true, "indent": 2}}},
            {"id": "g", "continue_on_error": true, "tasks": [leaf("t")]},
        ]);
        let parsed = parse(
            &json!({"id": "w", "name": "n", "tasks": steps}),
            ParseOptions::default(),
        );
        let (def, issues) = steps_at(&steps, "tasks");
        assert_eq!(issues, parsed.issues);
        assert_eq!(def, parsed.def.map(|def| def.tasks));
        let Some(StepDef::Task(publish)) = def.as_ref().map(|steps| &steps[1]) else {
            panic!("expected the publish task: {def:?}");
        };
        assert!(publish.header.ext.contains_key("function.input.indent"));
        assert!(publish.header.ext.contains_key("colour"));
    }
}
