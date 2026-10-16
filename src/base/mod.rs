//! Crate-private foundations shared by the public modules.

mod cancel;
mod change;
mod context;
mod error;
mod ids;
mod issue;
#[cfg(feature = "ts")]
pub(crate) mod ts;
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
// `Ctx::deadline`: `web_time::Instant`, which is `std::time::Instant` on native
// targets and a `performance.now()` clock on wasm32, where `std`'s panics (I86).
pub use web_time::Instant;
