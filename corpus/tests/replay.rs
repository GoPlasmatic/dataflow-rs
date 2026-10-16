//! What v3's own traces do not rebuild by replaying their changes, measured
//! over the whole corpus in phase 1 and pinned here. v4's trace is the initial
//! context plus each step's changes (R17), so each group below is something
//! v4 must record, or its runner derive, to rebuild a snapshot. A case that
//! adds a path here must add it to the right group, deliberately.

use dataflow_corpus::replay::unrebuilt;
use dataflow_corpus::{Json, load_cases, recording_path};
use std::collections::BTreeSet;

/// Stamped at the start of a run, before any step: v4's `initial` must be
/// taken after them.
const STAMPS: &[&str] = &[
    "metadata.processed_at",
    "metadata.engine_version",
    "metadata.channel",
];

/// Written by the engine itself, never as a step's change: progress after
/// every task, error-context records, loop counters and `over` bindings.
const ENGINE_WRITES: &[&str] = &[
    "metadata.progress",
    "metadata.failures",
    "temp_data.i",
    "temp_data.item",
    "temp_data.bi",
    "temp_data.bitem",
    "temp_data.ei",
];

/// A fan-out's `into`, which the fold first fills with one `null` per element.
const INTO_PREFILL: &[&str] = &[
    "data.results",
    "data.fanned",
    "data.terminal_results",
    "data.halt_on_results",
];

/// A handler's writes that stay in the message without a change: v3 keeps them
/// when the handler returns `Err` (break 1) or `Skip`.
const UNAUDITED_HANDLER_WRITES: &[&str] = &["data.partial", "data.skipper_wrote"];

/// Runs whose trace carries no changes at all, because `capture_changes` is off
/// or the audit mode keeps nothing. R45 makes a v4 trace record its own changes.
const TRACE_WITHOUT_CHANGES: &[&str] = &["data.a", "data.b", "data.x", "data.y"];

#[test]
fn replay_falls_short_exactly_where_measured() {
    let mut found = BTreeSet::new();
    for (path, case) in load_cases() {
        let text = std::fs::read_to_string(recording_path(&path)).expect("every case is recorded");
        found.extend(unrebuilt(
            &case,
            &Json::from_text(&text).expect("a recording parses"),
        ));
    }
    let pinned: BTreeSet<String> = [
        STAMPS,
        ENGINE_WRITES,
        INTO_PREFILL,
        UNAUDITED_HANDLER_WRITES,
        TRACE_WITHOUT_CHANGES,
    ]
    .concat()
    .into_iter()
    .map(str::to_owned)
    .collect();
    let new: Vec<_> = found.difference(&pinned).collect();
    let gone: Vec<_> = pinned.difference(&found).collect();
    assert!(
        new.is_empty() && gone.is_empty(),
        "replay gaps changed. New: {new:?}. No longer seen: {gone:?}. \
         Run `cargo run -p dataflow-corpus --bin replay-report` and classify them."
    );
}
