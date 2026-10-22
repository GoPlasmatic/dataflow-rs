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
pub(crate) use context::{ROOTS, exact, in_order, keep_last};
pub use error::{
    BudgetCause, Error, ErrorKind, ErrorRecord, ErrorSite, ExprCause, ExprPathStep, Result,
    SiteKind,
};
pub(crate) use error::{
    EVALUATION_ERROR, MAPPING_ERROR, TASK_ERROR, TASK_STATUS_ERROR, VALIDATION_ERROR,
};
pub use ids::{ProgramId, TaskIx, WorkflowIx};
pub(crate) use issue::any_rejected;
pub use issue::{
    AuthoredPath, Issue, IssueCode, ParseIssueCodeError, ParsePathError, PathSeg, Severity,
    StepKind,
};
// The clock a deadline is read on, named by `run::Instant` and by
// `Ctx::deadline`: `web_time::Instant`, which is `std::time::Instant` on native
// targets and a `performance.now()` clock on wasm32, where `std`'s panics.
pub use web_time::Instant;
