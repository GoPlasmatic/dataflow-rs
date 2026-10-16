//! `CancelToken`: a flag a host trips to cancel a run, shared by every clone
//! and inherited by every child.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use {std::pin::Pin, std::pin::pin, tokio::sync::Notify};

/// A cancellation flag. Clones share one flag; a [`child`](Self::child) has
/// its own, and is cancelled when it or any ancestor is.
///
/// A function reads the run's token through
/// [`Ctx::cancel`](crate::function::Ctx::cancel), and hands a child to a run
/// it starts, so cancelling the parent cancels both.
///
/// ```
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
    /// The tasks awaiting [`CancelToken::cancelled`] on this token, woken by
    /// its `cancel`. A child's waiters wait on every ancestor's too.
    #[cfg(not(target_arch = "wasm32"))]
    waiters: Notify,
}

impl CancelToken {
    /// A token not yet cancelled, with no parent.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels this token, its clones and every child made from them.
    #[cfg_attr(
        not(target_arch = "wasm32"),
        doc = "Every task awaiting [`cancelled`](Self::cancelled) on them is woken."
    )]
    pub fn cancel(&self) {
        self.inner.flag.store(true, Ordering::Release);
        #[cfg(not(target_arch = "wasm32"))]
        self.inner.waiters.notify_waiters();
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
                parent: Some(Arc::clone(&self.inner)),
                ..Inner::default()
            }),
        }
    }

    /// Resolves once this token or any of its ancestors is cancelled, at
    /// once if one already is: for a host's own races, such as a
    /// `tokio::select!` between its I/O and the run's token. Native targets
    /// only.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn cancelled(&self) {
        tripped(&self.inner).await;
    }
}

/// Resolves once `inner`'s flag or an ancestor's is set. It registers on
/// `inner`'s waiters before it reads the flag, so a `cancel` between the two
/// still wakes it, and awaits the parent's the same way, boxed: a chain of
/// children has no fixed depth.
#[cfg(not(target_arch = "wasm32"))]
async fn tripped(inner: &Inner) {
    let mut own = pin!(inner.waiters.notified());
    own.as_mut().enable();
    if inner.flag.load(Ordering::Acquire) {
        return;
    }
    match &inner.parent {
        None => own.await,
        Some(parent) => {
            futures_util::future::select(own, boxed(parent)).await;
        }
    }
}

/// [`tripped`] for an ancestor, boxed to a named type so the recursion has
/// one.
#[cfg(not(target_arch = "wasm32"))]
fn boxed(inner: &Inner) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
    Box::pin(tripped(inner))
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

    /// Polls `future` once, with a waker that does nothing.
    #[cfg(not(target_arch = "wasm32"))]
    fn poll_once<F: Future>(future: Pin<&mut F>) -> bool {
        let waker = std::task::Waker::noop();
        future
            .poll(&mut std::task::Context::from_waker(waker))
            .is_ready()
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn cancelled_is_pending_until_the_token_or_an_ancestor_is_cancelled() {
        let root = CancelToken::new();
        let child = root.child();
        let mut on_child = pin!(child.cancelled());
        let mut on_root = pin!(root.cancelled());
        assert!(!poll_once(on_child.as_mut()) && !poll_once(on_root.as_mut()));
        root.cancel();
        assert!(poll_once(on_child.as_mut()) && poll_once(on_root.as_mut()));
        // Already cancelled: ready at the first poll.
        assert!(poll_once(pin!(child.child().cancelled())));
    }

    #[test]
    fn debug_shows_whether_it_is_cancelled() {
        let token = CancelToken::new();
        assert_eq!(format!("{token:?}"), "CancelToken { cancelled: false, .. }");
    }
}
