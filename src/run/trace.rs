//! The trace as a run builds it: [`Capture`], which redacts and
//! charges `initial` and each step's changes, and drops the changes past the
//! byte budget, a `map` step's mapping ends with them.
//!
//! Redaction is v3's pruning rule (`redacting_clone`), over a [`Path`]'s
//! segments: on an object a segment is its key (one leading `#` stripped),
//! on an array its index, and a path that resolves nothing creates nothing.
//! A change is redacted by where its path sits against each redact path: at
//! or below one, it becomes a write of `null` at its own prefix of the
//! redact path's length, so no key below a redact path reaches the trace and
//! a replay reads `null` there at every step, its parent absent from
//! `initial` or not; above one, the rest of the redact path is pruned inside
//! both values. The size is v3's scale too: one
//! machine word per node, plus a string's length and a key's.

use crate::base::{Change, Context};
use crate::expr::Path;
use crate::expr::path::Seg;
use crate::observe::{Trace, TraceOptions, TraceStep};
use datavalue::OwnedDataValue;
use std::sync::Arc;

/// What one node costs: a machine word, as v3's `NODE_SIZE`.
const NODE_SIZE: usize = size_of::<usize>();

/// A run's trace while the run makes it.
#[derive(Debug)]
pub(crate) struct Capture {
    options: TraceOptions,
    steps: Vec<TraceStep>,
    /// What `initial` and the changes kept so far cost, counted only under a
    /// byte budget.
    bytes: usize,
    truncated: bool,
    initial: Context,
    /// The engine's writes made for the next recorded step, before its
    /// call's own: a fan-out's `into` pre-fill, which its first element's
    /// step takes. Filled only while the trace keeps changes.
    prelude: Vec<Change>,
    /// Where each mapping of the `map` call running ends in its changes,
    /// lent to the call while the trace keeps changes, and moved
    /// onto its step by the recorder.
    mapping_ends: Vec<u32>,
}

impl Capture {
    /// A trace under `options` starting from `context`, the object holding
    /// the three roots: `initial` is its redacted copy, charged under a byte
    /// budget. A budget `initial` alone crosses truncates from the start.
    pub(crate) fn new(options: TraceOptions, context: &OwnedDataValue) -> Self {
        let paths: Vec<&[Seg]> = options.redact.iter().map(Path::segs).collect();
        let sized = options.max_bytes.is_some();
        let (initial, bytes) = redacting_clone(context, &paths, sized);
        let bytes = if sized { bytes } else { 0 };
        let truncated = options.max_bytes.is_some_and(|max| bytes > max);
        Self {
            options,
            steps: Vec::new(),
            bytes,
            truncated,
            initial: Context::from_root(initial),
            prelude: Vec::new(),
            mapping_ends: Vec::new(),
        }
    }

    /// Keeps `change` for the next recorded step, before its call's own.
    pub(crate) fn push_prelude(&mut self, change: Change) {
        self.prelude.push(change);
    }

    /// The engine's writes kept for this step, taken.
    pub(crate) fn take_prelude(&mut self) -> Vec<Change> {
        std::mem::take(&mut self.prelude)
    }

    /// The mapping ends a `map` call notes, lent to it: each an index into
    /// the call's own changes.
    pub(crate) fn mapping_ends(&mut self) -> &mut Vec<u32> {
        &mut self.mapping_ends
    }

    /// The mapping ends the call noted, taken, each moved past the
    /// `offset` changes its step carries before the call's own (the
    /// prelude). Empty, and allocating nothing, for any call but a traced
    /// `map`.
    pub(crate) fn take_mapping_ends(&mut self, offset: usize) -> Vec<u32> {
        let mut ends = std::mem::take(&mut self.mapping_ends);
        if offset > 0 {
            let offset = u32::try_from(offset).unwrap_or(u32::MAX);
            for end in &mut ends {
                *end = end.saturating_add(offset);
            }
        }
        ends
    }

    /// Whether a step's changes are kept: the trace records changes and is
    /// not truncated. Only then does the engine read an old value for its
    /// own writes.
    pub(crate) fn changes(&self) -> bool {
        self.options.changes && !self.truncated
    }

    /// Whether the trace times its calls.
    pub(crate) fn timings(&self) -> bool {
        self.options.timings
    }

    /// Pushes `step`: its changes redacted and, under a byte budget,
    /// charged after the redaction, or dropped when the trace keeps none or
    /// they would cross the budget, which truncates the trace from this step
    /// on. Its mapping ends go with its changes: they index nothing once
    /// those are dropped. Without a budget nothing is sized (A M-6).
    pub(crate) fn push(&mut self, mut step: TraceStep) {
        if !self.changes() {
            step.changes.clear();
            step.mapping_ends.clear();
        } else if !step.changes.is_empty() {
            for change in &mut step.changes {
                self.redact(change);
            }
            if let Some(max) = self.options.max_bytes {
                let mut remaining = max.saturating_sub(self.bytes);
                let fits = step
                    .changes
                    .iter()
                    .all(|c| charge_change(c, &mut remaining));
                if !fits {
                    self.truncated = true;
                    step.changes.clear();
                    step.mapping_ends.clear();
                } else {
                    self.bytes = max - remaining;
                }
            }
        }
        self.steps.push(step);
    }

    /// The finished trace.
    pub(crate) fn finish(self) -> Trace {
        Trace {
            initial: self.initial,
            steps: self.steps,
            truncated: self.truncated,
        }
    }

    /// `change` redacted in place, by each redact path.
    fn redact(&self, change: &mut Change) {
        if self.options.redact.is_empty() {
            return;
        }
        // Every change's path is a `Path`'s spelling; one that is not is
        // nulled rather than let through.
        let Ok(parsed) = change.path.parse::<Path>() else {
            change.old_value = OwnedDataValue::Null;
            change.new_value = OwnedDataValue::Null;
            return;
        };
        let at = parsed.segs();
        let mut inside = Vec::new();
        for redact in &self.options.redact {
            let redact = redact.segs();
            let shared = redact.len().min(at.len());
            if !redact[..shared]
                .iter()
                .zip(&at[..shared])
                .all(|(a, b)| Seg::may_alias(a, b))
            {
                continue;
            }
            if redact.len() <= at.len() {
                // At the change's own prefix, so the container kind
                // a replay writes through is the real one, and as a write of
                // `null`, a removal's included.
                if redact.len() < at.len() {
                    change.path = Arc::clone(parsed.prefix(redact.len()).dotted());
                }
                change.old_value = OwnedDataValue::Null;
                change.new_value = OwnedDataValue::Null;
                change.removed = false;
                change.replaced = false;
                return;
            }
            inside.push(&redact[at.len()..]);
        }
        if !inside.is_empty() {
            prune(&mut change.old_value, &inside);
            prune(&mut change.new_value, &inside);
        }
    }
}

/// The suffixes of `paths` that go on below object key `key`.
fn below_key<'s>(paths: &[&'s [Seg]], key: &str) -> Vec<&'s [Seg]> {
    paths
        .iter()
        .filter(|p| *p[0].key == *key)
        .map(|p| &p[1..])
        .collect()
}

/// The suffixes of `paths` that go on below array element `index`.
fn below_index<'s>(paths: &[&'s [Seg]], index: usize) -> Vec<&'s [Seg]> {
    paths
        .iter()
        .filter(|p| p[0].index == Some(index))
        .map(|p| &p[1..])
        .collect()
}

/// v3's `redacting_clone`: `value` cloned with `null` for every node a path
/// in `paths` names, which is never cloned, and the clone's size, which,
/// unless `sized`, leaves out every subtree no path enters, so it is not the
/// size (no budget reads it then). An empty suffix names the node it
/// reached; a suffix that matches nothing is dropped, so a path that
/// resolves nothing creates nothing.
fn redacting_clone(
    value: &OwnedDataValue,
    paths: &[&[Seg]],
    sized: bool,
) -> (OwnedDataValue, usize) {
    let measure = |v: &OwnedDataValue| if sized { approx_size(v) } else { 0 };
    if paths.iter().any(|p| p.is_empty()) {
        return (OwnedDataValue::Null, NODE_SIZE);
    }
    if paths.is_empty() {
        return (value.clone(), measure(value));
    }
    match value {
        OwnedDataValue::Object(pairs) => {
            let mut size = NODE_SIZE;
            let pairs = pairs
                .iter()
                .map(|(key, child)| {
                    let (child, child_size) = redacting_clone(child, &below_key(paths, key), sized);
                    size += key.len() + child_size;
                    (key.clone(), child)
                })
                .collect();
            (OwnedDataValue::Object(pairs), size)
        }
        OwnedDataValue::Array(items) => {
            let mut size = NODE_SIZE;
            let items = items
                .iter()
                .enumerate()
                .map(|(index, child)| {
                    let (child, child_size) =
                        redacting_clone(child, &below_index(paths, index), sized);
                    size += child_size;
                    child
                })
                .collect();
            (OwnedDataValue::Array(items), size)
        }
        other => (other.clone(), measure(other)),
    }
}

/// [`redacting_clone`] in place, for a value the trace already owns.
fn prune(value: &mut OwnedDataValue, paths: &[&[Seg]]) {
    if paths.is_empty() {
        return;
    }
    if paths.iter().any(|p| p.is_empty()) {
        *value = OwnedDataValue::Null;
        return;
    }
    match value {
        OwnedDataValue::Object(pairs) => {
            for (key, child) in pairs {
                prune(child, &below_key(paths, key));
            }
        }
        OwnedDataValue::Array(items) => {
            for (index, child) in items.iter_mut().enumerate() {
                prune(child, &below_index(paths, index));
            }
        }
        _ => {}
    }
}

/// One word per node, plus a string's length and each key's, measured by the
/// shared [`crate::state::charge_size`] walker.
fn approx_size(value: &OwnedDataValue) -> usize {
    let mut budget = usize::MAX;
    let mut visited = 0;
    let _ = crate::state::charge_size(value, NODE_SIZE, &mut budget, &mut visited);
    usize::MAX - budget
}

/// Charges a change (its path's length and both its values) against `budget`,
/// stopping as soon as `budget` is exceeded.
fn charge_change(change: &Change, budget: &mut usize) -> bool {
    if change.path.len() > *budget {
        return false;
    }
    *budget -= change.path.len();
    let mut visited = 0;
    crate::state::charge_size(&change.old_value, NODE_SIZE, budget, &mut visited)
        && crate::state::charge_size(&change.new_value, NODE_SIZE, budget, &mut visited)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::WorkflowIx;
    use crate::observe::StepResult;
    use serde_json::{Value, json};
    use std::sync::Arc;

    fn dv(v: Value) -> OwnedDataValue {
        OwnedDataValue::from(&v)
    }

    fn text(v: &OwnedDataValue) -> String {
        serde_json::to_string(v).unwrap()
    }

    fn paths(raw: &[&str]) -> Vec<Path> {
        raw.iter().map(|p| p.parse().unwrap()).collect()
    }

    fn capture(redact: &[&str], context: &Value) -> Capture {
        let options = TraceOptions::default().redact(paths(redact));
        Capture::new(options, &dv(context.clone()))
    }

    #[test]
    fn redacting_clone_prunes_by_key_and_index_and_creates_nothing() {
        // The keys are written in the order the expectations read them, so the
        // test means the same whether serde_json sorts keys or keeps their order.
        let context = json!({"data": {"#7": "x", "7": "y", "a": {"b": 1, "c": [10, {"d": 2}]}},
                             "metadata": {}, "temp_data": {}});
        for (redact, expected) in [
            (
                vec!["data.a.b"],
                r##"{"#7":"x","7":"y","a":{"b":null,"c":[10,{"d":2}]}}"##,
            ),
            (
                vec!["data.a.c.1.d"],
                r##"{"#7":"x","7":"y","a":{"b":1,"c":[10,{"d":null}]}}"##,
            ),
            (
                vec!["data.a.c.0"],
                r##"{"#7":"x","7":"y","a":{"b":1,"c":[null,{"d":2}]}}"##,
            ),
            // One `#` is stripped from a key: `##7` names the key `#7`.
            (
                vec!["data.##7"],
                r##"{"#7":null,"7":"y","a":{"b":1,"c":[10,{"d":2}]}}"##,
            ),
            (
                vec!["data.#7"],
                r##"{"#7":"x","7":null,"a":{"b":1,"c":[10,{"d":2}]}}"##,
            ),
            // Through a scalar, and to nowhere: nothing.
            (
                vec!["data.a.b.z", "data.nope.deep"],
                r##"{"#7":"x","7":"y","a":{"b":1,"c":[10,{"d":2}]}}"##,
            ),
        ] {
            let c = capture(&redact, &context);
            assert_eq!(text(c.initial.data()), expected, "{redact:?}");
        }
    }

    #[test]
    fn the_size_is_v3s_scale_and_agrees_with_the_clone() {
        assert_eq!(approx_size(&dv(json!("née"))), NODE_SIZE + 4);
        assert_eq!(
            approx_size(&dv(json!({"ab": [1, "xyz"]}))),
            NODE_SIZE * 4 + 2 + 3
        );
        let context = dv(json!({"data": {"s": "secret", "k": [1, 2]}, "metadata": {}}));
        let redact = paths(&["data.s"]);
        let segs: Vec<&[Seg]> = redact.iter().map(Path::segs).collect();
        let (clone, bytes) = redacting_clone(&context, &segs, true);
        assert_eq!(bytes, approx_size(&clone));
        assert_eq!(bytes, approx_size(&context) - "secret".len());
    }

    #[test]
    fn nothing_is_sized_without_a_budget() {
        // A M-6: `initial` and the changes are charged only under
        // `max_bytes`; a trace without one counts nothing.
        let context = dv(json!({"data": {"s": "x".repeat(64)}, "metadata": {}}));
        let mut c = Capture::new(TraceOptions::default(), &context);
        c.push(step(vec![Change::write(
            "data.a",
            dv(json!(null)),
            dv(json!("y".repeat(64))),
        )]));
        assert_eq!(c.bytes, 0);
        let budget = TraceOptions::default().max_bytes(1 << 20);
        assert_eq!(Capture::new(budget, &context).bytes, approx_size(&context));
    }

    fn step(changes: Vec<Change>) -> TraceStep {
        let mut step = TraceStep::new(WorkflowIx::new(0), &Arc::from("w"), StepResult::Executed);
        step.changes = changes;
        step
    }

    #[test]
    fn a_change_is_nulled_at_or_below_a_redact_path_and_pruned_above_one() {
        let mut c = capture(&["data.user.password", "data.list.1"], &json!({}));
        let write = |path: &str, old: Value, new: Value| Change::write(path, dv(old), dv(new));
        c.push(step(vec![
            write("data.user.password", json!("a"), json!("b")),
            write("data.user.password.deep", json!(null), json!(1)),
            write(
                "data.user",
                json!({"name": "n"}),
                json!({"name": "m", "password": "p"}),
            ),
            write(
                "data",
                json!({}),
                json!({"list": [0, 1, 2], "user": {"password": "p"}}),
            ),
            write("data.list", json!(null), json!([0, 1])),
            write("data.other", json!(null), json!("p")),
            Change::removal("data.user.password", dv(json!("p"))),
        ]));
        let trace = c.finish();
        let seen: Vec<_> = trace.steps[0]
            .changes
            .iter()
            .map(|c| {
                format!(
                    "{} {} {} {}",
                    c.path,
                    text(&c.old_value),
                    text(&c.new_value),
                    c.removed
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                "data.user.password null null false",
                // Below a redact path: at the redact path's length.
                "data.user.password null null false",
                r#"data.user {"name":"n"} {"name":"m","password":null} false"#,
                r#"data {} {"list":[0,null,2],"user":{"password":null}} false"#,
                "data.list null [0,null] false",
                r#"data.other null "p" false"#,
                // A removal: a write of `null`, so the path reads `null`.
                "data.user.password null null false",
            ]
        );
    }

    #[test]
    fn the_budget_truncates_from_the_crossing_step_and_stays_truncated() {
        let options = TraceOptions::default().max_bytes(100);
        let mut c = Capture::new(options, &dv(json!({})));
        assert_eq!(c.bytes, NODE_SIZE);
        let small = || vec![Change::write("data.a", dv(json!(null)), dv(json!(1)))];
        let big = || {
            vec![Change::write(
                "data.b",
                dv(json!(null)),
                dv(json!("x".repeat(80))),
            )]
        };
        c.push(step(small()));
        assert!(c.changes());
        c.push(step(big()));
        assert!(!c.changes(), "crossed");
        c.push(step(small()));
        let trace = c.finish();
        assert!(trace.truncated);
        let kept: Vec<_> = trace.steps.iter().map(|s| s.changes.len()).collect();
        assert_eq!(kept, [1, 0, 0]);

        let tiny = TraceOptions::default().max_bytes(NODE_SIZE - 1);
        assert!(
            !Capture::new(tiny, &dv(json!({}))).changes(),
            "initial alone crosses"
        );
    }
}
