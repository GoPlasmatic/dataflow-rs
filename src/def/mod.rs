//! Definitions: the authored form of workflows and tasks.
//!
//! A definition is plain data with raw expressions. It enters only through
//! [`parse`], which is also the linter: one walk over the authored JSON builds
//! a [`WorkflowDef`] and reports every issue at the path the author typed, at
//! v3's codes for v3's rules. With [`ParseOptions::compat`] on, the compat
//! reader rewrites v3's spellings first (see [`compat`](crate::compat)). A
//! definition serializes in the v4 spelling, which `parse` reads back with the
//! reader off.
//!
//! ```
//! use dataflow_rs::def::{ParseOptions, parse};
//! use dataflow_rs::program::IssueCode;
//! use serde_json::json;
//!
//! let parsed = parse(
//!     &json!({"id": "w", "tasks": [
//!         {"id": "t", "conditon": true, "function": {"name": "f"}}
//!     ]}),
//!     ParseOptions::default(),
//! );
//! // A near miss of a known key loads, kept in the task's `ext`, and is reported.
//! assert_eq!(parsed.issues[0].code, IssueCode::LikelyTypo);
//! assert_eq!(parsed.issues[0].path.to_string(), "tasks[0].conditon");
//! assert!(parsed.def.is_some());
//! ```

mod for_each;
pub(crate) mod keys;
mod loops;
mod parse;
pub(crate) mod reader;
mod rollout;
mod ser;
mod types;

pub(crate) use for_each::for_each_issue;
pub(crate) use loops::bound_issues;
pub(crate) use parse::parse_steps;
pub(crate) use parse::parse_with_rewrites;
pub use parse::{ParseOptions, Parsed, parse};
pub(crate) use parse::{fragment_place, holds_steps};
pub use rollout::{Rollout, RolloutError};
pub use types::defaults;
pub use types::{
    FailurePolicy, ForEachDef, FunctionRef, GroupDef, Logic, LoopDef, NodeHeader, RejectionPolicy,
    Status, StepDef, TaskDef, WorkflowDef, WorkflowFailurePolicy,
};

/// The deepest group nesting a definition may use.
///
/// Depth counts enclosing groups: a top-level group is at depth 0, so groups
/// are accepted at depths `0..MAX_GROUP_DEPTH`, and a group at
/// `MAX_GROUP_DEPTH` is `GROUP_TOO_DEEP`.
pub const MAX_GROUP_DEPTH: usize = 8;

/// v3's alias for `validation`, which the compat reader rewrites and no
/// host function may take.
pub(crate) const V3_VALIDATE: &str = "validate";

/// The built-in function names. They are reserved: no registration shadows
/// one, and a task running one cannot carry `for_each`, since a built-in runs
/// inline.
pub const BUILTIN_FUNCTIONS: [&str; 8] = [
    "map",
    "validation",
    "filter",
    "log",
    "parse_json",
    "parse_xml",
    "publish_json",
    "publish_xml",
];
