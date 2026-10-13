//! dataflow-rs v4, under construction.
//!
//! This crate is built beside v3 (the root `dataflow-rs` crate) until phase 7
//! (design decision I22). Every public type follows the v4 design documents.

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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the runtime uses it from Task 4")
)]
mod state;

pub use crate::base::{Error, Issue, Result};
pub use crate::def::WorkflowDef;
pub use crate::expr::{Expr, Path};
pub use crate::function::{Ctx, Function, Outcome, Registry};
pub use datalogic_rs;
pub use datavalue;
