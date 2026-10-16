//! Runs: executing a program over a message.

mod builder;
mod cancel;
mod engine;
mod error_context;
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

/// The clock a deadline is read on: `web_time::Instant`, which is
/// `std::time::Instant` itself on native targets and a browser clock on wasm32
/// (I86).
pub use crate::base::Instant;
pub use crate::base::{CancelToken, Change, Context, Error, ErrorKind, ErrorRecord, Result};
pub use crate::observe::{RunStatus, Selection, WorkflowExit, WorkflowRecord};
pub use builder::EngineBuilder;
pub use engine::{Engine, EngineConfig};
pub use error_context::ErrorContext;
pub use message::{AuditEntry, AuditMode, Message};
pub use options::RunOptions;
#[cfg(test)]
pub(crate) use record::traced_work;
#[doc(hidden)]
pub use report::ArenaStats;
pub use report::Run;
