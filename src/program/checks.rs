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
//! 3. **Payload.** `PAYLOAD_VAR` at the expression, for each path it reads
//!    from the root that is rooted at `payload`, which is not in the
//!    evaluation context. Which paths a rule reads, and whether each is a
//!    root read, is the compiled rule's own `facts()`: a scoping operator's
//!    per-element argument reads the element, not the root, and `missing`,
//!    `missing_some` and `exists` are covered too, without a second `var` /
//!    `val` parser here.
//!
//! The workflow lint is `UNGUARDED_VALIDATION`. A fragment has none.
//!
//! 4. **Fingerprint.** Each single-key object is classified by its key: an
//!    operator of the vocabulary, data, or (an escaped key) nothing. See
//!    [`Fingerprint`].

use super::fingerprint::Fingerprint;
use crate::base::{AuthoredPath, Issue, IssueCode, PathSeg, StepKind};
use crate::def::{RejectionPolicy, StepDef, TaskDef, WorkflowDef};
use crate::expr::{CRATE, ExprEngine, ExprVisitor, ResolveExpr, escaped_keys, secret_refs};
use crate::function::builtins::BuiltinKind;
use datavalue::OwnedDataValue;
use serde_json::Value;
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap, HashSet};

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
    fn expr(
        &mut self,
        site: Site<'_>,
        raw: &Value,
        compiled: Option<&dyn ResolveExpr>,
        issues: &mut Vec<Issue>,
    );

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
    /// Step condition paths that compiled and constant-folded to `true`.
    trivial_conds: HashSet<AuthoredPath>,
}

impl<'e> Checks<'e> {
    /// The checks against `exprs`.
    pub(crate) fn new(exprs: &'e ExprEngine) -> Self {
        Self {
            exprs,
            ops: BTreeSet::new(),
            data: BTreeSet::new(),
            trivial_conds: HashSet::new(),
        }
    }

    /// Rule 4: classify every single-key object below `raw`.
    fn classify(&mut self, raw: &Value) {
        match raw {
            Value::Object(map) => {
                if let Some(key) = map.keys().next().filter(|_| map.len() == 1) {
                    if self.exprs.operator(key).is_some() {
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
    fn secrets(
        &self,
        site: Site<'_>,
        raw: &Value,
        compiled: Option<&dyn ResolveExpr>,
        report: &mut Report<'_>,
    ) {
        if compiled.is_some_and(|expr| !expr.reads_secret(CRATE)) {
            return;
        }
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

    /// Rule 3: every path rooted at `payload` the expression reads at the
    /// root, at the expression, from the compiled rule's own `facts()`.
    fn payload_vars(
        &self,
        at: &AuthoredPath,
        raw: &Value,
        compiled: Option<&dyn ResolveExpr>,
        report: &mut Report<'_>,
    ) {
        if !matches!(raw, Value::Object(_) | Value::Array(_)) {
            return;
        }
        let owned;
        let logic = match compiled {
            Some(expr) => expr.logic(CRATE),
            None => {
                let Ok(compiled) = self.exprs.logic().compile(raw) else {
                    return;
                };
                owned = compiled;
                &owned
            }
        };
        for read in logic.facts().reads() {
            if read.segments().first().map(String::as_str) != Some("payload") {
                continue;
            }
            let path = read.to_string();
            let message = format!(
                "reads `{path}`, but the payload is not in the evaluation context, which \
                 holds only `data`, `metadata` and `temp_data`, so this always resolves to \
                 nothing; parse the payload into `data` first"
            );
            report(IssueCode::PayloadVar, at.clone(), message);
        }
    }
}

/// Where a rule reports: its code, absolute path and message.
type Report<'r> = dyn FnMut(IssueCode, AuthoredPath, String) + 'r;

impl Hook for Checks<'_> {
    fn expr(
        &mut self,
        site: Site<'_>,
        raw: &Value,
        compiled: Option<&dyn ResolveExpr>,
        issues: &mut Vec<Issue>,
    ) {
        if !site.records
            && site.field.is_none()
            && site.step.is_some()
            && compiled.is_some_and(|e| e.constant_value(CRATE) == Some(OwnedDataValue::Bool(true)))
        {
            self.trivial_conds.insert(site.at.clone());
        }
        let mut report = |code, at, message: String| {
            issues.push(site.stamp(Issue::new(code, at, message)));
        };
        self.secrets(site, raw, compiled, &mut report);
        self.template_keys(site.at, raw, &mut report);
        self.payload_vars(site.at, raw, compiled, &mut report);
        self.classify(raw);
    }

    fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of(&self.ops, &self.data)
    }

    fn workflow(&mut self, def: &WorkflowDef, issues: &mut Vec<Issue>) {
        unguarded_validation(&self.trivial_conds, def, issues);
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
    let mut at = AuthoredPath::root();
    walk_duplicates(raw, &mut at, escape, &mut out);
    out
}

fn walk_duplicates(
    value: &Value,
    at: &mut AuthoredPath,
    escape: char,
    out: &mut Vec<(AuthoredPath, String)>,
) {
    match value {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                at.push(i);
                walk_duplicates(item, at, escape, out);
                at.pop();
            }
        }
        Value::Object(map) => {
            if map.len() > 1 {
                let mut emitted: HashMap<&str, &str> = HashMap::with_capacity(map.len());
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
            }
            for (key, child) in map {
                at.push(key.as_str());
                walk_duplicates(child, at, escape, out);
                at.pop();
            }
        }
        _ => {}
    }
}

/// Whether a step's condition at `step_path` might gate what follows on a
/// failure: it is present and did not constant-fold to `true` during the
/// compile walk.
fn gates(trivial_conds: &HashSet<AuthoredPath>, has_cond: bool, step_path: &AuthoredPath) -> bool {
    has_cond && (trivial_conds.is_empty() || !trivial_conds.contains(&step_path.join("condition")))
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
fn unguarded_validation(
    trivial_conds: &HashSet<AuthoredPath>,
    def: &WorkflowDef,
    issues: &mut Vec<Issue>,
) {
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
            StepDef::Group(group) => {
                opening |= gates(trivial_conds, group.condition.is_some(), &path);
            }
            StepDef::Task(task) => {
                let guarded = opening
                    || gates(trivial_conds, task.condition.is_some(), &path)
                    || BuiltinKind::from_name(&task.function.name) == Some(BuiltinKind::Filter);
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
        if BuiltinKind::from_name(&task.function.name) != Some(BuiltinKind::Validation)
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
             `on_rejection` is `continue`. Add `\"on_rejection\": \"halt\"` to stop \
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
        self.hook.expr(site, raw, None, self.issues);
    }

    fn compiled_expr(&mut self, at: &AuthoredPath, expr: &dyn ResolveExpr, records: bool) {
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
        self.hook.expr(site, expr.raw(), Some(expr), self.issues);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Operator;
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

    /// A host's scoping operator, for `host_each`.
    struct Each;

    impl datalogic_rs::CustomOperator for Each {
        fn evaluate<'a>(
            &self,
            _args: &[&'a datalogic_rs::DataValue<'a>],
            _ctx: &mut datalogic_rs::operator::EvalContext<'_, 'a>,
            arena: &'a datalogic_rs::bumpalo::Bump,
        ) -> datalogic_rs::Result<&'a datalogic_rs::DataValue<'a>> {
            Ok(arena.alloc(datalogic_rs::DataValue::Null))
        }
    }

    /// The paths `raw` reads from the root that are rooted at `payload`, as
    /// the compiled rule's own facts report them: `all_operators`, and a
    /// host's `host_each`.
    fn payload_reads(raw: &Value) -> Vec<String> {
        let exprs = ExprEngine::builder()
            .all_operators()
            .operator_with(Operator::custom("host_each").scoping(true), Each)
            .build()
            .unwrap();
        let logic = exprs.logic().compile(raw).expect("the raw compiles");
        logic
            .facts()
            .reads()
            .iter()
            .filter(|read| read.segments().first().map(String::as_str) == Some("payload"))
            .map(ToString::to_string)
            .collect()
    }

    /// The lint's answer is the rule's own root reads, so `if`, `var` (a
    /// default included) and `val` are all covered, and a data read or a
    /// template object reports nothing.
    #[test]
    fn the_payload_reads_are_the_compiled_rules_root_reads() {
        assert_eq!(
            payload_reads(
                &json!({"if": [{"var": ["payload.a", 1]}, {"val": ["payload", "b", 0]}, null]})
            ),
            ["payload.a", "payload.b.0"]
        );
        assert_eq!(
            payload_reads(&json!({"var": ["data.a", {"var": "payload"}]})),
            ["payload"]
        );
        assert!(payload_reads(&json!({"var": "data.payload"})).is_empty());
        // A multi-key object is a template, not an operator.
        assert!(payload_reads(&json!({"var": "payload", "x": 1})).is_empty());
    }

    /// A scoping operator's per-element argument reads the element, not the
    /// root, so its `payload` read is not reported; the source is.
    #[test]
    fn a_payload_read_inside_a_scope_is_not_a_root_read() {
        assert_eq!(
            payload_reads(&json!({"map": [{"val": "payload"}, {"var": "payload.x"}]})),
            ["payload"]
        );
        // A host's operator declares no positions, so each argument is a root
        // read here.
        let p = || json!({"var": "payload"});
        assert_eq!(
            payload_reads(&json!({"host_each": [p(), p(), p()]})),
            ["payload"]
        );
    }

    #[test]
    fn a_payload_read_in_distincts_key_is_not_a_root_read() {
        assert!(
            payload_reads(&json!({"distinct": [{"var": "data.items"}, {"var": "payload.k"}]}))
                .is_empty()
        );
        assert_eq!(
            payload_reads(&json!({"distinct": [{"var": "payload.items"}, {"var": "payload.k"}]})),
            ["payload.items"]
        );
    }

    #[test]
    fn a_payload_read_in_trys_catch_arm_is_not_a_root_read() {
        assert_eq!(
            payload_reads(&json!({"try": [{"var": "payload.a"}, {"var": "payload.b"}]})),
            ["payload.a"]
        );
    }

    /// The gap the raw walk left: `missing`, `missing_some` and `exists` read
    /// their paths too, and the lint reports them now.
    #[test]
    fn a_payload_read_through_missing_is_a_root_read() {
        assert_eq!(
            payload_reads(&json!({"missing": "payload.x"})),
            ["payload.x"]
        );
        assert_eq!(
            payload_reads(&json!({"missing_some": [1, ["payload.x", "data.y"]]})),
            ["payload.x"]
        );
    }

    #[test]
    fn a_payload_read_through_exists_is_a_root_read() {
        // `exists` takes segment names, like `val`: `"payload"` or
        // `["payload", "x"]`. `"payload.x"` is one key with a dot in it.
        assert_eq!(payload_reads(&json!({"exists": "payload"})), ["payload"]);
        assert_eq!(
            payload_reads(&json!({"exists": ["payload", "x"]})),
            ["payload.x"]
        );
        assert!(payload_reads(&json!({"exists": "payload.x"})).is_empty());
        assert_eq!(
            payload_reads(&json!({"and": [{"var": "payload.a"}, {"exists": ["payload", "b"]}]})),
            ["payload.a", "payload.b"]
        );
    }
}
