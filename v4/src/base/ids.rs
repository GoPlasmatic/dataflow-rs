//! Opaque identities of programs and the positions within them.

use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_PROGRAM: AtomicU64 = AtomicU64::new(1);

/// The identity of one compiled program, unique within the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProgramId(u64);

impl ProgramId {
    /// The next unused identity.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used from program, which assigns one per build")
    )]
    pub(crate) fn next() -> Self {
        Self(NEXT_PROGRAM.fetch_add(1, Ordering::Relaxed))
    }

    /// The identity as a number.
    pub fn index(self) -> u64 {
        self.0
    }
}

/// The position of a workflow within a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkflowIx(u32);

impl WorkflowIx {
    /// The position as a number.
    pub fn index(self) -> u32 {
        self.0
    }
}

/// The position of a task within a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskIx(u32);

impl TaskIx {
    /// The position as a number.
    pub fn index(self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_ids_are_distinct() {
        assert_ne!(ProgramId::next(), ProgramId::next());
        assert!(ProgramId::next().index() > 0);
    }
}
