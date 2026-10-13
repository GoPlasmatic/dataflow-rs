//! Runs: executing a program over a message.

mod message;
mod options;
mod report;

pub use crate::base::{Change, Error, ErrorKind, ErrorRecord, Result};
pub use crate::observe::{RunStatus, Selection, WorkflowExit, WorkflowRecord};
pub use message::{AuditEntry, AuditMode, Context, Message};
#[expect(unused_imports, reason = "the engine uses them from Task 6")]
pub(crate) use message::{Parts, finish_audit, record_audit};
pub use options::RunOptions;
#[doc(hidden)]
pub use report::ArenaStats;
#[expect(unused_imports, reason = "the engine uses it from Task 6")]
pub(crate) use report::Record;
pub use report::Run;
