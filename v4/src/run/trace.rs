//! The trace as a run builds it (I105): [`Capture`], which redacts and
//! charges `initial` and each step's changes, and drops the changes past the
//! byte budget.
//!
//! Redaction is v3's pruning rule (`redacting_clone`), over a [`Path`]'s
//! segments: on an object a segment is its key (one leading `#` stripped),
//! on an array its index, and a path that resolves nothing creates nothing.
//! A change is redacted by where its path sits against each redact path: at
//! or below one, both its values read `null`; above one, the rest of the
//! redact path is pruned inside both values. The size is v3's scale too: one
//! machine word per node, plus a string's length and a key's.

use crate::base::Change;
use crate::expr::Path;
use crate::expr::path::Seg;
use crate::observe::{Trace, TraceOptions, TraceStep};
use crate::run::Context;
use datavalue::OwnedDataValue;

/// What one node costs: a machine word, as v3's `NODE_SIZE`.
const NODE_SIZE: usize = size_of::<usize>();

/// A run's trace while the run makes it.
#[derive(Debug)]
pub(crate) struct Capture {
    options: TraceOptions,
    steps: Vec<TraceStep>,
    /// What `initial` and the changes kept so far cost.
    bytes: usize,
    truncated: bool,
    initial: Context,
    /// The engine's writes made for the next recorded step, before its
    /// call's own: a fan-out's `into` pre-fill, which its first element's
    /// step takes. Filled only while the trace keeps changes.
    prelude: Vec<Change>,
}

impl Capture {
    /// A trace under `options` starting from `context`, the object holding
    /// the three roots: `initial` is its redacted copy, charged. A budget
    /// `initial` alone crosses truncates from the start.
    pub(crate) fn new(options: TraceOptions, context: &OwnedDataValue) -> Self {
        let paths: Vec<&[Seg]> = options.redact.iter().map(Path::segs).collect();
        let (initial, bytes) = redacting_clone(context, &paths);
        let truncated = options.max_bytes.is_some_and(|max| bytes > max);
        Self {
            options,
            steps: Vec::new(),
            bytes,
            truncated,
            initial: Context::from_root(initial),
            prelude: Vec::new(),
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

    /// Pushes `step`: its changes redacted and charged, or dropped when the
    /// trace keeps none or they would cross the budget, which truncates the
    /// trace from this step on.
    pub(crate) fn push(&mut self, mut step: TraceStep) {
        if !self.changes() {
            step.changes.clear();
        } else if !step.changes.is_empty() {
            let mut size = 0usize;
            for change in &mut step.changes {
                self.redact(change);
                size = size.saturating_add(change_size(change));
            }
            let total = self.bytes.saturating_add(size);
            if self.options.max_bytes.is_some_and(|max| total > max) {
                self.truncated = true;
                step.changes.clear();
            } else {
                self.bytes = total;
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
        let Ok(at) = change.path.parse::<Path>() else {
            change.old_value = OwnedDataValue::Null;
            change.new_value = OwnedDataValue::Null;
            return;
        };
        let at = at.segs();
        let mut inside = Vec::new();
        for redact in &self.options.redact {
            let redact = redact.segs();
            let shared = redact.len().min(at.len());
            if !redact[..shared]
                .iter()
                .zip(&at[..shared])
                .all(|(a, b)| same(a, b))
            {
                continue;
            }
            if redact.len() <= at.len() {
                change.old_value = OwnedDataValue::Null;
                change.new_value = OwnedDataValue::Null;
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

/// Whether two paths' segments can name one node: by key, as on an object,
/// or by index, as on an array. Either reading matching is enough, so a
/// change's path is never let past a redact path it might name.
fn same(a: &Seg, b: &Seg) -> bool {
    a.key == b.key || a.index.is_some_and(|i| b.index == Some(i))
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
/// in `paths` names, which is never cloned, and the clone's size. An empty
/// suffix names the node it reached; a suffix that matches nothing is
/// dropped, so a path that resolves nothing creates nothing.
fn redacting_clone(value: &OwnedDataValue, paths: &[&[Seg]]) -> (OwnedDataValue, usize) {
    if paths.iter().any(|p| p.is_empty()) {
        return (OwnedDataValue::Null, NODE_SIZE);
    }
    if paths.is_empty() {
        return (value.clone(), approx_size(value));
    }
    match value {
        OwnedDataValue::Object(pairs) => {
            let mut size = NODE_SIZE;
            let pairs = pairs
                .iter()
                .map(|(key, child)| {
                    let (child, child_size) = redacting_clone(child, &below_key(paths, key));
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
                    let (child, child_size) = redacting_clone(child, &below_index(paths, index));
                    size += child_size;
                    child
                })
                .collect();
            (OwnedDataValue::Array(items), size)
        }
        other => (other.clone(), approx_size(other)),
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

/// v3's `approx_owned_size`: one word per node, plus a string's length and
/// each key's.
fn approx_size(value: &OwnedDataValue) -> usize {
    match value {
        OwnedDataValue::Object(pairs) => {
            NODE_SIZE
                + pairs
                    .iter()
                    .map(|(key, child)| key.len() + approx_size(child))
                    .sum::<usize>()
        }
        OwnedDataValue::Array(items) => NODE_SIZE + items.iter().map(approx_size).sum::<usize>(),
        OwnedDataValue::String(s) => NODE_SIZE + s.len(),
        _ => NODE_SIZE,
    }
}

/// What a change costs: its path's length and both its values.
fn change_size(change: &Change) -> usize {
    change.path.len() + approx_size(&change.old_value) + approx_size(&change.new_value)
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
        // `json!` sorts an object's keys.
        let context = json!({"data": {"a": {"b": 1, "c": [10, {"d": 2}]}, "#7": "x", "7": "y"},
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
        let (clone, bytes) = redacting_clone(&context, &segs);
        assert_eq!(bytes, approx_size(&clone));
        assert_eq!(bytes, approx_size(&context) - "secret".len());
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
                json!({"user": {"password": "p"}, "list": [0, 1, 2]}),
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
                "data.user.password.deep null null false",
                r#"data.user {"name":"n"} {"name":"m","password":null} false"#,
                r#"data {} {"list":[0,null,2],"user":{"password":null}} false"#,
                "data.list null [0,null] false",
                r#"data.other null "p" false"#,
                "data.user.password null null true",
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
