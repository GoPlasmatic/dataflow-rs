//! Crate-private foundations shared by the public modules.

mod cancel;
mod change;
mod context;
mod error;
mod ids;
mod issue;
pub(crate) mod typo;

pub use cancel::CancelToken;
pub use change::Change;
pub use context::Context;
pub(crate) use context::exact;
pub use error::{Error, ErrorKind, ErrorRecord, Result};
pub use ids::{ProgramId, TaskIx, WorkflowIx};
pub use issue::{
    AuthoredPath, Issue, IssueCode, ParseIssueCodeError, ParsePathError, PathSeg, Severity,
    StepKind,
};
// The clock a deadline is read on, named by `run::Instant` and by
// `Ctx::deadline`: the one line phase 7's wasm track changes to
// `web_time::Instant`, the same type on native targets (I86).
pub use std::time::Instant;
