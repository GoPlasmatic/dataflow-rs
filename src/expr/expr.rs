//! `Expr<T>`: compiled JSONLogic that resolves to a `T`.

use super::config::ValueKind;
use super::path::Path;
use super::walk::secret_refs;
use crate::base::{Error, ErrorKind};
use datalogic_rs::Logic;
use datavalue::{DataValue, NumberValue, OwnedDataValue};
use serde_json::Value;
use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

mod sealed {
    pub trait Sealed {}
    pub trait SealedExpr {}
}

/// A type an expression can resolve to.
///
/// Sealed: the implementations are [`OwnedDataValue`] (lent as
/// `Cow<OwnedDataValue>`), [`String`] (`Cow<str>`; a result that is not a
/// string is its compact JSON), [`u64`] and [`i64`] (themselves: an integer,
/// or a whole float, in the type's range; above `i64::MAX` an integer is read
/// as a float and may round; anything else is an error), [`f64`] (itself:
/// any finite number, the only kind JSON carries; anything else is an
/// error), [`bool`] (itself; anything else is an error) and [`Path`]
/// (`Cow<Path>`; a string parsed as a path).
pub trait FromValue: Sized + Send + Sync + 'static + sealed::Sealed {
    /// What kind of value the type is, for a field's schema.
    #[doc(hidden)]
    const KIND: ValueKind;

    /// What an evaluation hands back: a folded constant borrowed from the
    /// expression, a dynamic result owned.
    type Out<'a>;

    #[doc(hidden)]
    fn from_view(v: &DataValue<'_>) -> Result<Self, String>;
    #[doc(hidden)]
    fn lend(c: &Self) -> Self::Out<'_>;
    #[doc(hidden)]
    fn give<'a>(v: Self) -> Self::Out<'a>;
    /// A folded constant as an owned value, for a resolved config.
    #[doc(hidden)]
    fn to_value(c: &Self) -> OwnedDataValue;
    /// An evaluated value as an owned one, for a resolved config.
    #[doc(hidden)]
    fn into_value(v: Self) -> OwnedDataValue;
    /// The type's name, for messages.
    #[doc(hidden)]
    fn type_name() -> &'static str;
}

impl sealed::Sealed for OwnedDataValue {}
impl sealed::Sealed for String {}
impl sealed::Sealed for u64 {}
impl sealed::Sealed for i64 {}
impl sealed::Sealed for f64 {}
impl sealed::Sealed for bool {}
impl sealed::Sealed for Path {}

impl FromValue for OwnedDataValue {
    const KIND: ValueKind = ValueKind::Any;
    type Out<'a> = Cow<'a, Self>;
    fn from_view(v: &DataValue<'_>) -> Result<Self, String> {
        Ok(v.to_owned())
    }
    fn lend(c: &Self) -> Self::Out<'_> {
        Cow::Borrowed(c)
    }
    fn give<'a>(v: Self) -> Self::Out<'a> {
        Cow::Owned(v)
    }
    fn to_value(c: &Self) -> OwnedDataValue {
        c.clone()
    }
    fn into_value(v: Self) -> OwnedDataValue {
        v
    }
    fn type_name() -> &'static str {
        "a value"
    }
}

impl FromValue for String {
    const KIND: ValueKind = ValueKind::String;
    type Out<'a> = Cow<'a, str>;
    /// A string's contents; anything else as compact JSON (v3's
    /// `resolve_string`).
    fn from_view(v: &DataValue<'_>) -> Result<Self, String> {
        Ok(match v {
            DataValue::String(s) => (*s).to_owned(),
            other => other.to_string(),
        })
    }
    fn lend(c: &Self) -> Self::Out<'_> {
        Cow::Borrowed(c)
    }
    fn give<'a>(v: Self) -> Self::Out<'a> {
        Cow::Owned(v)
    }
    fn to_value(c: &Self) -> OwnedDataValue {
        OwnedDataValue::String(c.clone())
    }
    fn into_value(v: Self) -> OwnedDataValue {
        OwnedDataValue::String(v)
    }
    fn type_name() -> &'static str {
        "a string"
    }
}

impl FromValue for u64 {
    const KIND: ValueKind = ValueKind::Number;
    type Out<'a> = Self;
    fn from_view(v: &DataValue<'_>) -> Result<Self, String> {
        match v {
            DataValue::Number(n) => n.as_u64().ok_or_else(|| format!("{v} is not a u64")),
            other => Err(format!("{other} is not a u64")),
        }
    }
    fn lend(c: &Self) -> Self::Out<'_> {
        *c
    }
    fn give<'a>(v: Self) -> Self::Out<'a> {
        v
    }
    fn to_value(c: &Self) -> OwnedDataValue {
        (*c).into()
    }
    fn into_value(v: Self) -> OwnedDataValue {
        v.into()
    }
    fn type_name() -> &'static str {
        "a u64"
    }
}

impl FromValue for i64 {
    const KIND: ValueKind = ValueKind::Number;
    type Out<'a> = Self;
    /// An integer in range, or a whole float that is one, as [`u64`]'s.
    fn from_view(v: &DataValue<'_>) -> Result<Self, String> {
        match v {
            DataValue::Number(n) => n.as_i64().ok_or_else(|| format!("{v} is not an i64")),
            other => Err(format!("{other} is not an i64")),
        }
    }
    fn lend(c: &Self) -> Self::Out<'_> {
        *c
    }
    fn give<'a>(v: Self) -> Self::Out<'a> {
        v
    }
    fn to_value(c: &Self) -> OwnedDataValue {
        (*c).into()
    }
    fn into_value(v: Self) -> OwnedDataValue {
        v.into()
    }
    fn type_name() -> &'static str {
        "an i64"
    }
}

impl FromValue for f64 {
    const KIND: ValueKind = ValueKind::Number;
    type Out<'a> = Self;
    /// Any finite number: an infinity, which arithmetic can make, has no
    /// JSON spelling and would be written as `null`.
    fn from_view(v: &DataValue<'_>) -> Result<Self, String> {
        match v {
            DataValue::Number(n) if n.as_f64().is_finite() => Ok(n.as_f64()),
            DataValue::Number(n) => Err(format!("{} is not a finite f64", n.as_f64())),
            other => Err(format!("{other} is not an f64")),
        }
    }
    fn lend(c: &Self) -> Self::Out<'_> {
        *c
    }
    fn give<'a>(v: Self) -> Self::Out<'a> {
        v
    }
    /// A float, as the JSON bridge builds one: `2.0` stays `2.0`.
    fn to_value(c: &Self) -> OwnedDataValue {
        OwnedDataValue::Number(NumberValue::Float(*c))
    }
    fn into_value(v: Self) -> OwnedDataValue {
        OwnedDataValue::Number(NumberValue::Float(v))
    }
    fn type_name() -> &'static str {
        "an f64"
    }
}

impl FromValue for bool {
    const KIND: ValueKind = ValueKind::Bool;
    type Out<'a> = Self;
    fn from_view(v: &DataValue<'_>) -> Result<Self, String> {
        match v {
            DataValue::Bool(b) => Ok(*b),
            other => Err(format!("{other} is not a bool")),
        }
    }
    fn lend(c: &Self) -> Self::Out<'_> {
        *c
    }
    fn give<'a>(v: Self) -> Self::Out<'a> {
        v
    }
    fn to_value(c: &Self) -> OwnedDataValue {
        (*c).into()
    }
    fn into_value(v: Self) -> OwnedDataValue {
        v.into()
    }
    fn type_name() -> &'static str {
        "a bool"
    }
}

impl FromValue for Path {
    const KIND: ValueKind = ValueKind::Path;
    type Out<'a> = Cow<'a, Self>;
    fn from_view(v: &DataValue<'_>) -> Result<Self, String> {
        match v {
            DataValue::String(s) => Self::from_str(s).map_err(|e| e.message().to_owned()),
            other => Err(format!("{other} is not a path")),
        }
    }
    fn lend(c: &Self) -> Self::Out<'_> {
        Cow::Borrowed(c)
    }
    fn give<'a>(v: Self) -> Self::Out<'a> {
        Cow::Owned(v)
    }
    fn to_value(c: &Self) -> OwnedDataValue {
        OwnedDataValue::String(c.to_string())
    }
    fn into_value(v: Self) -> OwnedDataValue {
        OwnedDataValue::String(v.to_string())
    }
    fn type_name() -> &'static str {
        "a path"
    }
}

/// What an evaluation returns: the value and what it cost.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Evaluated<T> {
    /// The result.
    pub value: T,
    /// Operations the evaluation spent: `Some` when the `budget` feature is on
    /// (zero for a folded constant), `None` otherwise.
    pub ops: Option<u64>,
}

/// Compiled JSONLogic resolving to a `T`, made by
/// [`ExprEngine::compile`](super::ExprEngine::compile).
///
/// An expression that reads nothing folds at compile into a constant, and
/// every evaluation lends that constant from the expression, never cloning it.
pub struct Expr<T: FromValue = OwnedDataValue> {
    pub(super) raw: Value,
    pub(super) logic: Logic,
    pub(super) constant: Option<T>,
}

impl<T: FromValue> Expr<T> {
    /// The folded constant, if the expression reads nothing.
    pub fn constant(&self) -> Option<&T> {
        self.constant.as_ref()
    }

    /// The authored JSON.
    pub fn raw(&self) -> &Value {
        &self.raw
    }

    /// The compiled logic, for an evaluation against the run view.
    pub(crate) fn logic(&self) -> &Logic {
        &self.logic
    }

    /// An evaluation's result as a `T`, or the [`ErrorKind::Eval`] error
    /// naming this expression. The reason quotes the value, so an expression
    /// that reads a secret gives only the type it expected: the authored
    /// expression names the secret, never its value.
    pub(crate) fn convert(&self, view: &DataValue<'_>) -> crate::Result<T> {
        T::from_view(view).map_err(|why| {
            let message = if secret_refs(&self.raw).next().is_some() {
                format!("`{}` did not give {}", self.raw, T::type_name())
            } else {
                format!("`{}` did not give {}: {why}", self.raw, T::type_name())
            };
            Error::new(ErrorKind::Eval, message)
        })
    }
}

impl<T: FromValue> fmt::Debug for Expr<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Expr")
            .field("raw", &self.raw)
            .field("folded", &self.constant.is_some())
            .finish()
    }
}

/// The object-safe face of an [`Expr`], which a
/// [`Resolver`](super::Resolver) evaluates: its logic, and its type's
/// conversion to an owned value (`u64`, `i64` and `f64` as numbers, `bool` as
/// a boolean, `String` as a string, [`Path`] as its dotted text, and
/// `OwnedDataValue` as itself).
///
/// Sealed: every `Expr<T>` is one, and nothing else is.
pub trait ResolveExpr: Send + Sync + sealed::SealedExpr {
    /// The authored JSON.
    fn raw(&self) -> &Value;

    /// The folded constant as an owned value, when the expression reads
    /// nothing.
    #[doc(hidden)]
    fn constant_value(&self) -> Option<OwnedDataValue>;

    /// The compiled logic.
    #[doc(hidden)]
    fn logic(&self) -> &Logic;

    /// An evaluation's result as the expression's type, then as an owned
    /// value: the [`ErrorKind::Eval`] error naming the expression when it is
    /// not of that type.
    #[doc(hidden)]
    fn value_of(&self, view: &DataValue<'_>) -> crate::Result<OwnedDataValue>;
}

impl<T: FromValue> sealed::SealedExpr for Expr<T> {}

impl<T: FromValue> ResolveExpr for Expr<T> {
    fn raw(&self) -> &Value {
        &self.raw
    }

    fn constant_value(&self) -> Option<OwnedDataValue> {
        self.constant.as_ref().map(T::to_value)
    }

    fn logic(&self) -> &Logic {
        &self.logic
    }

    fn value_of(&self, view: &DataValue<'_>) -> crate::Result<OwnedDataValue> {
        self.convert(view).map(T::into_value)
    }
}
