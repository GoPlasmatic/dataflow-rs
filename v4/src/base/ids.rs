//! Opaque identities of programs and the positions within them.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_PROGRAM: AtomicU64 = AtomicU64::new(1);

/// The identity of one compiled program, unique within the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProgramId(u64);

impl ProgramId {
    /// The next unused identity.
    pub(crate) fn next() -> Self {
        Self(NEXT_PROGRAM.fetch_add(1, Ordering::Relaxed))
    }

    /// The identity as a number.
    pub fn index(self) -> u64 {
        self.0
    }
}

/// The position of a workflow within a program. Serialized as its number,
/// and read from one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkflowIx(u32);

impl WorkflowIx {
    /// The workflow at position `index`.
    pub(crate) fn new(index: u32) -> Self {
        Self(index)
    }

    /// The position as a number.
    pub fn index(self) -> u32 {
        self.0
    }
}

/// The position of a task within a program. Serialized as its number, and
/// read from one: any `u32`, so a position read for another program makes
/// [`Program::task`](crate::program::Program::task) panic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskIx(u32);

impl TaskIx {
    /// The task at position `index`.
    pub(crate) fn new(index: u32) -> Self {
        Self(index)
    }

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

    #[test]
    fn a_position_is_its_number() {
        assert_eq!(WorkflowIx::new(3).index(), 3);
        assert_eq!(TaskIx::new(7).index(), 7);
        assert!(TaskIx::new(1) < TaskIx::new(2));
    }
}
