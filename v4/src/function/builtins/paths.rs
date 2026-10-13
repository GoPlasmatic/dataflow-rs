//! The path fields parse and publish keep from v3 (I44, I56): a parse
//! source, which may name the payload; a path written relative to `data`; and
//! a publish source, which is one with v3's `data.` fallback.
//!
//! Each has one resolver, which reads a literal at compile and a computed
//! value at run time, so the two can never disagree. Each takes a string or
//! JSONLogic computing one: an expression that folds to anything but a string
//! is `INPUT_PARSE`, as an `Expr<Path>` and a runtime `Expr(String)` field
//! refuse it.

use crate::base::{IssueCode, Result};
use crate::expr::{CompileCx, ConfigField, Expr, FieldKind, Path, ValueKind, Visit};
use datavalue::OwnedDataValue;
use serde_json::Value;
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

/// Where `parse_json` and `parse_xml` read from (I44).
///
/// A literal string resolves at compile through `ParseSource::literal`:
/// `"payload"`, `"payload.<path>"`, or a context path. Anything else compiles
/// as JSONLogic computing that string, which resolves through the same
/// function at run time, so a computed source can name the payload too.
#[derive(Debug)]
#[non_exhaustive]
pub enum ParseSource {
    /// `"payload"`: the whole payload.
    Payload,
    /// `"payload.<path>"`: `<path>` split by [`Path`]'s segment rules and held
    /// under the `data` root, which stands for the payload.
    PayloadPath(Path),
    /// Any other literal: a context path.
    Context(Path),
    /// JSONLogic computing the source's name. One that folds keeps its
    /// expression, and its constant has been checked by
    /// `ParseSource::literal`.
    Computed(Expr<String>),
}

impl ParseSource {
    /// The one resolver: `"payload"`, `"payload.<path>"`, or a context path.
    ///
    /// # Errors
    ///
    /// A name that is none of those, such as `extra.x`, whose root is not
    /// `data`, `metadata` or `temp_data`.
    pub(crate) fn literal(s: &str) -> Result<Self> {
        if s == "payload" {
            return Ok(Self::Payload);
        }
        if let Some(rest) = s.strip_prefix("payload.") {
            return Path::from_str(&format!("data.{rest}")).map(Self::PayloadPath);
        }
        Path::from_str(s).map(Self::Context)
    }
}

impl ConfigField for ParseSource {
    fn compile(raw: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
        let (expr, literal) = compile_path(raw, cx, Self::literal)?;
        match literal {
            // A literal string is its resolved source; JSONLogic, folded or
            // not, keeps its expression for the walks that read it.
            Some(source) if raw.is_string() => Some(source),
            _ => Some(Self::Computed(expr)),
        }
    }

    /// Reports the authored JSON. A literal is rebuilt from its resolved
    /// form, which keeps the text it was parsed from.
    fn visit(&self, v: &mut Visit<'_>) {
        match self {
            Self::Payload => v.expr(&Value::from("payload")),
            Self::PayloadPath(path) => {
                let dotted = path.to_string();
                let rest = dotted.strip_prefix("data.").unwrap_or(&dotted);
                v.expr(&Value::from(format!("payload.{rest}")));
            }
            Self::Context(path) => v.expr(&Value::from(path.to_string())),
            Self::Computed(expr) => v.expr(expr.raw()),
        }
    }

    fn kind() -> FieldKind {
        FieldKind::Expr(ValueKind::String)
    }
}

/// A path written relative to `data` (I44): `"orders"` is `data.orders`, and
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
    pub fn constant(&self) -> Option<&Path> {
        self.constant.as_ref()
    }

    /// The expression computing the name below `data`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by the phase 4 runtime, which resolves it")
    )]
    pub(crate) fn expr(&self) -> &Expr<String> {
        &self.expr
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

    fn kind() -> FieldKind {
        FieldKind::Expr(ValueKind::String)
    }
}

/// A publish source (I56): rooted at `data`, as a [`DataPath`] is, plus v3's
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
    pub fn constant(&self) -> Option<&Path> {
        self.path.constant()
    }

    /// The fallback, when the source is literal (or folds) and begins with
    /// `data.`.
    pub fn fallback(&self) -> Option<&Path> {
        self.fallback.as_ref()
    }

    /// The expression computing the name below `data`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by the phase 4 runtime, which resolves it")
    )]
    pub(crate) fn expr(&self) -> &Expr<String> {
        self.path.expr()
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
        let fallback = s
            .strip_prefix("data.")
            .and_then(|rest| DataPath::resolve(rest).ok());
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

    fn kind() -> FieldKind {
        FieldKind::Expr(ValueKind::String)
    }
}
