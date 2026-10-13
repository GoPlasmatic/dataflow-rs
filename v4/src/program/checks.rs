//! The checks visitor: one hook the compile walk calls at every expression
//! site, with the expression's absolute path, its raw JSON, whether the
//! engine records its result, and the step it sits in; and once more, over
//! the whole workflow, for the workflow lint.
//!
//! Per expression the rules run in this order:
//!
//! 1. **Secrets.** An expression the engine records may not read a secret at
//!    all: one `SECRET_IN_MESSAGE_WRITE` at the field. Anywhere else, each
//!    literal name the engine's secrets lack is `UNKNOWN_SECRET` at the field.
//! 2. **Template keys.** `ESCAPED_TEMPLATE_KEY` at each key starting with
//!    exactly one escape, then `DUPLICATE_TEMPLATE_KEY` at the later of two
//!    keys of one object that emit the same name.
//! 3. **Payload.** `PAYLOAD_VAR` at each `var` or `val` object reading a
//!    literal path rooted at `payload`, which is not in the evaluation
//!    context.
//!
//! The workflow lint is `UNGUARDED_VALIDATION`. A fragment has none.
//!
//! 4. **Fingerprint.** Each single-key object is classified by its key: an
//!    operator of the vocabulary, data, or (an escaped key) nothing. See
//!    [`Fingerprint`].

use super::fingerprint::Fingerprint;
use crate::base::{AuthoredPath, Issue, IssueCode, PathSeg, StepKind};
use crate::def::{Logic, RejectionPolicy, StepDef, TaskDef, WorkflowDef};
use crate::expr::{ExprEngine, ExprVisitor, escaped_keys, secret_refs};
use serde_json::Value;
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap};

/// The step an expression sits in: its kind and id. `None` for the
/// workflow's own expressions.
pub(crate) type Stamp<'s> = Option<(StepKind, &'s str)>;

/// Where an expression sits.
#[derive(Clone, Copy)]
pub(crate) struct Site<'s> {
    /// The expression's absolute authored path.
    pub(crate) at: &'s AuthoredPath,
    /// Whether the engine records the expression's result.
    pub(crate) records: bool,
    /// The step it sits in.
    pub(crate) step: Stamp<'s>,
    /// The config field it sits in, the first key below `function.input`,
    /// when it is in a config.
    pub(crate) field: Option<&'s str>,
}

impl Site<'_> {
    /// `issue`, stamped with the site's step and field, as an issue a config
    /// reports is stamped.
    fn stamp(&self, mut issue: Issue) -> Issue {
        if let Some((kind, id)) = self.step {
            issue = issue.with_step(kind, Some(id));
        }
        if let Some(field) = self.field {
            issue = issue.with_field(field);
        }
        issue
    }
}

/// What the compile walk calls: [`Checks`] in a compile. Issues go to
/// `issues`, in the order they are found.
pub(crate) trait Hook {
    /// One expression, at `site`, compiled from `raw`.
    fn expr(&mut self, site: Site<'_>, raw: &Value, issues: &mut Vec<Issue>);

    /// The whole workflow, after its steps. Not called for a fragment.
    fn workflow(&mut self, _def: &WorkflowDef, _issues: &mut Vec<Issue>) {}

    /// The fingerprint of what the hook has seen.
    fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of(&BTreeSet::new(), &BTreeSet::new())
    }
}

/// The compile checks over one workflow or fragment.
pub(crate) struct Checks<'e> {
    /// The vocabulary and secrets the rules read.
    exprs: &'e ExprEngine,
    /// The operators the expressions use.
    ops: BTreeSet<String>,
    /// The data keys the expressions use.
    data: BTreeSet<String>,
}

impl<'e> Checks<'e> {
    /// The checks against `exprs`.
    pub(crate) fn new(exprs: &'e ExprEngine) -> Self {
        Self {
            exprs,
            ops: BTreeSet::new(),
            data: BTreeSet::new(),
        }
    }

    /// Rule 4: classify every single-key object below `raw`.
    fn classify(&mut self, raw: &Value) {
        match raw {
            Value::Object(map) => {
                if let Some(key) = map.keys().next().filter(|_| map.len() == 1) {
                    if self.exprs.operators().iter().any(|op| op.name == *key) {
                        self.ops.insert(key.clone());
                    } else if !key.starts_with(self.exprs.escape()) {
                        self.data.insert(key.clone());
                    }
                }
                map.values().for_each(|child| self.classify(child));
            }
            Value::Array(items) => items.iter().for_each(|item| self.classify(item)),
            _ => {}
        }
    }

    /// Rule 1: a recorded expression reading any secret, or a literal name
    /// the engine does not declare. Both point at the field.
    fn secrets(&self, site: Site<'_>, raw: &Value, report: &mut Report<'_>) {
        let mut refs = secret_refs(raw).peekable();
        if refs.peek().is_none() {
            return;
        }
        if site.records {
            report(
                IssueCode::SecretInMessageWrite,
                site.at.clone(),
                "reads a secret, and the engine records this expression's result; \
                 compute a derived value in a host function instead"
                    .to_owned(),
            );
            return;
        }
        for (name, _) in refs {
            if let Some(name) = name
                && self.exprs.secrets().get(name).is_none()
            {
                report(
                    IssueCode::UnknownSecret,
                    site.at.clone(),
                    format!("secret `{name}` is not declared on the engine"),
                );
            }
        }
    }

    /// Rule 2: every escaped key, then every duplicate, at its deep path.
    fn template_keys(&self, at: &AuthoredPath, raw: &Value, report: &mut Report<'_>) {
        let escape = self.exprs.escape();
        for key_at in escaped_keys(raw) {
            let Some(PathSeg::Key(key)) = key_at.segs().last() else {
                continue;
            };
            let emitted = &key[escape.len_utf8()..];
            let message = format!(
                "`{key}` is emitted as `{emitted}`: one `{escape}` is stripped from every \
                 template key. Double it to `{escape}{key}` to emit `{key}` itself"
            );
            report(IssueCode::EscapedTemplateKey, at.extend(&key_at), message);
        }
        for (key_at, message) in duplicate_keys(raw, escape) {
            report(IssueCode::DuplicateTemplateKey, at.extend(&key_at), message);
        }
    }

    /// Rule 3: every literal read rooted at `payload`, at its object.
    fn payload_vars(&self, at: &AuthoredPath, raw: &Value, report: &mut Report<'_>) {
        let scoping = |name: &str| {
            self.exprs
                .operators()
                .iter()
                .any(|op| op.scoping && op.name == name)
        };
        let mut found = Vec::new();
        walk_payload(raw, &AuthoredPath::root(), &scoping, &mut found);
        for (object_at, path) in found {
            let message = format!(
                "reads `{path}`, but the payload is not in the evaluation context, which \
                 holds only `data`, `metadata` and `temp_data`, so this always resolves to \
                 nothing; parse the payload into `data` first"
            );
            report(IssueCode::PayloadVar, at.extend(&object_at), message);
        }
    }
}

/// Where a rule reports: its code, absolute path and message.
type Report<'r> = dyn FnMut(IssueCode, AuthoredPath, String) + 'r;

impl Hook for Checks<'_> {
    fn expr(&mut self, site: Site<'_>, raw: &Value, issues: &mut Vec<Issue>) {
        let mut report = |code, at, message: String| {
            issues.push(site.stamp(Issue::new(code, at, message)));
        };
        self.secrets(site, raw, &mut report);
        self.template_keys(site.at, raw, &mut report);
        self.payload_vars(site.at, raw, &mut report);
        self.classify(raw);
    }

    fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of(&self.ops, &self.data)
    }

    fn workflow(&mut self, def: &WorkflowDef, issues: &mut Vec<Issue>) {
        unguarded_validation(def, issues);
    }
}

/// Every key that emits the same name as an earlier key of its object, once
/// one leading `escape` is stripped from each, with its path below `raw` and a
/// message naming both. `$$a` emits `$a` and `$a` emits `a`.
///
/// Each object's own keys come before the keys of the values below it, and an
/// object's keys go in its iteration order, so the later key is reported.
fn duplicate_keys(raw: &Value, escape: char) -> Vec<(AuthoredPath, String)> {
    let mut out = Vec::new();
    walk_duplicates(raw, &AuthoredPath::root(), escape, &mut out);
    out
}

fn walk_duplicates(
    value: &Value,
    at: &AuthoredPath,
    escape: char,
    out: &mut Vec<(AuthoredPath, String)>,
) {
    match value {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                walk_duplicates(item, &at.join(i), escape, out);
            }
        }
        Value::Object(map) => {
            let mut emitted: HashMap<&str, &str> = HashMap::new();
            for key in map.keys() {
                let name = key.strip_prefix(escape).unwrap_or(key);
                match emitted.entry(name) {
                    Entry::Vacant(slot) => {
                        slot.insert(key);
                    }
                    Entry::Occupied(first) => {
                        let first = first.get();
                        let message = format!(
                            "`{first}` and `{key}` both emit the key `{name}`, so the object \
                             would carry it twice: a later read sees only the first, while \
                             serialization emits both"
                        );
                        out.push((at.join(key.as_str()), message));
                    }
                }
            }
            for (key, child) in map {
                walk_duplicates(child, &at.join(key.as_str()), escape, out);
            }
        }
        _ => {}
    }
}

/// Every `var` or `val` object below `value` reading a literal path rooted at
/// `payload`: the object's path and the path it reads.
///
/// An argument a scoping operator evaluates per element (each after its
/// first) is skipped: there `var` reads the element, which may well have a
/// `payload` key.
fn walk_payload(
    value: &Value,
    at: &AuthoredPath,
    scoping: &dyn Fn(&str) -> bool,
    out: &mut Vec<(AuthoredPath, String)>,
) {
    match value {
        Value::Object(map) => {
            if let Some((key, arg)) = map.iter().next().filter(|_| map.len() == 1) {
                if let Some(path) = payload_read(key, arg) {
                    out.push((at.clone(), path));
                }
                if let Value::Array(args) = arg
                    && scoping(key)
                {
                    if let Some(first) = args.first() {
                        walk_payload(first, &at.join(key.as_str()).join(0), scoping, out);
                    }
                    return;
                }
            }
            for (key, child) in map {
                walk_payload(child, &at.join(key.as_str()), scoping, out);
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                walk_payload(item, &at.join(i), scoping, out);
            }
        }
        _ => {}
    }
}

/// The path a `{key: arg}` object reads, when it is a `var` or a `val` whose
/// literal path is rooted at `payload`.
///
/// A `var` path is dotted, and its first segment must be `payload`: `"payload"`,
/// `"payload.a"`, `["payload.a", default]`. A `val` path is a list of
/// segments, and its first must be `"payload"`: `"payload"`, `["payload",
/// "a"]`.
fn payload_read(key: &str, arg: &Value) -> Option<String> {
    let rooted = |path: &str| path.split('.').next() == Some("payload");
    match (key, arg) {
        ("var", Value::String(path)) if rooted(path) => Some(path.clone()),
        ("var", Value::Array(args)) => args
            .first()
            .and_then(Value::as_str)
            .filter(|path| rooted(path))
            .map(str::to_owned),
        ("val", Value::String(seg)) if seg == "payload" => Some(seg.clone()),
        ("val", Value::Array(segs)) if segs.first().and_then(Value::as_str) == Some("payload") => {
            let segs: Vec<String> = segs
                .iter()
                .map(|seg| match seg {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect();
            Some(segs.join("."))
        }
        _ => None,
    }
}

/// Whether a step's condition might gate what follows on a failure: it is
/// present and not the literal `true`.
fn gates(condition: Option<&Logic>) -> bool {
    condition.is_some_and(|c| c.0 != Value::Bool(true))
}

/// `UNGUARDED_VALIDATION`: a `validation` task whose rejection nothing after
/// it acts on, so the workflow goes on as if it had passed.
///
/// The chain is every task in [`WorkflowDef::walk`] order, setup first. A
/// validation is reported when it does not stop the workflow itself (it is
/// not `terminal`, and its `on_rejection` is not `halt`), some task follows
/// it, and no following task is guarded. A task is guarded by its condition,
/// by being a `filter`, or by the condition of a group opening at it: any
/// group the walk yields just before it, nested ones included.
///
/// An Advisory at `<task>.on_rejection`, the key that fixes it.
fn unguarded_validation(def: &WorkflowDef, issues: &mut Vec<Issue>) {
    struct Link<'d> {
        path: AuthoredPath,
        kind: StepKind,
        task: &'d TaskDef,
        guarded: bool,
    }
    let mut chain = Vec::new();
    // Whether a group with a gating condition opens at the next task.
    let mut opening = false;
    for (path, kind, step) in def.walk() {
        match step {
            StepDef::Group(group) => opening |= gates(group.condition.as_ref()),
            StepDef::Task(task) => {
                let guarded =
                    opening || gates(task.condition.as_ref()) || task.function.name == "filter";
                chain.push(Link {
                    path,
                    kind,
                    task,
                    guarded,
                });
                opening = false;
            }
        }
    }
    for (i, link) in chain.iter().enumerate() {
        let task = link.task;
        if task.function.name != "validation"
            || task.terminal
            || task.on_rejection == RejectionPolicy::Halt
        {
            continue;
        }
        let following = &chain[i + 1..];
        let Some(next) = following.first() else {
            continue;
        };
        if following.iter().any(|link| link.guarded) {
            continue;
        }
        let message = format!(
            "a failing rule here records status 400 and task `{}` still runs: \
             `on_rejection` defaults to `continue`. Add `\"on_rejection\": \"halt\"` to stop \
             the workflow, or gate what follows on `metadata.progress.status_code`",
            next.task.id
        );
        let issue = Issue::new(
            IssueCode::UnguardedValidation,
            link.path.join("on_rejection"),
            message,
        );
        issues.push(issue.with_step(link.kind, Some(&task.id)));
    }
}

/// A config's expressions, forwarded to a [`Hook`]. A config's visit reports
/// paths relative to the config; this joins each onto `root`, the task's
/// `function.input`, and names the field each sits in.
pub(crate) struct Below<'a> {
    pub(crate) hook: &'a mut dyn Hook,
    pub(crate) issues: &'a mut Vec<Issue>,
    pub(crate) root: &'a AuthoredPath,
    pub(crate) step: Stamp<'a>,
}

impl ExprVisitor for Below<'_> {
    fn expr(&mut self, at: &AuthoredPath, raw: &Value, records: bool) {
        let field = match at.segs().first() {
            Some(PathSeg::Key(key)) => Some(&**key),
            _ => None,
        };
        let at = self.root.extend(at);
        let site = Site {
            at: &at,
            records,
            step: self.step,
            field,
        };
        self.hook.expr(site, raw, self.issues);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn duplicates(raw: &Value) -> Vec<String> {
        duplicate_keys(raw, '$')
            .into_iter()
            .map(|(at, _)| at.to_string())
            .collect()
    }

    #[test]
    fn a_duplicate_is_the_later_key_emitting_a_name_already_emitted() {
        assert_eq!(duplicates(&json!({"$a": 1, "a": 2})), ["a"]);
        // `$$a` emits `$a`, and `$a` emits `a`: three names, no duplicate.
        assert!(duplicates(&json!({"$$a": 1, "$a": 2, "b": 3})).is_empty());
        // Each object on its own: two objects may emit one name each.
        assert_eq!(
            duplicates(&json!({"x": {"$b": 1, "b": 2}, "y": [{"$$c": 1, "$c": 2}], "b": 3})),
            ["x.b"]
        );
        // Every depth: an object's own keys, then the values below them.
        assert_eq!(
            duplicates(&json!({"$k": [{"$a": 1, "a": 2}], "k": {"$z": 0, "z": 1}})),
            ["k", "$k[0].a", "k.z"]
        );
    }

    fn reads(raw: &Value) -> Vec<(String, String)> {
        let scoping = |name: &str| name == "map";
        let mut out = Vec::new();
        walk_payload(raw, &AuthoredPath::root(), &scoping, &mut out);
        out.into_iter().map(|(at, p)| (at.to_string(), p)).collect()
    }

    #[test]
    fn a_payload_read_is_reported_at_its_object_with_the_path_it_reads() {
        let r = |at: &str, p: &str| (at.to_owned(), p.to_owned());
        assert_eq!(
            reads(&json!({"if": [{"var": ["payload.a", 1]}, {"val": ["payload", "b", 0]}, null]})),
            [r("if[0]", "payload.a"), r("if[1]", "payload.b.0")]
        );
        // A default is walked too.
        assert_eq!(
            reads(&json!({"var": ["data.a", {"var": "payload"}]})),
            [r("var[1]", "payload")]
        );
        // Only the first argument of a scoping operator reads the context.
        assert_eq!(
            reads(&json!({"map": [{"val": "payload"}, {"var": "payload.x"}]})),
            [r("map[0]", "payload")]
        );
        // A multi-key object is a template, not an operator.
        assert!(reads(&json!({"var": "payload", "x": 1})).is_empty());
    }
}
