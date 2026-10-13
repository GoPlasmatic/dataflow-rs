//! Expressions: the JSONLogic surface a definition is written in.

pub(crate) mod arena;
mod config;
mod document;
mod engine;
#[expect(
    clippy::module_inception,
    reason = "the file holds `Expr`, as the design names it"
)]
mod expr;
mod operators;
mod output;
pub(crate) mod path;
mod secrets;
mod walk;

pub use config::{CompileCx, ConfigField, ExprVisitor, FieldKind, FieldSpec, ValueKind, Visit};
pub use document::Interpolated;
pub use engine::{ExprEngine, ExprEngineBuilder};
pub use expr::{Evaluated, Expr, FromValue};
pub use operators::{Family, OperatorInfo};
pub use output::Output;
pub(crate) use path::Seg;
pub use path::{Path, Root};
pub use secrets::Secrets;
pub use walk::{escaped_keys, secret_refs};
