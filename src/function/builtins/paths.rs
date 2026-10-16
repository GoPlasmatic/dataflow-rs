//! The path fields parse and publish keep from v3: a parse
//! source, which may name the payload; a path written relative to `data`; and
//! a publish source, which is one with v3's `data.` fallback.
//!
//! Each has one resolver, which reads a literal at compile and a computed
//! value at run time, so the two can never disagree. Each takes a string or
//! JSONLogic computing one: an expression that folds to anything but a string
//! is `INPUT_PARSE`, as an `Expr<Path>` and a runtime `Expr(String)` field
//! refuse it.

use crate::base::{IssueCode, Result};
use crate::expr::{CompileCx, ConfigField, Expr, FieldKind, Path, Resolver, ValueKind, Visit};
use crate::function::SyncCtx;
use datavalue::OwnedDataValue;
use serde_json::Value;
use std::borrow::Cow;
use std::str::FromStr;

/// Compiles a path field: its expression and, when the expression folds, its
/// constant through `resolve`.
///
/// The constant must be a string. One that is not, or that `resolve` refuses,
/// is reported as `Expr<Path>` reports it: `INVALID_PATH` for a literal
/// string, `INPUT_PARSE` for anything else.
fn compile_path<T>(
    raw: &Value,
    cx: &mut CompileCx<'_>,
    resolve: fn(&str) -> Result<T>,
) -> Option<(Expr<String>, Option<T>)> {
    let expr = <Expr<String>>::compile(raw, cx)?;
    let Some(text) = expr.constant() else {
        return Some((expr, None));
    };
    // `Expr<String>` lends a folded non-string as its JSON text, so `7` would
    // read as the name "7". A literal string is a string; anything else that
    // folded is compiled again to see what it folded to.
    if !raw.is_string() && !folds_to_string(raw, cx) {
        cx.issue(
            IssueCode::InputParse,
            format!("`{raw}` must be a path string, or JSONLogic computing one"),
        );
        return None;
    }
    match resolve(text) {
        Ok(resolved) => Some((expr, Some(resolved))),
        Err(error) => {
            let code = if raw.is_string() {
                IssueCode::InvalidPath
            } else {
                IssueCode::InputParse
            };
            cx.issue(code, error.message().to_owned());
            None
        }
    }
}

/// Whether `raw`, which compiled and folded, folded to a string.
fn folds_to_string(raw: &Value, cx: &CompileCx<'_>) -> bool {
    cx.exprs()
        .compile::<OwnedDataValue>(raw)
        .is_ok_and(|expr| matches!(expr.constant(), Some(OwnedDataValue::String(_))))
}

/// Where a parse reads from, resolved: the one form the runtime reads.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum ParseFrom {
    /// `"payload"`: the whole payload.
    Payload,
    /// `"payload.<path>"`: `<path>` split by [`Path`]'s segment rules and held
    /// under the `data` root, which stands for the payload.
    PayloadPath(Path),
    /// Any other name: a context path.
    Context(Path),
}

impl ParseFrom {
    /// The one resolver: `"payload"`, `"payload.<path>"`, or a context path.
    ///
    /// # Errors
    ///
    /// A name that is none of those, such as `extra.x`, whose root is not
    /// `data`, `metadata` or `temp_data`.
    pub(crate) fn parse(s: &str) -> Result<Self> {
        // v3's empty remainder names the root it follows: `get_nested_value(x, "")` is `x`.
        if s == "payload" || s == "payload." {
            return Ok(Self::Payload);
        }
        if s == "data." {
            return Path::from_str("data").map(Self::Context);
        }
        if let Some(rest) = s.strip_prefix("payload.") {
            return Path::from_str(&format!("data.{rest}")).map(Self::PayloadPath);
        }
        Path::from_str(s).map(Self::Context)
    }

    /// The name it was parsed from, as a message names the source.
    pub(crate) fn text(&self) -> String {
        match self {
            Self::Payload => "payload".to_owned(),
            Self::PayloadPath(path) => {
                let dotted = path.dotted();
                format!("payload.{}", dotted.strip_prefix("data.").unwrap_or(dotted))
            }
            Self::Context(path) => path.dotted().to_string(),
        }
    }
}

/// Where `parse_json` and `parse_xml` read from.
///
/// A literal string resolves at compile through `ParseFrom::parse`, and so
/// does JSONLogic that folds, so a constant is not parsed per call. Anything
/// else compiles as JSONLogic computing that string, which resolves through
/// the same function at run time, so a computed source can name the payload
/// too.
#[derive(Debug)]
#[non_exhaustive]
pub enum ParseSource {
    /// A literal string, resolved at compile. Its `visit` rebuilds the text.
    Literal(ParseFrom),
    /// JSONLogic that folded, resolved at compile and never again. It keeps
    /// the authored JSON, which `visit` reports.
    Folded {
        /// The authored JSONLogic.
        raw: Value,
        /// What it folded to.
        from: ParseFrom,
    },
    /// JSONLogic computing the source's name, resolved per call.
    Computed(Expr<String>),
}

impl ParseSource {
    /// The one resolver for a literal and a computed value.
    ///
    /// # Errors
    ///
    /// The evaluation's, or a computed name that is no source.
    pub(crate) fn resolve<'p>(
        &'p self,
        cx: &mut SyncCtx<'_, '_, 'p>,
    ) -> Result<Cow<'p, ParseFrom>> {
        match self {
            Self::Literal(from) | Self::Folded { from, .. } => Ok(Cow::Borrowed(from)),
            Self::Computed(expr) => ParseFrom::parse(&cx.eval(expr)?).map(Cow::Owned),
        }
    }
}

impl ConfigField for ParseSource {
    fn compile(raw: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
        let (expr, resolved) = compile_path(raw, cx, ParseFrom::parse)?;
        Some(match resolved {
            Some(from) if raw.is_string() => Self::Literal(from),
            Some(from) => Self::Folded {
                raw: raw.clone(),
                from,
            },
            None => Self::Computed(expr),
        })
    }

    /// Reports the authored JSON. A literal is rebuilt from its resolved
    /// form, which keeps the text it was parsed from.
    fn visit(&self, v: &mut Visit<'_>) {
        match self {
            Self::Literal(from) => v.expr(&Value::from(from.text())),
            Self::Folded { raw, .. } => v.expr(raw),
            Self::Computed(expr) => v.expr(expr.raw()),
        }
    }

    /// The source's name: the one resolved at compile, as its text, or the
    /// one computed for this call.
    fn resolve(&self, r: &mut dyn Resolver) -> Result<Option<OwnedDataValue>> {
        match self {
            Self::Literal(from) | Self::Folded { from, .. } => {
                Ok(Some(OwnedDataValue::String(from.text())))
            }
            Self::Computed(expr) => ConfigField::resolve(expr, r),
        }
    }

    fn kind() -> FieldKind {
        FieldKind::Expr(ValueKind::String)
    }
}

/// A path written relative to `data`: `"orders"` is `data.orders`, and
/// `"data.t"` is `data.data.t`, as v3's `target` always was.
///
/// A literal resolves at compile; JSONLogic that does not fold resolves
/// through `DataPath::resolve` at run time.
#[derive(Debug)]
pub struct DataPath {
    expr: Expr<String>,
    constant: Option<Path>,
}

impl DataPath {
    /// The path, when it is literal or folds, resolved at compile.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the runtime reads the field; the tests read this")
    )]
    pub fn constant(&self) -> Option<&Path> {
        self.constant.as_ref()
    }

    /// The expression computing the name below `data`.
    #[cfg(test)]
    pub(crate) fn expr(&self) -> &Expr<String> {
        &self.expr
    }

    /// The path for this call: the one resolved at compile, or the computed
    /// name through [`Self::resolve`].
    ///
    /// # Errors
    ///
    /// The evaluation's.
    pub(crate) fn path<'p>(&'p self, cx: &mut SyncCtx<'_, '_, 'p>) -> Result<Cow<'p, Path>> {
        match &self.constant {
            Some(path) => Ok(Cow::Borrowed(path)),
            None => Self::resolve(&cx.eval(&self.expr)?).map(Cow::Owned),
        }
    }

    /// The one resolver, for a literal at compile and a computed value at run
    /// time: `data.<s>`.
    ///
    /// # Errors
    ///
    /// `data.<s>` that is not a [`Path`]. Every name under `data` is one, as
    /// every name was in v3, so none is refused today.
    pub(crate) fn resolve(s: &str) -> Result<Path> {
        Path::from_str(&format!("data.{s}"))
    }
}

impl ConfigField for DataPath {
    fn compile(raw: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
        let (expr, constant) = compile_path(raw, cx, Self::resolve)?;
        Some(Self { expr, constant })
    }

    fn visit(&self, v: &mut Visit<'_>) {
        v.expr(self.expr.raw());
    }

    /// The name below `data`, as written or as computed for this call.
    fn resolve(&self, r: &mut dyn Resolver) -> Result<Option<OwnedDataValue>> {
        ConfigField::resolve(&self.expr, r)
    }

    fn kind() -> FieldKind {
        FieldKind::Expr(ValueKind::String)
    }
}

/// A publish source: rooted at `data`, as a [`DataPath`] is, plus v3's
/// fallback. A source beginning with `data.` also names the source without
/// that prefix, read when `data.<source>` is missing: `"data.obj"` reads
/// `data.data.obj`, else `data.obj`.
///
/// A literal resolves at compile; JSONLogic that does not fold resolves
/// through `PublishSource::resolve` at run time.
#[derive(Debug)]
pub struct PublishSource {
    path: DataPath,
    fallback: Option<Path>,
}

impl PublishSource {
    /// The primary path, when it is literal or folds, resolved at compile.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the runtime reads the field; the tests read this")
    )]
    pub fn constant(&self) -> Option<&Path> {
        self.path.constant()
    }

    /// The fallback, when the source is literal (or folds) and begins with
    /// `data.`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the runtime reads the field; the tests read this")
    )]
    pub fn fallback(&self) -> Option<&Path> {
        self.fallback.as_ref()
    }

    /// The expression computing the name below `data`.
    #[cfg(test)]
    pub(crate) fn expr(&self) -> &Expr<String> {
        self.path.expr()
    }

    /// The primary path and the fallback for this call: the ones resolved at
    /// compile, or the computed name through [`Self::resolve`].
    ///
    /// # Errors
    ///
    /// The evaluation's.
    pub(crate) fn paths<'p>(
        &'p self,
        cx: &mut SyncCtx<'_, '_, 'p>,
    ) -> Result<(Cow<'p, Path>, Option<Cow<'p, Path>>)> {
        match &self.path.constant {
            Some(path) => Ok((
                Cow::Borrowed(path),
                self.fallback.as_ref().map(Cow::Borrowed),
            )),
            None => {
                let (path, fallback) = Self::resolve(&cx.eval(&self.path.expr)?)?;
                Ok((Cow::Owned(path), fallback.map(Cow::Owned)))
            }
        }
    }

    /// The one resolver, for a literal at compile and a computed value at run
    /// time: the primary path, `DataPath::resolve(s)`, and the fallback when
    /// `s` begins with `data.`.
    ///
    /// The fallback is the rest of `s` read as a [`DataPath`] is, which is
    /// where v3's lookup of the stripped name read: `data.<rest>`. A rest that
    /// is not a path gives no fallback and no error, as v3's lookup simply
    /// missed; none is refused today.
    ///
    /// # Errors
    ///
    /// The primary's, as `DataPath::resolve`'s.
    pub(crate) fn resolve(s: &str) -> Result<(Path, Option<Path>)> {
        let primary = DataPath::resolve(s)?;
        // v3's empty remainder is `data` itself (`get_nested_value(data, "")`),
        // so a source of `""` or `"data."` falls back to the whole of `data`.
        let root = || Path::from_str("data").ok();
        let fallback = match s.strip_prefix("data.") {
            _ if s.is_empty() => root(),
            Some("") => root(),
            Some(rest) => DataPath::resolve(rest).ok(),
            None => None,
        };
        Ok((primary, fallback))
    }
}

impl ConfigField for PublishSource {
    fn compile(raw: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
        let (expr, resolved) = compile_path(raw, cx, Self::resolve)?;
        let (constant, fallback) =
            resolved.map_or((None, None), |(path, fallback)| (Some(path), fallback));
        Some(Self {
            path: DataPath { expr, constant },
            fallback,
        })
    }

    fn visit(&self, v: &mut Visit<'_>) {
        self.path.visit(v);
    }

    /// The source's name, as a [`DataPath`]'s.
    fn resolve(&self, r: &mut dyn Resolver) -> Result<Option<OwnedDataValue>> {
        ConfigField::resolve(&self.path, r)
    }

    fn kind() -> FieldKind {
        FieldKind::Expr(ValueKind::String)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::ExprEngine;
    use serde_json::json;

    /// Compiles `raw` as one field `T`, with no issue.
    fn field<T: ConfigField>(raw: &Value) -> T {
        let engine = ExprEngine::default();
        let mut issues = Vec::new();
        let value = {
            let mut cx = CompileCx::new(&engine, &mut issues);
            T::compile(raw, &mut cx)
        };
        assert!(issues.is_empty(), "{raw}: {issues:?}");
        value.expect("compiles")
    }

    /// What `visit` reports for `field`.
    fn reported(field: &impl ConfigField) -> Vec<Value> {
        struct Seen(Vec<Value>);
        impl crate::expr::ExprVisitor for Seen {
            fn expr(&mut self, _: &crate::base::AuthoredPath, raw: &Value, _: bool) {
                self.0.push(raw.clone());
            }
        }
        let mut seen = Seen(Vec::new());
        field.visit(&mut Visit::new(
            &mut seen,
            crate::base::AuthoredPath::root(),
        ));
        seen.0
    }

    #[test]
    fn a_folded_computed_path_resolves_at_compile() {
        // JSONLogic that folds; none of it reads the message.
        let source = json!({"cat": ["payload.", "x.y"]});
        let parse = field::<ParseSource>(&source);
        assert!(
            matches!(
                &parse,
                ParseSource::Folded { raw, from: ParseFrom::PayloadPath(p) }
                    if raw == &source && p.to_string() == "data.x.y"
            ),
            "{parse:?}"
        );
        assert_eq!(reported(&parse), [source]);

        let target = json!({"cat": ["or", "ders"]});
        let data = field::<DataPath>(&target);
        assert_eq!(
            data.constant().map(ToString::to_string).as_deref(),
            Some("data.orders")
        );
        assert_eq!(reported(&data), [target]);

        let origin = json!({"cat": ["data.", "obj"]});
        let publish = field::<PublishSource>(&origin);
        assert_eq!(
            publish.constant().map(ToString::to_string).as_deref(),
            Some("data.data.obj")
        );
        assert_eq!(
            publish.fallback().map(ToString::to_string).as_deref(),
            Some("data.obj")
        );
        assert_eq!(reported(&publish), [origin]);

        // One that reads the message is not folded, and stays computed.
        let computed = json!({"cat": ["payload.", {"var": "data.k"}]});
        let parse = field::<ParseSource>(&computed);
        assert!(matches!(parse, ParseSource::Computed(_)), "{parse:?}");
        assert_eq!(reported(&parse), std::slice::from_ref(&computed));
        assert!(field::<DataPath>(&computed).constant().is_none());
        assert!(field::<PublishSource>(&computed).constant().is_none());
    }
}
