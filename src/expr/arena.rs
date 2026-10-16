//! The per-thread pool of evaluation arenas. It lives in `expr`, below
//! `state`, because `ExprEngine::eval` is what takes one.
//!
//! Taking an arena while one is held takes another, so evaluation may re-enter
//! (an operator calling back into the engine) without a panic.

use bumpalo::Bump;
use std::cell::RefCell;

/// Arenas kept per thread.
const MAX_POOLED: usize = 8;
/// An arena holding more than this after `reset` is dropped, not pooled.
const MAX_RETAINED_BYTES: usize = 4 * 1024 * 1024;

thread_local! {
    static POOL: RefCell<Vec<Bump>> = const { RefCell::new(Vec::new()) };
}

/// An arena from this thread's pool, or a new one.
pub(crate) fn take() -> Bump {
    POOL.try_with(|pool| pool.borrow_mut().pop())
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// Resets `arena` and keeps it, unless the pool is full or it grew past 4 MiB.
/// A pool already torn down (thread exit) just drops the arena.
pub(crate) fn give(mut arena: Bump) {
    arena.reset();
    if arena.allocated_bytes() > MAX_RETAINED_BYTES {
        return;
    }
    let _ = POOL.try_with(|pool| {
        let mut pool = pool.borrow_mut();
        if pool.len() < MAX_POOLED {
            pool.push(arena);
        }
    });
}

/// Bytes in use: `allocated_bytes` counts whole chunks, and `reset` keeps the
/// largest one, so the current chunk's free space is taken off.
pub(crate) fn used(arena: &Bump) -> usize {
    arena
        .allocated_bytes()
        .saturating_sub(arena.chunk_capacity())
}

/// Runs `f` with an arena from this thread's pool, then resets it and returns
/// it to the pool unless the pool is full or the arena grew past 4 MiB.
pub(crate) fn with_arena<R>(f: impl FnOnce(&Bump) -> R) -> R {
    let arena = take();
    let result = f(&arena);
    give(arena);
    result
}

/// How many arenas this thread has pooled.
#[cfg(test)]
pub(crate) fn pool_len() -> usize {
    POOL.with(|pool| pool.borrow().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_returned_arena_is_reset_and_pooled() {
        assert_eq!(pool_len(), 0);
        with_arena(|a| {
            a.alloc_str("hello");
        });
        assert_eq!(pool_len(), 1);
        with_arena(|a| assert!(a.allocated_bytes() > 0));
        assert_eq!(pool_len(), 1);
    }

    #[test]
    fn nested_calls_get_distinct_arenas() {
        let (outer, inner) = with_arena(|a| {
            let outer = std::ptr::from_ref(a);
            let inner = with_arena(std::ptr::from_ref);
            (outer, inner)
        });
        assert_ne!(outer, inner);
        assert_eq!(pool_len(), 2);
    }

    #[test]
    fn at_most_eight_arenas_are_pooled() {
        fn nest(depth: usize) {
            if depth > 0 {
                with_arena(|_| nest(depth - 1));
            }
        }
        nest(12);
        assert_eq!(pool_len(), 8);
    }

    #[test]
    fn an_arena_still_over_four_mib_after_reset_is_dropped() {
        with_arena(|a| {
            a.alloc_slice_fill_copy(8 * 1024 * 1024, 0u8);
        });
        assert_eq!(pool_len(), 0);
    }

    #[test]
    fn a_given_arena_comes_back_reset() {
        let a = take();
        a.alloc_slice_fill_copy(1000, 7u8);
        assert!(used(&a) >= 1000);
        give(a);
        let b = take();
        assert!(used(&b) < 64, "reset on the way in: {}", used(&b));
        give(b);
    }

    #[test]
    fn used_counts_allocations_not_chunks() {
        let mut a = Bump::with_capacity(1 << 20);
        assert!(used(&a) < 64);
        a.alloc_slice_fill_copy(10_000, 1u8);
        let after = used(&a);
        assert!((10_000..11_000).contains(&after), "{after}");
        a.reset();
        assert!(used(&a) < 64);
    }
}
