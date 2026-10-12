//! Write destinations.

use super::config::{CompileCx, ConfigField, FieldKind, Visit};
use super::expr::Expr;
use super::path::Path;
use serde_json::Value;

/// A write destination: an `Expr<Path>`, folded when literal.
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
    fn visit(&self, v: &mut Visit<'_>) {
        v.expr(self.0.raw());
    }
    fn kind() -> FieldKind {
        FieldKind::Output
    }
}
