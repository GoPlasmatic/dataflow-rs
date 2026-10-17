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

pub(crate) use config::sealed::Sealed as SealedResolver;
pub use config::{
    CompileCx, ConfigField, ExprVisitor, FieldKind, FieldSpec, Resolver, ValueKind, Visit,
};
pub(crate) use document::Filled;
pub use document::{Interpolated, Rendered};
pub use engine::{EvalRoot, ExprEngine, ExprEngineBuilder};
pub use expr::{Evaluated, Expr, FromValue, ResolveExpr};
pub use operators::{Family, OperatorInfo};
pub use output::Output;
pub(crate) use path::Seg;
pub use path::{MAX_ARRAY_INDEX, Path, Root};
pub use secrets::Secrets;
pub(crate) use walk::reads_secret;
pub use walk::{escaped_keys, secret_refs};
