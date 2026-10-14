//! `CancelToken`: a flag a host trips to cancel a run, shared by every clone
//! and inherited by every child.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A cancellation flag. Clones share one flag; a [`child`](Self::child) has
/// its own, and is cancelled when it or any ancestor is.
///
/// A function reads the run's token through
/// [`Ctx::cancel`](crate::function::Ctx::cancel), and hands a child to a run
/// it starts, so cancelling the parent cancels both (R60).
///
/// ```
/// # extern crate dataflow_rs_v4 as dataflow_rs;
/// use dataflow_rs::run::CancelToken;
///
/// let parent = CancelToken::new();
/// let child = parent.child();
/// child.cancel();
/// assert!(child.is_cancelled() && !parent.is_cancelled());
/// let other = parent.child();
/// parent.cancel();
/// assert!(other.is_cancelled());
/// ```
#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<Inner>,
}

/// One token's flag and the chain of its ancestors'.
#[derive(Default)]
struct Inner {
    flag: AtomicBool,
    parent: Option<Arc<Self>>,
}

impl CancelToken {
    /// A token not yet cancelled, with no parent.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels this token, its clones and every child made from them.
    pub fn cancel(&self) {
        self.inner.flag.store(true, Ordering::Release);
    }

    /// Whether this token or any of its ancestors was cancelled.
    pub fn is_cancelled(&self) -> bool {
        let mut at = Some(&self.inner);
        while let Some(inner) = at {
            if inner.flag.load(Ordering::Acquire) {
                return true;
            }
            at = inner.parent.as_ref();
        }
        false
    }

    /// A token cancelled with this one, or on its own without cancelling
    /// this one.
    #[must_use]
    pub fn child(&self) -> Self {
        Self {
            inner: Arc::new(Inner {
                flag: AtomicBool::new(false),
                parent: Some(Arc::clone(&self.inner)),
            }),
        }
    }
}

impl fmt::Debug for CancelToken {
    /// Whether it is cancelled.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancelToken")
            .field("cancelled", &self.is_cancelled())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_token_is_not_cancelled_and_cancel_trips_every_clone() {
        let token = CancelToken::new();
        let clone = token.clone();
        assert!(!token.is_cancelled());
        clone.cancel();
        assert!(token.is_cancelled() && clone.is_cancelled());
    }

    #[test]
    fn a_child_trips_with_any_ancestor_and_alone_without_them() {
        let root = CancelToken::new();
        let child = root.child();
        let grandchild = child.child();
        let sibling = root.child();
        grandchild.cancel();
        assert!(grandchild.is_cancelled());
        assert!(!child.is_cancelled() && !root.is_cancelled() && !sibling.is_cancelled());
        root.cancel();
        assert!(child.is_cancelled() && sibling.is_cancelled());
        assert!(root.child().is_cancelled(), "a child of a cancelled token");
    }

    #[test]
    fn debug_shows_whether_it_is_cancelled() {
        let token = CancelToken::new();
        assert_eq!(format!("{token:?}"), "CancelToken { cancelled: false, .. }");
    }
}
