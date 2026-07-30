//! # Execution Observer Module
//!
//! An always-on, per-task callback for aggregation — counters, histograms,
//! spans — as distinct from [`crate::ExecutionTrace`], which is a per-request
//! allocation you persist.
//!
//! This exists because the eight sync built-ins (`map`, `validation`/`validate`,
//! `parse_json`, `parse_xml`, `publish_json`, `publish_xml`, `filter`, `log`)
//! are dispatched inside a private method on the workflow executor and never
//! reach the function registry. A host can time its own registered handlers by
//! wrapping their bodies, but it cannot time those eight at any price, and so
//! cannot tell how much of a message's wall clock was spent inside the engine
//! versus inside its own handlers.

use core::time::Duration;

/// One finished task.
///
/// Borrowed for the duration of the callback; an observer must copy out anything
/// it needs to keep.
#[derive(Debug)]
pub struct TaskEvent<'a> {
    /// `Workflow::id` of the workflow the task belongs to.
    pub workflow_id: &'a str,
    /// `Task::id`.
    pub task_id: &'a str,
    /// The function name — one of the built-in names, or a `Custom` handler's
    /// registered name.
    ///
    /// Note this reports `"validate"` for both `validation` and `validate`
    /// configs: they share a single `FunctionConfig::Validation` variant, and
    /// this is that variant's canonical name.
    pub function: &'a str,
    /// `TaskOutcome::audit_status()` for a successful dispatch, `Some(500)` when
    /// the task returned `Err`.
    ///
    /// `None` means the handler returned `TaskOutcome::Skip` — the body ran, but
    /// no audit entry was recorded for it.
    pub status: Option<u16>,
    /// Wall-clock duration of the task **body only**: the dispatch call, not the
    /// condition evaluation, the audit-trail push, or the `metadata.progress`
    /// write.
    ///
    /// Derived from two `Utc::now()` reads rather than a monotonic clock, because
    /// `std::time::Instant::now()` panics on `wasm32-unknown-unknown` and the
    /// wasm bindings route through these instrumentation points. A backward clock
    /// step clamps to zero rather than wrapping.
    pub duration: Duration,
}

/// Receives one callback per dispatched task.
///
/// Object-safe by construction — no generic methods, no associated types — so
/// this is unrelated to the [`crate::AsyncFunctionHandler`] /
/// `DynAsyncFunctionHandler` split and needs no `Dyn` sibling.
/// `Arc<dyn ExecutionObserver>` works directly.
///
/// # Contract
///
/// `task_finished` is called **synchronously**, on the executor's thread,
/// immediately after the task body returns and *before* the audit trail is
/// written. On the sync-built-in path it runs inside the arena scope while the
/// `!Send` arena borrow is live.
///
/// So an implementation must not block, must not re-enter the engine, and must
/// not panic — a panic unwinds through the arena scope and out of
/// `process_message`. It cannot `await`, since the method is synchronous. Push to
/// a channel or bump an atomic and return.
///
/// A task whose condition evaluated false is **not** reported: it was never
/// dispatched, so there is nothing to time. Tasks that fail *are* reported, with
/// `status: Some(500)` — the event is emitted before the error propagates,
/// because those are the tasks a host most wants timed.
///
/// # Example
///
/// ```
/// use dataflow_rs::{ExecutionObserver, TaskEvent};
/// use std::sync::atomic::{AtomicU64, Ordering};
///
/// #[derive(Default)]
/// struct TotalMicros(AtomicU64);
///
/// impl ExecutionObserver for TotalMicros {
///     fn task_finished(&self, event: &TaskEvent<'_>) {
///         // Cheap and non-blocking, as the contract requires.
///         self.0
///             .fetch_add(event.duration.as_micros() as u64, Ordering::Relaxed);
///     }
/// }
/// ```
pub trait ExecutionObserver: Send + Sync + 'static {
    /// Called once per dispatched task, immediately after its body returns.
    fn task_finished(&self, event: &TaskEvent<'_>);
}
