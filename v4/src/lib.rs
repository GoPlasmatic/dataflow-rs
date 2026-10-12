//! dataflow-rs v4, under construction.
//!
//! This crate is built beside v3 (the root `dataflow-rs` crate) until phase 7
//! (design decision I22). Every public type follows the v4 design documents.

#![forbid(unsafe_code)]

extern crate self as dataflow_rs;

/// The crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

mod base;
pub mod compat;
pub mod def;
pub mod expr;
pub mod function;
pub mod program;
pub mod run;

pub use crate::base::{Error, Issue, Result};
