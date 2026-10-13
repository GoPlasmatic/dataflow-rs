//! Config fields: the types a function's `input` is made of, and the context
//! that compiles them with every issue at its own path.

use super::document::Interpolated;
use super::engine::ExprEngine;
use super::expr::{Expr, FromValue};
use crate::base::typo::unknown_key;
use crate::base::{AuthoredPath, ErrorKind, Issue, IssueCode, PathSeg, Severity, StepKind};
use datavalue::OwnedDataValue;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::BTreeMap;

/// What a field's value is, for a field's schema and for checking a literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValueKind {
    /// Any JSON.
    Any,
    /// A string.
    String,
    /// A number.
    Number,
    /// A boolean.
    Bool,
    /// An object.
    Object,
    /// An array.
    Array,
    /// A context path, rooted at `data`, `metadata` or `temp_data`.
    Path,
}

/// What a config field is, for tooling.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum FieldKind {
    /// A literal of the given kind, never evaluated.
    Literal(ValueKind),
    /// JSONLogic resolving to the given kind.
    Expr(ValueKind),
    /// A document with expression holes.
    Interpolated,
    /// A write destination.
    Output,
    /// A runtime-declared set of fields.
    Templated,
    /// An array of the element kind.
    List(fn() -> Self),
    /// An object whose values are of the given kind.
    Map(fn() -> Self),
    /// A nested config's fields.
    Nested(fn() -> &'static [FieldSpec]),
}

/// One field of a config.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FieldSpec {
    /// The authored key.
    pub name: Cow<'static, str>,
    /// What the field is.
    pub kind: FieldKind,
    /// Is the key required?
    pub required: bool,
    /// Does the engine record the field's result?
    pub records: bool,
    /// The default, as JSON text.
    pub default: Option<Cow<'static, str>>,
    /// Is this the primary output?
    pub primary: bool,
    /// The field's doc comment.
    pub description: Option<Cow<'static, str>>,
}

impl FieldSpec {
    /// A runtime field: its kind is an expression, an interpolated document, a
    /// literal or an output.
    pub fn runtime(name: impl Into<Cow<'static, str>>, kind: FieldKind, required: bool) -> Self {
        Self {
            name: name.into(),
            kind,
            required,
            records: false,
            default: None,
            primary: false,
            description: None,
        }
    }

    /// Sets the default, as JSON text.
    #[must_use]
    pub fn with_default(mut self, json: impl Into<Cow<'static, str>>) -> Self {
        self.default = Some(json.into());
        self
    }
}

/// Receives every expression of a config, with its path.
pub trait ExprVisitor {
    /// An expression at `at`; `records` is set when the engine records its
    /// result.
    fn expr(&mut self, _at: &AuthoredPath, _raw: &Value, _records: bool) {}

    /// An interpolated document at `at`.
    fn interpolated(&mut self, _at: &AuthoredPath, _doc: &Interpolated) {}
}

/// A walk over a compiled config's expressions.
pub struct Visit<'v> {
    visitor: &'v mut dyn ExprVisitor,
    path: AuthoredPath,
    records: bool,
}

impl<'v> Visit<'v> {
    /// A walk reporting to `visitor`, with paths below `root`.
    pub fn new(visitor: &'v mut dyn ExprVisitor, root: AuthoredPath) -> Self {
        Self {
            visitor,
            path: root,
            records: false,
        }
    }

    /// Runs `f` one segment deeper.
    pub fn at(&mut self, seg: impl Into<PathSeg>, f: impl FnOnce(&mut Self)) {
        self.path.push(seg);
        f(self);
        self.path.pop();
    }

    /// Runs `f` with the `records` flag ORed in, restoring it after.
    pub fn recording(&mut self, records: bool, f: impl FnOnce(&mut Self)) {
        let before = self.records;
        self.records |= records;
        f(self);
        self.records = before;
    }

    /// Reports an expression at the current path.
    pub fn expr(&mut self, raw: &Value) {
        self.visitor.expr(&self.path, raw, self.records);
    }

    /// Reports an interpolated document at the current path.
    pub fn interpolated(&mut self, doc: &Interpolated) {
        self.visitor.interpolated(&self.path, doc);
    }
}

/// Compiles config fields, recording each issue at its exact authored path.
pub struct CompileCx<'a> {
    exprs: &'a ExprEngine,
    issues: &'a mut Vec<Issue>,
    path: AuthoredPath,
    root_depth: usize,
    step: Option<(StepKind, String)>,
    runtime_fields: Option<&'a [FieldSpec]>,
    rejected: usize,
}

impl<'a> CompileCx<'a> {
    /// A context rooted at the document itself.
    pub fn new(exprs: &'a ExprEngine, issues: &'a mut Vec<Issue>) -> Self {
        Self {
            exprs,
            issues,
            path: AuthoredPath::root(),
            root_depth: 0,
            step: None,
            runtime_fields: None,
            rejected: 0,
        }
    }

    /// Roots the walk at `path`; a field is the first key below it.
    #[must_use]
    pub fn at_path(mut self, path: AuthoredPath) -> Self {
        self.root_depth = path.segs().len();
        self.path = path;
        self
    }

    /// Stamps the step on every issue.
    #[must_use]
    pub fn for_step(mut self, kind: StepKind, id: impl Into<String>) -> Self {
        self.step = Some((kind, id.into()));
        self
    }

    /// The fields a plugin declared at runtime.
    #[must_use]
    pub fn with_runtime_fields(mut self, fields: &'a [FieldSpec]) -> Self {
        self.runtime_fields = Some(fields);
        self
    }

    /// The engine expressions compile with.
    pub fn exprs(&self) -> &ExprEngine {
        self.exprs
    }

    /// The current path.
    pub fn path(&self) -> &AuthoredPath {
        &self.path
    }

    /// The runtime-declared fields, if any.
    pub fn runtime_fields(&self) -> Option<&'a [FieldSpec]> {
        self.runtime_fields
    }

    /// Records an issue at the current path, at the code's default severity.
    pub fn issue(&mut self, code: IssueCode, message: impl Into<String>) {
        self.issue_as(code, code.severity(), message);
    }

    /// Records an issue at the current path with an explicit severity.
    pub fn issue_as(&mut self, code: IssueCode, severity: Severity, message: impl Into<String>) {
        let issue = Issue::new(code, self.path.clone(), message).with_severity(severity);
        self.record(issue);
    }

    /// Records an issue found by another walk, such as a function's own
    /// checks, whose path is relative to the current path. The path is
    /// prefixed with the current one, the step and the field are stamped as
    /// [`Self::issue`] stamps them, and a Rejected issue is counted.
    pub(crate) fn adopt(&mut self, mut issue: Issue) {
        issue.path = self.path.extend(&issue.path);
        self.record(issue);
    }

    /// Stamps the step and the field on `issue`, counts it when it is
    /// Rejected, and records it.
    fn record(&mut self, mut issue: Issue) {
        if let Some((kind, id)) = &self.step {
            issue = issue.with_step(*kind, Some(id));
        }
        if let Some(PathSeg::Key(key)) = issue.path.segs().get(self.root_depth) {
            issue.field = Some(key.to_string());
        }
        if issue.severity == Severity::Rejected {
            self.rejected += 1;
        }
        self.issues.push(issue);
    }

    /// Runs `f` one segment deeper.
    pub fn at<R>(&mut self, seg: impl Into<PathSeg>, f: impl FnOnce(&mut Self) -> R) -> R {
        self.path.push(seg);
        let out = f(self);
        self.path.pop();
        out
    }

    /// How many Rejected issues this context has recorded.
    pub fn rejected(&self) -> usize {
        self.rejected
    }

    /// Records an unknown `key` at the current path (the caller is inside
    /// `at(key, …)`): a Rejected `LIKELY_TYPO` naming the near key, else a
    /// Rejected `UNKNOWN_KEY`.
    #[doc(hidden)]
    pub fn unknown_key(&mut self, key: &str, known: &[&str]) {
        let (code, message) = unknown_key(key, known);
        self.issue_as(code, Severity::Rejected, message);
    }
}

/// A type a config is made of.
///
/// An `Option<T>` field is optional: an absent key is `None`, and so is an
/// explicit `null`, unless `T` takes `null` as a value
/// ([`NULL_IS_VALUE`](Self::NULL_IS_VALUE)), as an [`Expr`], an
/// [`Interpolated`] document and any JSON value do. There the `null` is
/// compiled as `T`: `map`'s `"logic": null` is a constant `null` expression,
/// not a missing `logic`.
pub trait ConfigField: Sized + Send + Sync + 'static {
    /// Whether `null` is one of the type's values. When it is not, an explicit
    /// `null` in an `Option<Self>` field means `None`.
    const NULL_IS_VALUE: bool = false;

    /// Compiles `raw`; `None` after recording at least one issue.
    fn compile(raw: &Value, cx: &mut CompileCx<'_>) -> Option<Self>;

    /// The value when the key is absent and no default is declared. `None`
    /// makes the field required.
    fn absent() -> Option<Self> {
        None
    }

    /// Reports the field's expressions.
    fn visit(&self, _v: &mut Visit<'_>) {}

    /// What the field is.
    fn kind() -> FieldKind;
}

fn scalar<T: serde::de::DeserializeOwned>(
    raw: &Value,
    cx: &mut CompileCx<'_>,
    expected: &str,
) -> Option<T> {
    match serde_json::from_value::<T>(raw.clone()) {
        Ok(v) => Some(v),
        Err(_) => {
            cx.issue(IssueCode::InputParse, format!("expected {expected}"));
            None
        }
    }
}

macro_rules! scalar_field {
    ($($t:ty => $kind:ident, $expected:expr;)*) => {$(
        impl ConfigField for $t {
            fn compile(raw: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
                scalar(raw, cx, $expected)
            }
            fn kind() -> FieldKind {
                FieldKind::Literal(ValueKind::$kind)
            }
        }
    )*};
}

scalar_field! {
    bool => Bool, "a boolean";
    String => String, "a string";
    u8 => Number, "an unsigned integer that fits u8";
    u16 => Number, "an unsigned integer that fits u16";
    u32 => Number, "an unsigned integer that fits u32";
    u64 => Number, "an unsigned integer that fits u64";
    usize => Number, "an unsigned integer that fits usize";
    i32 => Number, "an integer that fits i32";
    i64 => Number, "an integer that fits i64";
    f64 => Number, "a number";
}

impl ConfigField for OwnedDataValue {
    const NULL_IS_VALUE: bool = true;
    fn compile(raw: &Value, _cx: &mut CompileCx<'_>) -> Option<Self> {
        Some(Self::from(raw))
    }
    fn kind() -> FieldKind {
        FieldKind::Literal(ValueKind::Any)
    }
}

impl ConfigField for Value {
    const NULL_IS_VALUE: bool = true;
    fn compile(raw: &Value, _cx: &mut CompileCx<'_>) -> Option<Self> {
        Some(raw.clone())
    }
    fn kind() -> FieldKind {
        FieldKind::Literal(ValueKind::Any)
    }
}

impl<T: FromValue> ConfigField for Expr<T> {
    /// `null` is a constant expression.
    const NULL_IS_VALUE: bool = true;
    fn compile(raw: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
        match cx.exprs().compile::<T>(raw) {
            Ok(expr) => Some(expr),
            Err(error) => {
                // `INVALID_PATH` is a literal string that is not a path; a
                // non-string where a path goes is a type error.
                let code = match error.kind() {
                    ErrorKind::Config if T::KIND == ValueKind::Path && raw.is_string() => {
                        IssueCode::InvalidPath
                    }
                    ErrorKind::Config => IssueCode::InputParse,
                    _ => IssueCode::TemplateCompile,
                };
                cx.issue(code, error.message().to_owned());
                None
            }
        }
    }
    fn visit(&self, v: &mut Visit<'_>) {
        v.expr(self.raw());
    }
    fn kind() -> FieldKind {
        FieldKind::Expr(T::KIND)
    }
}

impl<T: ConfigField> ConfigField for Option<T> {
    /// `null` is `None`, or `T`'s own `null`.
    const NULL_IS_VALUE: bool = true;
    fn compile(raw: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
        if raw.is_null() && !T::NULL_IS_VALUE {
            return Some(None);
        }
        T::compile(raw, cx).map(Some)
    }
    fn absent() -> Option<Self> {
        Some(None)
    }
    fn visit(&self, v: &mut Visit<'_>) {
        if let Some(inner) = self {
            inner.visit(v);
        }
    }
    fn kind() -> FieldKind {
        T::kind()
    }
}

impl<T: ConfigField> ConfigField for Vec<T> {
    fn compile(raw: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
        let Some(items) = raw.as_array() else {
            cx.issue(IssueCode::InputParse, "expected an array");
            return None;
        };
        // Every element is compiled, so every issue is reported.
        let compiled: Vec<Option<T>> = items
            .iter()
            .enumerate()
            .map(|(i, item)| cx.at(i, |cx| T::compile(item, cx)))
            .collect();
        compiled.into_iter().collect()
    }
    fn visit(&self, v: &mut Visit<'_>) {
        for (i, item) in self.iter().enumerate() {
            v.at(i, |v| item.visit(v));
        }
    }
    fn kind() -> FieldKind {
        FieldKind::List(T::kind)
    }
}

impl<T: ConfigField> ConfigField for BTreeMap<String, T> {
    fn compile(raw: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
        let Some(entries) = raw.as_object() else {
            cx.issue(IssueCode::InputParse, "expected an object");
            return None;
        };
        let compiled: Vec<Option<(String, T)>> = entries
            .iter()
            .map(|(key, value)| {
                cx.at(key.as_str(), |cx| T::compile(value, cx))
                    .map(|v| (key.clone(), v))
            })
            .collect();
        compiled.into_iter().collect()
    }
    fn visit(&self, v: &mut Visit<'_>) {
        for (key, item) in self {
            v.at(key.as_str(), |v| item.visit(v));
        }
    }
    fn kind() -> FieldKind {
        FieldKind::Map(T::kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopt_puts_an_issue_below_the_current_path_with_the_step_and_field() {
        let engine = ExprEngine::default();
        let mut issues = Vec::new();
        let rejected = {
            let mut cx = CompileCx::new(&engine, &mut issues)
                .at_path("tasks[1].function.input".parse().unwrap())
                .for_step(StepKind::Task, "fetch");
            assert_eq!(cx.rejected(), 0);
            cx.adopt(Issue::new(
                IssueCode::InputParse,
                "connector".parse().unwrap(),
                "no connector",
            ));
            // An Advisory is adopted and not counted.
            cx.adopt(Issue::new(
                IssueCode::UnknownKey,
                "retries".parse().unwrap(),
                "unknown",
            ));
            // The path is restored: the adopted issues did not move it.
            assert_eq!(cx.path().to_string(), "tasks[1].function.input");
            cx.rejected()
        };
        assert_eq!(rejected, 1);
        assert_eq!(issues.len(), 2);
        let issue = &issues[0];
        assert_eq!(issue.code, IssueCode::InputParse);
        assert_eq!(issue.severity, Severity::Rejected);
        assert_eq!(issue.path.to_string(), "tasks[1].function.input.connector");
        assert_eq!(issue.field.as_deref(), Some("connector"));
        assert_eq!(issue.step_kind, Some(StepKind::Task));
        assert_eq!(issue.step_id.as_deref(), Some("fetch"));
        assert_eq!(issue.message, "no connector");
        assert_eq!(issues[1].severity, Severity::Advisory);
        assert_eq!(issues[1].field.as_deref(), Some("retries"));
    }
}
