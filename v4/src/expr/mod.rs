//! Expressions: the JSONLogic surface a definition is written in.

mod arena;
mod config;
mod engine;
#[expect(
    clippy::module_inception,
    reason = "the file holds `Expr`, as the design names it"
)]
mod expr;
mod operators;
mod path;
mod secrets;
mod walk;

pub use config::{CompileCx, ConfigField, ExprVisitor, FieldKind, FieldSpec, ValueKind, Visit};
pub use engine::{ExprEngine, ExprEngineBuilder};
pub use expr::{Evaluated, Expr, FromValue};
pub use operators::{Family, OperatorInfo};
pub use path::{Path, Root};
pub use secrets::Secrets;
pub use walk::{escaped_keys, secret_refs};
