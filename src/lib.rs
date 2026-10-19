//! dataflow-rs 4, under construction on `main`.
//!
//! # Features
//!
//! None is on by default. The six standard extension operator families
//! (`ExtString`, `ExtArray`, `ExtMath`, `ExtControl`, `ExtObject` and
//! `ErrorHandling`) are compiled in unconditionally and selected at runtime on
//! [`expr::ExprEngineBuilder`], which defaults to [`expr::OperatorFamily::Core`]
//! (and `secret`) only.
//!
//! - `datetime`: compiles in the [`expr::OperatorFamily::Datetime`] operators
//!   (still selected at runtime on [`expr::ExprEngineBuilder`]) and changes
//!   `==` and the ordering operators on plain date-shaped strings across the
//!   build.
//! - `tensor`: the `Tensor` value and [`expr::OperatorFamily::Tensor`]
//!   operators (selected at runtime on [`expr::ExprEngineBuilder`]). Not in
//!   [`expr::ExprEngineBuilder::all_operators`], since names like `shape`,
//!   `full`, `cast` and `concat` are ordinary JSON keys.
//! - `budget`: a ceiling on the operations of one evaluation, which
//!   `ExprEngineBuilder::ops_budget` sets.
//! - `wasm-web`: the browser's clock and randomness, which
//!   `wasm32-unknown-unknown` needs and native targets must not pull in.
//! - `ts`: derives `ts_rs::TS` on the wire shapes, from which
//!   `tests/ts_types.rs` renders `bindings/dataflow.d.ts`, which ships in the
//!   package. **`ts` is outside semver:** the wire shapes are covered by
//!   semver, but the generator, its output's layout and this feature's
//!   dependencies may change in any release.
//!
//! # Dependencies in the API
//!
//! [`datalogic_rs`] and [`datavalue`] are re-exported, and part of this
//! crate's API: a value is a [`datavalue::OwnedDataValue`] (a context, a
//! secret, a resolved config), and a host's custom operator is a
//! [`datalogic_rs::CustomOperator`]. A host names both through these
//! re-exports, so its versions are always the engine's. A major version of
//! either is a major version of this crate.

#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations)]

extern crate self as dataflow_rs;

/// The crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

mod base;
pub mod compat;
pub mod def;
pub mod expr;
pub mod function;
pub mod observe;
pub mod program;
pub mod run;
mod state;

pub use crate::base::{Error, ErrorKind, Issue, Result};
pub use crate::def::WorkflowDef;
pub use crate::expr::{Expr, Path};
pub use crate::function::{Ctx, Function, Outcome, Registry};
pub use crate::observe::Observer;
pub use crate::run::{Engine, Message, Run, RunOptions};
pub use datalogic_rs;
pub use datavalue;
