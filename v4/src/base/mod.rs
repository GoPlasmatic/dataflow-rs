//! Crate-private foundations shared by the public modules.

mod change;
mod error;
mod ids;
mod issue;
pub(crate) mod typo;

pub use change::Change;
pub use error::{Error, ErrorKind, ErrorRecord, Result};
pub use ids::{ProgramId, TaskIx, WorkflowIx};
pub use issue::{
    AuthoredPath, Issue, IssueCode, ParseIssueCodeError, ParsePathError, PathSeg, Severity,
    StepKind,
};
