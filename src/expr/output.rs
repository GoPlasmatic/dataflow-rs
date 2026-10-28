//! Write destinations.

use super::config::{CompileCx, ConfigField, FieldKind, OutputOverride, Resolver, Visit};
use super::expr::Expr;
use super::path::Path;
use crate::base::Result;
use datavalue::OwnedDataValue;
use serde_json::Value;

/// A write destination: an `Expr<Path>`, folded when literal.
///
/// The engine records every destination it writes through, in each
/// `Change`'s path, so a config's visit reports an `Output` as recorded
/// whether or not its field says `records`.
#[derive(Debug)]
pub struct Output(Expr<Path>);

impl Output {
    /// The destination expression.
    pub fn path(&self) -> &Expr<Path> {
        &self.0
    }

    /// The destination when it is a literal path, resolved at compile time.
    pub fn constant(&self) -> Option<&Path> {
        self.0.constant()
    }
}

impl ConfigField for Output {
    const IS_OUTPUT: bool = true;
    fn compile(raw: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
        <Expr<Path>>::compile(raw, cx).map(Self)
    }
    /// Visited as recorded, wherever it sits: a destination lands in every
    /// `Change` written through it, so it may not read a secret.
    fn visit(&self, v: &mut Visit<'_>) {
        v.recording(true, |v| v.expr(self.0.raw()));
    }
    /// The destination's dotted text, computed for this call when it is not
    /// literal.
    ///
    /// A resolver that supplies the callee's primary output answers before
    /// the expression is evaluated, so a computed destination that yields
    /// `null` need not fail an input a layer only reads
    /// ([`Ctx::input_with_primary_output`](crate::Ctx::input_with_primary_output)).
    fn resolve(&self, r: &mut dyn Resolver) -> Result<Option<OwnedDataValue>> {
        if let Some(supplied) = r.output_override(self) {
            return Ok(match supplied {
                OutputOverride::Omit => None,
                OutputOverride::Value(value) => Some(value),
            });
        }
        r.eval(&self.0).map(Some)
    }
    fn kind() -> FieldKind {
        FieldKind::Output
    }
}
