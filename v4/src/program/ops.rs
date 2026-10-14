//! The ops a workflow lowers to, and the side tables they index.
//!
//! The compile walk fills the tables in walk order: setup first, then the
//! body, pre-order. Every field is `pub(crate)`, for the runtime.

use crate::base::{AuthoredPath, StepKind};
use crate::def::{FailurePolicy, RejectionPolicy};
use crate::expr::{Expr, ExprVisitor, Path};
use crate::function::BoundCall;
use crate::function::builtins::Builtin;

/// What a `Cond` op gates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CondKind {
    Workflow,
    Group,
    Task,
}

/// One instruction. A target is an op index, and `ops.len()` is the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    Cond {
        cond: u32,
        kind: CondKind,
        else_to: u32,
    },
    Call {
        site: u32,
        fail_to: u32,
    },
    FanOut {
        site: u32,
        fan: u32,
        fail_to: u32,
    },
    LoopInit {
        lp: u32,
        exit_to: u32,
    },
    LoopNext {
        lp: u32,
        body_to: u32,
        exit_to: u32,
    },
    Halt,
}

const _: () = assert!(size_of::<Op>() == 16);

/// A compiled condition: the workflow's, a group's or a task's.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "read by the lowering and the runtime")
)]
pub(crate) struct CondSite {
    pub(crate) expr: Expr,
    /// Where the condition is authored.
    pub(crate) path: AuthoredPath,
    /// The group's or task's id; `None` for the workflow's.
    pub(crate) step: Option<String>,
}

/// One task's call: what it calls and what its outcome does.
pub(crate) struct CallSite {
    /// The task's row in `tasks`.
    pub(crate) task: u32,
    pub(crate) callee: Callee,
    pub(crate) on_failure: FailurePolicy,
    pub(crate) on_rejection: RejectionPolicy,
    pub(crate) terminal: bool,
}

/// What a call site calls (I42): a built-in's typed config (the parse and
/// publish ones boxed, I71), or a host function bound to its config.
pub(crate) enum Callee {
    Builtin(Builtin),
    Host(Box<dyn BoundCall>),
}

impl Callee {
    /// Walks the config's expressions; paths are relative to the config.
    pub(crate) fn visit(&self, v: &mut dyn ExprVisitor) {
        match self {
            Self::Builtin(builtin) => builtin.visit(v),
            Self::Host(bound) => bound.visit(v),
        }
    }
}

/// A task's fan-out.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "read by the lowering and the runtime")
)]
pub(crate) struct FanSite {
    pub(crate) over: Expr,
    /// `temp_data.<as>`.
    pub(crate) r#as: Path,
    /// `temp_data.<as>_index`.
    pub(crate) index: Path,
    pub(crate) max_concurrency: usize,
    pub(crate) collect: Option<Path>,
    pub(crate) into: Option<Path>,
}

/// A workflow's loop. Each slot is a `temp_data` path.
pub(crate) struct LoopSite {
    pub(crate) init: i64,
    pub(crate) increment: i64,
    pub(crate) max: i64,
    pub(crate) counter: Option<Path>,
    pub(crate) over: Option<Expr>,
    pub(crate) r#as: Option<Path>,
    pub(crate) scratch: Option<Path>,
}

/// One task, as the program lists it: one row per task, never per group.
pub(crate) struct TaskMeta {
    pub(crate) id: String,
    /// The registered name of the function it calls.
    pub(crate) function: String,
    pub(crate) kind: StepKind,
    pub(crate) path: AuthoredPath,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_site_fits_in_256_bytes() {
        // I71: the parse and publish configs are boxed so the hot built-ins
        // can stay inline.
        let size = std::mem::size_of::<CallSite>();
        assert!(size <= 256, "a call site is {size} bytes");
    }
}
