//! # Execution Trace Module
//!
//! This module provides step-by-step execution tracing for debugging workflows.
//! It captures message snapshots after each step, including which workflows/tasks
//! were executed or skipped.
//!
//! [`TraceOptions`] controls what a trace-mode run records. The default
//! reproduces the historical behaviour — a full [`Message`] snapshot per
//! executed step — which is unbounded in message size and quadratic in task
//! count, because each snapshot clones the accumulated audit trail. Hosts that
//! persist traces should set a snapshot budget, an audit-trail scope, or both.

use crate::engine::message::{AuditTrail, Change, Message};
use crate::engine::utils::strip_hash_prefix;
use chrono::{DateTime, Utc};
use datavalue::OwnedDataValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Approximate in-memory cost charged for a single container or scalar node.
/// One machine word, per [`TraceOptions::max_snapshot_bytes`]'s contract.
const NODE_SIZE: usize = std::mem::size_of::<usize>();

/// `skip_serializing_if` predicate — omit `false` so a complete trace keeps the
/// historical wire shape and only a truncated one carries the flag.
#[inline]
fn is_false(b: &bool) -> bool {
    !*b
}

/// Result of executing a step (workflow or task)
///
/// Deliberately **not** `#[non_exhaustive]`. Downstream code matches on this to
/// classify a step, and the npm wire type mirrors it as a string union; adding a
/// variant should break those matches at compile time rather than silently
/// reclassify them through a `_` arm.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StepResult {
    /// The step was executed
    Executed,
    /// The step was skipped due to condition being false
    Skipped,
}

/// How much of [`Message::audit_trail`] a step's snapshot carries.
///
/// This is the knob for the *quadratic* term of trace size: under
/// [`AuditTrailScope::Full`], step `i` clones `i` audit entries, so an N-task
/// workflow retains `N*(N+1)/2` of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditTrailScope {
    /// Every entry accumulated so far — the historical behaviour. Total entries
    /// across the trace grow as `N*(N+1)/2` in task count.
    #[default]
    Full,
    /// Only the entry this task produced, and none for a `TaskOutcome::Skip`
    /// step. Linear in task count, and sufficient for the `dataflow-ui` step
    /// debugger, which reads only the last entry.
    Own,
    /// Empty `audit_trail` in every snapshot.
    None,
}

/// What a trace-mode run records for each executed step.
///
/// [`TraceOptions::default`] reproduces the historical capture behaviour: a full
/// [`Message`] snapshot and per-mapping contexts on every executed step, no
/// budget, no redaction, and the whole accumulated audit trail.
///
/// Trace mode reads the clock twice per executed task to populate
/// [`ExecutionStep::duration_us`]. The non-trace path
/// ([`crate::Engine::process_message`]) is unaffected and still takes a single
/// `Utc::now()` per message.
#[derive(Clone, Debug)]
pub struct TraceOptions {
    /// Full `Message` snapshot per executed step. `true` (default) is the
    /// historical behaviour and is what the `dataflow-ui` step debugger
    /// requires. With `false`, [`ExecutionTrace::final_message`] returns `None`
    /// and [`ExecutionTrace::is_success`] degenerates to `true` — inspect
    /// `Message::errors` on the message you passed in instead.
    pub snapshots: bool,

    /// Per-mapping context snapshots for `map` tasks. `true` (default). These
    /// are whole-context clones, one per mapping, so a multi-mapping `map` task
    /// can snapshot more than the step's own `Message` does.
    pub mapping_contexts: bool,

    /// Per-step diff: the changes produced by this task and nothing else.
    /// `false` (default) preserves the historical payload byte for byte.
    ///
    /// Prefer this over reading `audit_trail.last()`, which mis-attributes on a
    /// `TaskOutcome::Skip` step — no audit entry is recorded for a skip, so the
    /// last entry belongs to a different task.
    ///
    /// Empty when the message was built with
    /// `MessageBuilder::capture_changes(false)`: this flag reports the diff, it
    /// does not turn capture on.
    pub changes: bool,

    /// Soft budget over the *approximate accumulated in-memory size* of the
    /// snapshots taken so far — container and scalar nodes counted as one
    /// machine word, `String` contents by `str::len()`. This is **not**
    /// serialized JSON length; measuring that would mean serializing every step,
    /// which defeats the purpose of a pre-capture budget.
    ///
    /// Once exceeded, later executed steps are still recorded — ids, result,
    /// timing, and `changes` if enabled — with `message: None`, and
    /// [`ExecutionTrace::truncated`] returns `true`. `0` (default) is unbounded.
    pub max_snapshot_bytes: usize,

    /// Dot-paths under the message context whose subtrees are replaced with
    /// `OwnedDataValue::Null` as the snapshot is built, via a *pruning clone*:
    /// the redacted subtree is never cloned, so this bounds the snapshot's
    /// memory as well as its content. The live message is untouched, so later
    /// tasks still read the real values.
    ///
    /// Also applied to `mapping_contexts`, which are whole-context clones and
    /// would otherwise carry the redacted subtree through unchanged.
    ///
    /// Path syntax is the [`crate::engine::utils::get_nested_value`] vocabulary:
    /// dot segments, numeric segments index arrays, one leading `#` escapes a
    /// numerically-named object key. Unlike `set_nested_value`, a path that does
    /// not resolve creates nothing and is a no-op, and an empty path is ignored.
    ///
    /// This is a literal path list and nothing more — no value scanning, no
    /// pattern matching, no credential heuristics. It is not a masking engine.
    pub redact_paths: Vec<String>,

    /// How much of the accumulated audit trail each snapshot carries. See
    /// [`AuditTrailScope`]; this is the lever for trace size in task count.
    pub snapshot_audit_trail: AuditTrailScope,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self {
            snapshots: true,
            mapping_contexts: true,
            changes: false,
            max_snapshot_bytes: 0,
            redact_paths: Vec::new(),
            snapshot_audit_trail: AuditTrailScope::Full,
        }
    }
}

impl TraceOptions {
    /// Ids, result, timing and the per-task diff; no message snapshots and no
    /// mapping contexts. A step costs a few hundred bytes plus its diff,
    /// regardless of message size or task count.
    ///
    /// Note this is UI-incompatible: the `dataflow-ui` step debugger needs
    /// snapshots to render the step view.
    pub fn timings_only() -> Self {
        Self {
            snapshots: false,
            mapping_contexts: false,
            changes: true,
            ..Default::default()
        }
    }

    /// Pre-split `redact_paths` into raw segments once per trace.
    ///
    /// Segments stay raw so object-key matching can apply the `#` escape while
    /// array matching parses the segment, exactly as `get_nested_value` does.
    /// Empty paths are dropped here, which is what makes them a no-op.
    fn redact_segments(&self) -> Vec<Vec<String>> {
        self.redact_paths
            .iter()
            .filter(|p| !p.is_empty())
            .map(|p| p.split('.').map(str::to_string).collect())
            .collect()
    }
}

/// A single step in the execution trace
///
/// `#[non_exhaustive]`: construct through [`ExecutionStep::executed`],
/// [`ExecutionStep::task_skipped`] or [`ExecutionStep::workflow_skipped`] and
/// chain the `with_*` methods. Field reads and `..` patterns are unaffected.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ExecutionStep {
    /// ID of the workflow this step belongs to
    pub workflow_id: String,
    /// ID of the task (None for workflow-level skips)
    pub task_id: Option<String>,
    /// Result of the step execution
    pub result: StepResult,
    /// Message snapshot after this step. `None` for skipped steps, when
    /// [`TraceOptions::snapshots`] is off, and for executed steps recorded after
    /// [`TraceOptions::max_snapshot_bytes`] was exceeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    /// Context snapshots before each mapping (map tasks only, trace mode only).
    /// `mapping_contexts[i]` is `message.context` before `mapping[i]` executed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mapping_contexts: Option<Vec<Value>>,
    /// Wall-clock start of the task body. `Executed` steps only.
    ///
    /// `chrono`, not `std::time::Instant`: `Instant::now()` panics on
    /// `wasm32-unknown-unknown`, and the wasm bindings run the trace path there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// Task body duration in microseconds. `Executed` steps only.
    ///
    /// Derived from two `Utc::now()` reads, which are not monotonic: a backward
    /// clock step clamps to `0` rather than wrapping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_us: Option<u64>,
    /// This task's own writes, when [`TraceOptions::changes`] is set. `Some(vec![])`
    /// means the task wrote nothing (or change capture is off), which is
    /// distinct from `None` meaning "not recorded".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changes: Option<Vec<Change>>,
}

impl ExecutionStep {
    /// Create a new executed step with a message snapshot
    pub fn executed(workflow_id: &str, task_id: &str, message: &Message) -> Self {
        Self {
            workflow_id: workflow_id.to_string(),
            task_id: Some(task_id.to_string()),
            result: StepResult::Executed,
            message: Some(message.clone()),
            mapping_contexts: None,
            started_at: None,
            duration_us: None,
            changes: None,
        }
    }

    /// Create a skipped task step
    pub fn task_skipped(workflow_id: &str, task_id: &str) -> Self {
        Self {
            workflow_id: workflow_id.to_string(),
            task_id: Some(task_id.to_string()),
            result: StepResult::Skipped,
            message: None,
            mapping_contexts: None,
            started_at: None,
            duration_us: None,
            changes: None,
        }
    }

    /// Create a skipped workflow step
    pub fn workflow_skipped(workflow_id: &str) -> Self {
        Self {
            workflow_id: workflow_id.to_string(),
            task_id: None,
            result: StepResult::Skipped,
            message: None,
            mapping_contexts: None,
            started_at: None,
            duration_us: None,
            changes: None,
        }
    }

    /// Set mapping context snapshots (for map tasks in trace mode)
    pub fn with_mapping_contexts(mut self, contexts: Vec<Value>) -> Self {
        self.mapping_contexts = Some(contexts);
        self
    }

    /// Attach task-body timing. Chains after [`Self::executed`].
    pub fn with_timing(mut self, started_at: DateTime<Utc>, duration_us: u64) -> Self {
        self.started_at = Some(started_at);
        self.duration_us = Some(duration_us);
        self
    }

    /// Attach this task's own diff. Chains after [`Self::executed`].
    pub fn with_changes(mut self, changes: Vec<Change>) -> Self {
        self.changes = Some(changes);
        self
    }
}

/// Complete execution trace containing all steps
///
/// `#[non_exhaustive]`: construct with [`ExecutionTrace::new`] or
/// [`ExecutionTrace::with_options`]. `steps` stays public for reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ExecutionTrace {
    /// All execution steps in order
    pub steps: Vec<ExecutionStep>,

    /// Set when `max_snapshot_bytes` was exceeded. Serialized only when `true`,
    /// so a complete trace keeps the historical wire shape.
    #[serde(default, skip_serializing_if = "is_false")]
    truncated: bool,

    /// Capture policy. In-memory only — a deserialized trace carries the
    /// default, which is why `options()` is documented as the policy this trace
    /// *records* with rather than the one it was recorded with.
    #[serde(skip)]
    options: TraceOptions,

    /// Pre-split `options.redact_paths`, so the split cost is paid once.
    #[serde(skip)]
    redact_segments: Vec<Vec<String>>,

    /// Approximate accumulated in-memory size of snapshots taken so far.
    #[serde(skip)]
    snapshot_bytes: usize,
}

impl ExecutionTrace {
    /// Create a new empty execution trace with default capture policy.
    pub fn new() -> Self {
        Self::with_options(TraceOptions::default())
    }

    /// Empty trace that records according to `options`.
    pub fn with_options(options: TraceOptions) -> Self {
        Self {
            steps: Vec::new(),
            truncated: false,
            redact_segments: options.redact_segments(),
            options,
            snapshot_bytes: 0,
        }
    }

    /// The capture policy this trace records with.
    pub fn options(&self) -> &TraceOptions {
        &self.options
    }

    /// Whether [`TraceOptions::max_snapshot_bytes`] was exceeded, and one or
    /// more executed steps therefore carry `message: None`, missing mapping
    /// contexts, or both.
    ///
    /// The budget is shared across both: with [`TraceOptions::snapshots`] off
    /// but [`TraceOptions::mapping_contexts`] on, this can still return `true`
    /// from the mapping-context term alone, even though no step was ever going
    /// to carry a `message`.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Add a step to the trace
    pub fn add_step(&mut self, step: ExecutionStep) {
        self.steps.push(step);
    }

    /// Record an executed step under this trace's capture policy.
    ///
    /// Owns every policy decision — snapshot scope, pruning clone, audit-trail
    /// scope, budget check, timing and diff — so the executor's two dispatch
    /// sites do not duplicate any of it.
    pub(crate) fn add_executed_step(
        &mut self,
        workflow_id: &str,
        task_id: &str,
        message: &Message,
        started_at: DateTime<Utc>,
        duration_us: u64,
        mapping_contexts: Option<Vec<Value>>,
    ) {
        let mut step = ExecutionStep {
            workflow_id: workflow_id.to_string(),
            task_id: Some(task_id.to_string()),
            result: StepResult::Executed,
            message: None,
            mapping_contexts: None,
            started_at: Some(started_at),
            duration_us: Some(duration_us),
            changes: if self.options.changes {
                // Derived from this task's own audit entry rather than
                // `audit_trail.last()` unconditionally — that is the
                // mis-attribution being fixed. A `TaskOutcome::Skip` records no
                // entry, so it correctly reports an empty diff instead of
                // inheriting the previous task's (or another workflow's).
                Some(
                    own_audit_entry(message, workflow_id, task_id)
                        .map(|e| e.changes.clone())
                        .unwrap_or_default(),
                )
            } else {
                None
            },
        };

        if self.options.snapshots {
            // Size is *probed* without cloning, so a snapshot that would not fit
            // is never built. Checking after the clone would be pointless: the
            // peak memory this budget exists to bound has already been paid.
            let projected = self.projected_snapshot_size(message, workflow_id, task_id);
            if self.would_exceed(projected) {
                self.truncated = true;
            } else {
                self.snapshot_bytes += projected;
                step.message = Some(self.build_snapshot(message, workflow_id, task_id));
            }
        }

        if self.options.mapping_contexts {
            if let Some(mut contexts) = mapping_contexts {
                // These arrive already cloned by the map function, so redact
                // first and then decide whether to retain them.
                for ctx in &mut contexts {
                    redact_json_in_place(ctx, &self.redact_segments);
                }
                let size: usize = contexts.iter().map(approx_json_size).sum();
                if self.would_exceed(size) {
                    self.truncated = true;
                } else {
                    self.snapshot_bytes += size;
                    step.mapping_contexts = Some(contexts);
                }
            }
        }

        self.steps.push(step);
    }

    /// Whether retaining `additional` bytes would cross a finite budget.
    #[inline]
    fn would_exceed(&self, additional: usize) -> bool {
        self.options.max_snapshot_bytes != 0
            && self.snapshot_bytes + additional > self.options.max_snapshot_bytes
    }

    /// Approximate size the snapshot for this step would occupy, computed
    /// without cloning anything.
    ///
    /// Must agree with what [`Self::build_snapshot`] actually retains; a unit
    /// test pins the two together.
    fn projected_snapshot_size(
        &self,
        message: &Message,
        workflow_id: &str,
        task_id: &str,
    ) -> usize {
        let mut size = redacted_size(&message.context, &self.redact_segments);
        for entry in self.scoped_audit_trail(message, workflow_id, task_id) {
            size += NODE_SIZE;
            for change in &entry.changes {
                size += change.path.len()
                    + approx_owned_size(&change.old_value)
                    + approx_owned_size(&change.new_value);
            }
        }
        size
    }

    /// The audit entries this step's snapshot will carry, per
    /// [`TraceOptions::snapshot_audit_trail`].
    fn scoped_audit_trail<'m>(
        &self,
        message: &'m Message,
        workflow_id: &str,
        task_id: &str,
    ) -> Vec<&'m AuditTrail> {
        match self.options.snapshot_audit_trail {
            AuditTrailScope::Full => message.audit_trail.iter().collect(),
            AuditTrailScope::Own => own_audit_entry(message, workflow_id, task_id)
                .map(|e| vec![e])
                .unwrap_or_default(),
            AuditTrailScope::None => Vec::new(),
        }
    }

    /// Build this step's snapshot: context pruned per `redact_paths`, audit
    /// trail scoped per `snapshot_audit_trail`. Returns the approximate
    /// in-memory size charged to the budget.
    ///
    /// `payload` is an `Arc`, so it is shared rather than deep-cloned here —
    /// same as the derived `Message::clone` this replaces.
    fn build_snapshot(&self, message: &Message, workflow_id: &str, task_id: &str) -> Message {
        let (context, _) = redacting_clone(&message.context, &self.redact_segments);
        let audit_trail: Vec<AuditTrail> = self
            .scoped_audit_trail(message, workflow_id, task_id)
            .into_iter()
            .cloned()
            .collect();

        Message {
            id: message.id.clone(),
            payload: Arc::clone(&message.payload),
            context,
            audit_trail,
            errors: message.errors.clone(),
            capture_changes: message.capture_changes,
            routing_bucket: message.routing_bucket,
        }
    }

    /// Get the final message (from the last executed step)
    ///
    /// Returns `None` when [`TraceOptions::snapshots`] is off, or when every
    /// executed step was recorded after the snapshot budget was exceeded.
    pub fn final_message(&self) -> Option<&Message> {
        self.steps
            .iter()
            .rev()
            .find(|s| s.result == StepResult::Executed)
            .and_then(|s| s.message.as_ref())
    }

    /// Check if execution was successful (no errors in final message)
    ///
    /// Degenerates to `true` when there is no snapshot to inspect — with
    /// [`TraceOptions::snapshots`] off, read `Message::errors` on the message you
    /// passed in instead.
    pub fn is_success(&self) -> bool {
        self.final_message()
            .map(|m| m.errors.is_empty())
            .unwrap_or(true)
    }

    /// Get number of executed steps
    pub fn executed_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.result == StepResult::Executed)
            .count()
    }

    /// Get number of skipped steps
    pub fn skipped_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.result == StepResult::Skipped)
            .count()
    }
}

impl Default for ExecutionTrace {
    fn default() -> Self {
        Self::new()
    }
}

/// The audit entry this task produced, if any.
///
/// The executor pushes at most one entry per task and pushes it immediately, so
/// the last entry is this task's exactly when both its workflow and task id
/// match. A `TaskOutcome::Skip` records none — which is why reading
/// `audit_trail.last()` unconditionally mis-attributes the previous task's diff
/// to a skipped step. Comparing `task_id` alone is not enough either: two
/// workflows can share a task id (or the same task can run again in a later
/// workflow), so a `Skip` right after a same-named task in a different
/// workflow would otherwise inherit that other workflow's entry.
#[inline]
fn own_audit_entry<'m>(
    message: &'m Message,
    workflow_id: &str,
    task_id: &str,
) -> Option<&'m AuditTrail> {
    match message.audit_trail.last() {
        Some(entry)
            if entry.task_id.as_ref() == task_id && entry.workflow_id.as_ref() == workflow_id =>
        {
            Some(entry)
        }
        _ => None,
    }
}

/// Microseconds between two non-monotonic clock reads, clamped at `0`.
///
/// `Utc::now()` can step backwards, and `num_microseconds()` returns `None` on
/// overflow; both collapse to `0` rather than panicking or wrapping.
#[inline]
pub(crate) fn duration_us_between(start: DateTime<Utc>, end: DateTime<Utc>) -> u64 {
    (end - start)
        .num_microseconds()
        .unwrap_or(0)
        .max(0)
        .try_into()
        .unwrap_or(0)
}

/// Deep-clone `value`, substituting `OwnedDataValue::Null` for every subtree
/// named by `paths`, and return the approximate in-memory size of the result.
///
/// `paths` holds the path suffixes still in play at this node; an empty suffix
/// means this node is itself a redaction target, so its subtree is never
/// cloned. A suffix that matches nothing is dropped, which is what makes an
/// unresolvable path a no-op rather than a `set_nested_value`-style create.
fn redacting_clone(value: &OwnedDataValue, paths: &[Vec<String>]) -> (OwnedDataValue, usize) {
    let refs: Vec<&[String]> = paths.iter().map(|p| p.as_slice()).collect();
    redacting_clone_inner(value, &refs)
}

/// Narrow `paths` to the child suffixes that still apply under object key
/// `key` — the entries whose head segment (after the `#`-prefix escape)
/// matches `key`, each with that head segment dropped. Shared by every
/// redact/size walker below so the "does this path element apply here"
/// filter has one definition instead of one copy per walker per value type.
fn narrow_for_object_key<'a>(paths: &[&'a [String]], key: &str) -> Vec<&'a [String]> {
    paths
        .iter()
        .filter(|p| strip_hash_prefix(&p[0]) == key)
        .map(|p| &p[1..])
        .collect()
}

/// Same as [`narrow_for_object_key`] but for an array index: the entries
/// whose head segment parses as `idx`.
fn narrow_for_array_index<'a>(paths: &[&'a [String]], idx: usize) -> Vec<&'a [String]> {
    paths
        .iter()
        .filter(|p| p[0].parse::<usize>() == Ok(idx))
        .map(|p| &p[1..])
        .collect()
}

fn redacting_clone_inner(value: &OwnedDataValue, paths: &[&[String]]) -> (OwnedDataValue, usize) {
    // An exhausted suffix names this node: redact without descending.
    if paths.iter().any(|p| p.is_empty()) {
        return (OwnedDataValue::Null, NODE_SIZE);
    }

    match value {
        OwnedDataValue::Object(pairs) => {
            let mut out = Vec::with_capacity(pairs.len());
            let mut size = NODE_SIZE;
            for (key, child) in pairs {
                let sub = narrow_for_object_key(paths, key);
                let (cloned, child_size) = redacting_clone_inner(child, &sub);
                size += key.len() + child_size;
                out.push((key.clone(), cloned));
            }
            (OwnedDataValue::Object(out), size)
        }
        OwnedDataValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            let mut size = NODE_SIZE;
            for (idx, child) in items.iter().enumerate() {
                let sub = narrow_for_array_index(paths, idx);
                let (cloned, child_size) = redacting_clone_inner(child, &sub);
                size += child_size;
                out.push(cloned);
            }
            (OwnedDataValue::Array(out), size)
        }
        // Scalars: any leftover suffix cannot resolve, so it is dropped.
        OwnedDataValue::String(s) => (value.clone(), NODE_SIZE + s.len()),
        other => (other.clone(), NODE_SIZE),
    }
}

/// Size [`redacting_clone`] would produce, computed without cloning.
///
/// Exists so the snapshot budget can decline a capture *before* paying for it.
/// Must stay in step with `redacting_clone`; `redacted_size_agrees_with_redacting_clone`
/// pins that.
fn redacted_size(value: &OwnedDataValue, paths: &[Vec<String>]) -> usize {
    let refs: Vec<&[String]> = paths.iter().map(|p| p.as_slice()).collect();
    redacted_size_inner(value, &refs)
}

fn redacted_size_inner(value: &OwnedDataValue, paths: &[&[String]]) -> usize {
    if paths.iter().any(|p| p.is_empty()) {
        return NODE_SIZE;
    }
    match value {
        OwnedDataValue::Object(pairs) => {
            let mut size = NODE_SIZE;
            for (key, child) in pairs {
                let sub = narrow_for_object_key(paths, key);
                size += key.len() + redacted_size_inner(child, &sub);
            }
            size
        }
        OwnedDataValue::Array(items) => {
            let mut size = NODE_SIZE;
            for (idx, child) in items.iter().enumerate() {
                let sub = narrow_for_array_index(paths, idx);
                size += redacted_size_inner(child, &sub);
            }
            size
        }
        OwnedDataValue::String(s) => NODE_SIZE + s.len(),
        _ => NODE_SIZE,
    }
}

/// Approximate in-memory size of an `OwnedDataValue`, on the same scale as
/// [`redacting_clone`].
fn approx_owned_size(value: &OwnedDataValue) -> usize {
    match value {
        OwnedDataValue::Object(pairs) => {
            NODE_SIZE
                + pairs
                    .iter()
                    .map(|(k, v)| k.len() + approx_owned_size(v))
                    .sum::<usize>()
        }
        OwnedDataValue::Array(items) => {
            NODE_SIZE + items.iter().map(approx_owned_size).sum::<usize>()
        }
        OwnedDataValue::String(s) => NODE_SIZE + s.len(),
        _ => NODE_SIZE,
    }
}

/// Null out every subtree named by `paths` in a `serde_json::Value`, in place.
///
/// Mirrors [`redacting_clone`]'s path semantics so `mapping_contexts` are
/// redacted the same way message snapshots are. In place because the caller
/// already owns the value — there is no second clone to save.
fn redact_json_in_place(value: &mut Value, paths: &[Vec<String>]) {
    let refs: Vec<&[String]> = paths.iter().map(|p| p.as_slice()).collect();
    redact_json_inner(value, &refs);
}

fn redact_json_inner(value: &mut Value, paths: &[&[String]]) {
    if paths.is_empty() {
        return;
    }
    if paths.iter().any(|p| p.is_empty()) {
        *value = Value::Null;
        return;
    }

    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                let sub = narrow_for_object_key(paths, key);
                redact_json_inner(child, &sub);
            }
        }
        Value::Array(items) => {
            for (idx, child) in items.iter_mut().enumerate() {
                let sub = narrow_for_array_index(paths, idx);
                redact_json_inner(child, &sub);
            }
        }
        _ => {}
    }
}

/// Approximate in-memory size of a `serde_json::Value`, on the same scale as
/// [`approx_owned_size`].
fn approx_json_size(value: &Value) -> usize {
    match value {
        Value::Object(map) => {
            NODE_SIZE
                + map
                    .iter()
                    .map(|(k, v)| k.len() + approx_json_size(v))
                    .sum::<usize>()
        }
        Value::Array(items) => NODE_SIZE + items.iter().map(approx_json_size).sum::<usize>(),
        Value::String(s) => NODE_SIZE + s.len(),
        _ => NODE_SIZE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dv(v: serde_json::Value) -> OwnedDataValue {
        OwnedDataValue::from(&v)
    }

    fn segments(paths: &[&str]) -> Vec<Vec<String>> {
        TraceOptions {
            redact_paths: paths.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
        .redact_segments()
    }

    #[test]
    fn test_step_result_serialization() {
        assert_eq!(
            serde_json::to_string(&StepResult::Executed).unwrap(),
            "\"executed\""
        );
        assert_eq!(
            serde_json::to_string(&StepResult::Skipped).unwrap(),
            "\"skipped\""
        );
    }

    #[test]
    fn test_execution_step_executed() {
        let message = Message::from_value(&json!({"test": "data"}));
        let step = ExecutionStep::executed("workflow1", "task1", &message);

        assert_eq!(step.workflow_id, "workflow1");
        assert_eq!(step.task_id, Some("task1".to_string()));
        assert_eq!(step.result, StepResult::Executed);
        assert!(step.message.is_some());
    }

    #[test]
    fn test_execution_step_task_skipped() {
        let step = ExecutionStep::task_skipped("workflow1", "task1");

        assert_eq!(step.workflow_id, "workflow1");
        assert_eq!(step.task_id, Some("task1".to_string()));
        assert_eq!(step.result, StepResult::Skipped);
        assert!(step.message.is_none());
    }

    #[test]
    fn test_execution_step_workflow_skipped() {
        let step = ExecutionStep::workflow_skipped("workflow1");

        assert_eq!(step.workflow_id, "workflow1");
        assert_eq!(step.task_id, None);
        assert_eq!(step.result, StepResult::Skipped);
        assert!(step.message.is_none());
    }

    #[test]
    fn test_execution_step_with_mapping_contexts() {
        let message = Message::from_value(&json!({"test": "data"}));
        let contexts = vec![json!({"data": {"a": 1}}), json!({"data": {"a": 1, "b": 2}})];

        let step = ExecutionStep::executed("workflow1", "task1", &message)
            .with_mapping_contexts(contexts.clone());

        assert_eq!(step.mapping_contexts, Some(contexts));

        // Verify serialization includes mapping_contexts
        let serialized = serde_json::to_value(&step).unwrap();
        assert!(serialized.get("mapping_contexts").is_some());
        assert_eq!(serialized["mapping_contexts"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_execution_step_without_mapping_contexts_serialization() {
        let message = Message::from_value(&json!({"test": "data"}));
        let step = ExecutionStep::executed("workflow1", "task1", &message);

        // Every optional field is None, so all are omitted in serialization.
        let serialized = serde_json::to_value(&step).unwrap();
        assert!(serialized.get("mapping_contexts").is_none());
        assert!(serialized.get("started_at").is_none());
        assert!(serialized.get("duration_us").is_none());
        assert!(serialized.get("changes").is_none());
    }

    #[test]
    fn test_execution_trace() {
        let mut trace = ExecutionTrace::new();
        let message = Message::from_value(&json!({"test": "data"}));

        trace.add_step(ExecutionStep::workflow_skipped("workflow0"));
        trace.add_step(ExecutionStep::executed("workflow1", "task1", &message));
        trace.add_step(ExecutionStep::task_skipped("workflow1", "task2"));

        assert_eq!(trace.steps.len(), 3);
        assert_eq!(trace.executed_count(), 1);
        assert_eq!(trace.skipped_count(), 2);
        assert!(trace.final_message().is_some());
        assert!(trace.is_success());
    }

    #[test]
    fn default_options_reproduce_historical_capture() {
        let o = TraceOptions::default();
        assert!(o.snapshots);
        assert!(o.mapping_contexts);
        assert!(!o.changes);
        assert_eq!(o.max_snapshot_bytes, 0);
        assert!(o.redact_paths.is_empty());
        assert_eq!(o.snapshot_audit_trail, AuditTrailScope::Full);
    }

    #[test]
    fn timings_only_drops_snapshots_and_keeps_the_diff() {
        let o = TraceOptions::timings_only();
        assert!(!o.snapshots);
        assert!(!o.mapping_contexts);
        assert!(o.changes);
    }

    #[test]
    fn a_complete_trace_does_not_serialize_the_truncated_flag() {
        let trace = ExecutionTrace::new();
        let serialized = serde_json::to_value(&trace).unwrap();
        assert!(
            serialized.get("truncated").is_none(),
            "a complete trace keeps the historical wire shape"
        );
        assert!(!trace.truncated());
    }

    #[test]
    fn a_trace_deserializes_from_a_payload_without_the_truncated_flag() {
        let trace: ExecutionTrace = serde_json::from_value(json!({ "steps": [] })).unwrap();
        assert!(!trace.truncated());
    }

    #[test]
    fn duration_clamps_a_backward_clock_step_to_zero() {
        let start = Utc::now();
        let earlier = start - chrono::Duration::seconds(5);
        assert_eq!(duration_us_between(start, earlier), 0);
        assert_eq!(duration_us_between(start, start), 0);
        assert_eq!(
            duration_us_between(start, start + chrono::Duration::microseconds(1500)),
            1500
        );
    }

    #[test]
    fn redaction_nulls_only_the_named_subtree() {
        let ctx = dv(json!({"data": {"secret": {"k": "v"}, "keep": 1}}));
        let (out, _) = redacting_clone(&ctx, &segments(&["data.secret"]));
        assert_eq!(
            serde_json::Value::from(&out),
            json!({"data": {"secret": null, "keep": 1}})
        );
    }

    #[test]
    fn redaction_of_an_unresolvable_path_creates_nothing() {
        // `set_nested_value` would pad the array to index 99 with nulls; this
        // must not.
        let ctx = dv(json!({"data": {"items": [1, 2, 3]}}));
        let (out, _) = redacting_clone(&ctx, &segments(&["data.items.99"]));
        assert_eq!(
            serde_json::Value::from(&out),
            json!({"data": {"items": [1, 2, 3]}})
        );
    }

    #[test]
    fn redaction_through_a_non_container_is_a_noop() {
        let ctx = dv(json!({"data": {"name": "alice"}}));
        let (out, _) = redacting_clone(&ctx, &segments(&["data.name.first"]));
        assert_eq!(
            serde_json::Value::from(&out),
            json!({"data": {"name": "alice"}})
        );
    }

    #[test]
    fn an_empty_redact_path_is_ignored() {
        let ctx = dv(json!({"data": {"a": 1}}));
        let (out, _) = redacting_clone(&ctx, &segments(&[""]));
        assert_eq!(serde_json::Value::from(&out), json!({"data": {"a": 1}}));
    }

    #[test]
    fn redaction_honours_the_hash_escape() {
        // `data.#20` names the object key "20"; `data.20` indexes an array.
        let obj = dv(json!({"data": {"20": "secret", "other": 1}}));
        let (out, _) = redacting_clone(&obj, &segments(&["data.#20"]));
        assert_eq!(
            serde_json::Value::from(&out),
            json!({"data": {"20": null, "other": 1}})
        );

        let arr = dv(json!({"data": [0, 1, 2]}));
        let (out, _) = redacting_clone(&arr, &segments(&["data.1"]));
        assert_eq!(serde_json::Value::from(&out), json!({"data": [0, null, 2]}));

        // The escaped form does not index an array.
        let (out, _) = redacting_clone(&arr, &segments(&["data.#1"]));
        assert_eq!(serde_json::Value::from(&out), json!({"data": [0, 1, 2]}));
    }

    #[test]
    fn nested_and_duplicated_redact_paths_are_safe() {
        let ctx = dv(json!({"data": {"a": {"b": 1, "c": 2}}}));

        let (out, _) = redacting_clone(&ctx, &segments(&["data.a", "data.a.b"]));
        assert_eq!(serde_json::Value::from(&out), json!({"data": {"a": null}}));

        let (out, _) = redacting_clone(&ctx, &segments(&["data.a", "data.a"]));
        assert_eq!(serde_json::Value::from(&out), json!({"data": {"a": null}}));
    }

    #[test]
    fn redaction_matches_unicode_keys_and_sizes_strings_by_bytes() {
        let ctx = dv(json!({"data": {"café": "secret", "keep": "née"}}));
        let (out, size) = redacting_clone(&ctx, &segments(&["data.café"]));
        assert_eq!(
            serde_json::Value::from(&out),
            json!({"data": {"café": null, "keep": "née"}})
        );

        // "née" is 4 bytes, 3 chars — the budget counts bytes.
        let (_, unredacted) = redacting_clone(&ctx, &segments(&[]));
        assert!(unredacted > size, "redacting must lower the counted size");
        assert!(
            approx_owned_size(&dv(json!("née"))) == NODE_SIZE + 4,
            "str::len() bytes, not chars().count()"
        );
    }

    #[test]
    fn redacted_size_agrees_with_redacting_clone() {
        // The budget probes with `redacted_size` and then builds with
        // `redacting_clone`; if they drift, the budget stops meaning anything.
        let shapes = [
            json!({}),
            json!({"data": {"a": 1, "b": "hello"}}),
            json!({"data": {"items": [1, "two", {"three": 3}], "nested": {"x": {"y": "z"}}}}),
            json!({"data": {"secret": {"deep": [1, 2, 3]}, "keep": "café"}}),
        ];
        let path_sets: [&[&str]; 4] = [&[], &["data.secret"], &["data.items.1"], &["data.nope"]];

        for shape in &shapes {
            for paths in path_sets {
                let v = dv(shape.clone());
                let segs = segments(paths);
                let (_, cloned_size) = redacting_clone(&v, &segs);
                assert_eq!(
                    redacted_size(&v, &segs),
                    cloned_size,
                    "probe and clone disagree for {shape:?} with {paths:?}"
                );
            }
        }
    }

    #[test]
    fn redaction_applies_to_mapping_contexts_too() {
        let mut ctx = json!({"data": {"secret": {"k": "v"}, "keep": 1}});
        redact_json_in_place(&mut ctx, &segments(&["data.secret"]));
        assert_eq!(ctx, json!({"data": {"secret": null, "keep": 1}}));
    }

    #[test]
    fn json_redaction_shares_the_owned_path_semantics() {
        let mut arr = json!({"data": {"items": [1, 2, 3]}});
        redact_json_in_place(&mut arr, &segments(&["data.items.99"]));
        assert_eq!(arr, json!({"data": {"items": [1, 2, 3]}}));

        let mut scalar = json!({"data": {"name": "alice"}});
        redact_json_in_place(&mut scalar, &segments(&["data.name.first"]));
        assert_eq!(scalar, json!({"data": {"name": "alice"}}));

        let mut hash = json!({"data": {"20": "secret"}});
        redact_json_in_place(&mut hash, &segments(&["data.#20"]));
        assert_eq!(hash, json!({"data": {"20": null}}));
    }
}
