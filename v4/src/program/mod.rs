//! Programs: a definition compiled for execution.
//!
//! A [`Compiler`] comes from a [`Registry`](crate::Registry), and compiles one
//! workflow at a time, so a host owns quarantine: a Rejected issue rejects only
//! its own workflow.

mod checks;
mod compile;
mod fingerprint;
mod link;
mod lower;
mod ops;

pub use crate::base::{
    AuthoredPath, Issue, IssueCode, ParseIssueCodeError, ParsePathError, PathSeg, ProgramId,
    Severity, StepKind, TaskIx, WorkflowIx,
};
pub use compile::{CompileOptions, Compiled, CompiledWorkflow, Compiler, ResourceOracle};
pub use fingerprint::{Fingerprint, ParseFingerprintError};
pub use link::{Program, TaskInfo, WorkflowInfo};
