//! Runs: executing a program over a message.

mod builder;
mod cancel;
mod engine;
mod fan_out;
pub(crate) mod host;
mod interp;
mod message;
mod observers;
mod options;
pub(crate) mod policy;
mod record;
mod report;
mod trace;

/// The clock a deadline is read on: `std::time::Instant` until phase 7's wasm
/// track makes it `web_time::Instant`, the same type on native targets (I86).
pub use crate::base::Instant;
pub use crate::base::{CancelToken, Change, Error, ErrorKind, ErrorRecord, Result};
pub use crate::observe::{RunStatus, Selection, WorkflowExit, WorkflowRecord};
pub use builder::EngineBuilder;
pub use engine::{Engine, EngineConfig};
pub use message::{AuditEntry, AuditMode, Context, Message};
pub use options::RunOptions;
#[doc(hidden)]
pub use report::ArenaStats;
pub use report::Run;
