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
    AuthoredPath, IssueCode, ParseIssueCodeError, ParsePathError, PathSeg, ProgramId, Severity,
    StepKind, TaskIx, WorkflowIx,
};
pub(crate) use checks::Checks;
pub(crate) use compile::{CalleeAt, compile_callee, refused};
pub use compile::{CompileOptions, Compiled, CompiledWorkflow, Compiler, ResourceOracle};
pub use fingerprint::{Fingerprint, ParseFingerprintError};
pub use link::{Program, TaskInfo, WorkflowInfo};
pub(crate) use ops::{CallSite, Callee, CondKind, FanSite, LoopSite, Op, Policy};
