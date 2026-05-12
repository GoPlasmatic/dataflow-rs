//! # Task context
//!
//! Wraps the per-call state passed to every `AsyncFunctionHandler::execute`
//! call: the message under processing, a handle to the shared datalogic
//! engine, and an audit-trail accumulator. Exposes typed helpers so handlers
//! don't have to reach into `crate::engine::utils::{get,set}_nested_value`
//! or hand-build `Change` entries.
//!
//! Custom handlers should treat `TaskContext` as their *only* mutation surface
//! for `message.context`: the `set` family records a `Change` automatically
//! when `message.capture_changes` is true, keeping the audit trail in sync
//! with the data without per-handler boilerplate.

use crate::engine::error::ErrorInfo;
use crate::engine::message::{Change, Message};
use crate::engine::utils::{get_nested_value, set_nested_value};
use datalogic_rs::Engine as DatalogicEngine;
use datavalue::OwnedDataValue;
use serde_json::Value as JsonValue;
use std::sync::Arc;

/// Per-call execution context handed to `AsyncFunctionHandler::execute`.
///
/// Borrows the message and datalogic engine for the duration of the handler
/// call; collects `Change` entries that the workflow executor folds into the
/// audit trail when the handler returns. Drop semantics are trivial — there
/// is nothing to flush; the executor extracts the buffered changes via
/// `into_changes()`.
pub struct TaskContext<'a> {
    message: &'a mut Message,
    datalogic: &'a Arc<DatalogicEngine>,
    /// Changes accumulated through the `set*` family. Only populated when
    /// `message.capture_changes` is true; otherwise pushes are no-ops to
    /// keep the bulk-pipeline fast path allocation-free.
    changes: Vec<Change>,
}

impl<'a> TaskContext<'a> {
    /// Construct a new context. Mostly engine-internal — handlers receive a
    /// pre-built `&mut TaskContext` from the executor — but exposed `pub` so
    /// tests and benchmarks can drive `AsyncFunctionHandler::execute`
    /// directly without going through `Engine::process_message`.
    pub fn new(message: &'a mut Message, datalogic: &'a Arc<DatalogicEngine>) -> Self {
        Self {
            message,
            datalogic,
            changes: Vec::new(),
        }
    }

    /// Borrow the message under processing. Use this when you need to inspect
    /// the message id, payload, or audit trail; for reading and mutating the
    /// `data` / `metadata` / `temp_data` context, prefer the typed helpers on
    /// `TaskContext` itself.
    #[inline]
    pub fn message(&self) -> &Message {
        self.message
    }

    /// Mutable access to the message. Prefer the typed helpers (`set`,
    /// `add_error`) over poking at `message.context` directly — direct
    /// mutations bypass the audit trail.
    #[inline]
    pub fn message_mut(&mut self) -> &mut Message {
        self.message
    }

    /// Shared datalogic engine, in case the handler needs to evaluate ad-hoc
    /// JSONLogic. Most handlers can ignore this argument.
    #[inline]
    pub fn datalogic(&self) -> &Arc<DatalogicEngine> {
        self.datalogic
    }

    /// Read-only view of `data`. Returns `&OwnedDataValue::Null` if missing
    /// (mirrors the `Index` fallback semantics of `serde_json::Value`).
    #[inline]
    pub fn data(&self) -> &OwnedDataValue {
        self.message.data()
    }

    /// Read-only view of `metadata`.
    #[inline]
    pub fn metadata(&self) -> &OwnedDataValue {
        self.message.metadata()
    }

    /// Read-only view of `temp_data`.
    #[inline]
    pub fn temp_data(&self) -> &OwnedDataValue {
        self.message.temp_data()
    }

    /// Look up a value by dot-path against the full context tree (rooted at
    /// the unified `{data, metadata, temp_data}` object). Returns `None` if
    /// the path doesn't resolve.
    ///
    /// Use the same path syntax as JSONLogic: `"data.user.name"`,
    /// `"temp_data.items.0"`, `"metadata.progress.status_code"`.
    #[inline]
    pub fn get(&self, path: &str) -> Option<&OwnedDataValue> {
        get_nested_value(&self.message.context, path)
    }

    /// Set a value at a dot-path on the context. Records a `Change` on the
    /// audit trail when `message.capture_changes` is true; otherwise the
    /// write happens but no audit entry is buffered.
    ///
    /// Intermediate objects/arrays are created on demand; see
    /// [`crate::engine::utils::set_nested_value`] for the exact semantics
    /// (numeric segments → arrays, `#` prefix → escaped object key, etc.).
    pub fn set(&mut self, path: &str, value: OwnedDataValue) {
        if self.message.capture_changes {
            let old_value = get_nested_value(&self.message.context, path)
                .cloned()
                .unwrap_or(OwnedDataValue::Null);
            let new_value = value.clone();
            self.changes.push(Change {
                path: Arc::from(path),
                old_value,
                new_value,
            });
        }
        set_nested_value(&mut self.message.context, path, value);
    }

    /// Same as [`Self::set`] but accepts a `serde_json::Value` (bridges
    /// through `OwnedDataValue::from`). Convenience for handlers that
    /// already speak `serde_json::Value`.
    #[inline]
    pub fn set_json(&mut self, path: &str, value: &JsonValue) {
        self.set(path, OwnedDataValue::from(value));
    }

    /// Append an error to `message.errors`. Convenience for
    /// `ctx.message_mut().add_error(...)`.
    #[inline]
    pub fn add_error(&mut self, error: ErrorInfo) {
        self.message.add_error(error);
    }

    /// Drain the accumulated changes. The workflow executor calls this after
    /// the handler returns to fold them into the audit trail; tests and
    /// benchmarks driving the trait directly can use it to inspect what the
    /// handler buffered.
    #[inline]
    pub fn into_changes(self) -> Vec<Change> {
        self.changes
    }
}
