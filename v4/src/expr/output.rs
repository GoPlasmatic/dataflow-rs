//! Write destinations.

use super::config::{CompileCx, ConfigField, FieldKind, Visit};
use super::expr::Expr;
use super::path::Path;
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
    fn compile(raw: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
        <Expr<Path>>::compile(raw, cx).map(Self)
    }
    /// Visited as recorded, wherever it sits: a destination lands in every
    /// `Change` written through it, so it may not read a secret (break 16).
    fn visit(&self, v: &mut Visit<'_>) {
        v.recording(true, |v| v.expr(self.0.raw()));
    }
    fn kind() -> FieldKind {
        FieldKind::Output
    }
}
