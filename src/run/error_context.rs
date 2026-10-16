//! The error context: where the engine mirrors the records each call added
//! into the message's context, so a later condition or mapping can branch on
//! why a task failed. v3's `with_error_context_path` and
//! `with_error_context_limit`, with v3's record, trim and replacement rules.
//!
//! The recorder mirrors after the call's error record, for every recorded
//! call that is not a `Skip` and added a record; the engine removes the
//! path's key at run start, after the stamps and before the trace's
//! `initial` (break 20).

use crate::base::{Change, Error, ErrorKind, ErrorRecord, Result};
use crate::expr::{Path, Root};
use crate::state::RunContext;
use datavalue::OwnedDataValue;
use std::borrow::Cow;
use std::sync::Arc;

/// The `metadata` keys the engine writes on every run: the progress
/// after every call, which cross-workflow chaining reads, and the run's
/// stamps. A records array at or below one would be overwritten, or would
/// overwrite it.
const ENGINE_OWNED: [&str; 4] = ["progress", "processed_at", "engine_version", "channel"];

/// Where the engine mirrors failure records in the context, and how many it
/// keeps.
///
/// After every recorded call that is not a `Skip`, each error record the call
/// added (the handler's own and the engine's) is appended at the path as
/// `{"workflow_id", "task_id", "code", "status"}`, `status` being the call's
/// (500 for an `Err`), and the array is trimmed to the newest
/// [`limit`](Self::limit()). A call that adds no record writes nothing, so a
/// message whose calls all succeed has no key at the path, not `[]`. As in
/// v3, every segment below the root names an object key: a non-array at the
/// path, or anything but an object above it, is replaced.
///
/// At run start the engine removes the path's key: a message never brings
/// records in.
///
/// ```
/// use dataflow_rs::run::{EngineConfig, ErrorContext, ErrorKind};
///
/// let ec = ErrorContext::new("metadata.errors".parse()?, ErrorContext::DEFAULT_LIMIT)?;
/// assert_eq!(ec.limit(), 32);
/// let config = EngineConfig::default().error_context(ec);
/// assert!(config.error_context.is_some());
///
/// // The engine owns `metadata.progress`, and every slot below it.
/// let refused = ErrorContext::new("metadata.progress.errors".parse()?, 32).unwrap_err();
/// assert_eq!(*refused.kind(), ErrorKind::Config);
/// # Ok::<(), dataflow_rs::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct ErrorContext {
    path: Path,
    limit: usize,
}

impl ErrorContext {
    /// The records kept when a host names no limit: v3's default.
    pub const DEFAULT_LIMIT: usize = 32;

    /// The error context at `path`, keeping the newest `limit` records.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] for a root (`data`, `metadata` or `temp_data`
    /// itself); for a path at or below a `metadata` slot the engine owns
    /// `metadata.progress`, which it writes after every call for
    /// cross-workflow chaining, and the run's stamps `metadata.processed_at`,
    /// `metadata.engine_version` and `metadata.channel`; and for a `limit` of
    /// 0.
    pub fn new(path: Path, limit: usize) -> Result<Self> {
        if path.is_root() {
            return Err(Error::new(
                ErrorKind::Config,
                format!(
                    "error context path must name a slot inside `{}`, got `{path}`",
                    path.root().as_str()
                ),
            ));
        }
        if path.root() == Root::Metadata
            && let Some(slot) = path.segs().get(1)
            && ENGINE_OWNED.contains(&&*slot.key)
        {
            return Err(Error::new(
                ErrorKind::Config,
                format!(
                    "error context path may not be at or below `metadata.{}` — the engine owns \
                     that slot",
                    slot.key
                ),
            ));
        }
        if limit == 0 {
            return Err(Error::new(
                ErrorKind::Config,
                "error context limit must be at least 1",
            ));
        }
        Ok(Self { path, limit })
    }

    /// Where the records are mirrored.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How many records are kept: the newest.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Removes the path's key from `context`, the owned context at run start:
    /// what the write would find there, by the same reading of the segments
    /// as keys. Anything but an object on the way leaves `context` as it is.
    pub(crate) fn reset(&self, context: &mut OwnedDataValue) {
        let Some((last, parents)) = self.path.segs().split_last() else {
            return;
        };
        let mut node = context;
        for seg in parents {
            node = match node {
                OwnedDataValue::Object(pairs) => {
                    match pairs.iter_mut().find(|(k, _)| **k == *seg.key) {
                        Some((_, child)) => child,
                        None => return,
                    }
                }
                _ => return,
            };
        }
        if let OwnedDataValue::Object(pairs) = node
            && let Some(at) = pairs.iter().position(|(k, _)| **k == *last.key)
        {
            pairs.remove(at);
        }
    }

    /// Mirrors `records`, the ones a call of `workflow`'s `task` added, at
    /// the path, with the call's `status`: v3's `append_error_records`. The
    /// write's `Change`, when `traced`: at the path, from what was there (or
    /// `null`) to the array, or, when a hop above the path was replaced, at
    /// that hop, so a replay rebuilds the context.
    pub(crate) fn mirror(
        &self,
        rc: &mut RunContext<'_, '_>,
        records: &[ErrorRecord],
        workflow: &str,
        task: &str,
        status: u16,
        traced: bool,
    ) -> Option<Change> {
        if records.is_empty() {
            return None;
        }
        let fresh = || {
            records
                .iter()
                .map(|r| record(workflow, task, &r.code, status))
        };
        let segs = self.path.segs();
        // With every segment a key, what the path reads is under objects
        // alone, and a write there creates only objects: the path is the
        // slot, unless a hop above holds a scalar, `null` or an array, when
        // the write does nothing and the walk below finds that hop. Nothing
        // is converted on the way.
        if segs[1..].iter().all(|seg| seg.index.is_none()) {
            let old = rc.get(&self.path).map(Cow::into_owned);
            let (value, change) = self.extend(&self.path, 0, old, fresh(), traced);
            if rc.set(&self.path, value, None) {
                return change;
            }
        }
        let (at, old) = self.walk(rc);
        let below = segs.len() - at.segs().len();
        let (value, change) = self.extend(&at, below, old, fresh(), traced);
        rc.set(&at, value, None);
        change
    }

    /// Where v3 writes, and what is there now: the path itself when every
    /// hop above it holds an object, else the first hop that does not,
    /// missing or replaced. Each hop is read by key, so an array is a hop to
    /// replace, whatever the segment.
    fn walk(&self, rc: &RunContext<'_, '_>) -> (Cow<'_, Path>, Option<OwnedDataValue>) {
        for len in 1..self.path.segs().len() {
            let hop = self.path.prefix(len);
            match rc.get(&hop) {
                Some(value) if matches!(*value, OwnedDataValue::Object(_)) => {}
                found => {
                    let found = found.map(Cow::into_owned);
                    return (Cow::Owned(hop), found);
                }
            }
        }
        let old = rc.get(&self.path).map(Cow::into_owned);
        (Cow::Borrowed(&self.path), old)
    }

    /// The value written at `at`, `below` segments above the path, over
    /// `old`, what is there: the array there, or a new one, extended with
    /// `fresh` and trimmed to the newest `limit`, under an object for each
    /// segment `at` stops short of. Its `Change` when `traced`.
    fn extend(
        &self,
        at: &Path,
        below: usize,
        old: Option<OwnedDataValue>,
        fresh: impl Iterator<Item = OwnedDataValue>,
        traced: bool,
    ) -> (OwnedDataValue, Option<Change>) {
        let old_value = traced.then(|| old.clone().unwrap_or(OwnedDataValue::Null));
        let mut items = match old {
            Some(OwnedDataValue::Array(items)) if below == 0 => items,
            Some(_) if below == 0 => {
                ::log::warn!(
                    "error context path `{}` held a non-array — replacing it",
                    self.path
                );
                Vec::new()
            }
            Some(_) => {
                ::log::warn!(
                    "error context path `{}` runs through a non-object at `{at}` — replacing it",
                    self.path
                );
                Vec::new()
            }
            None => Vec::new(),
        };
        items.extend(fresh);
        // Keep-newest: a loop with a failing body would otherwise grow the
        // array once per sweep.
        if items.len() > self.limit {
            items.drain(..items.len() - self.limit);
        }
        let segs = self.path.segs();
        let value = segs[segs.len() - below..]
            .iter()
            .rev()
            .fold(OwnedDataValue::Array(items), |inner, seg| {
                OwnedDataValue::Object(vec![(String::from(&*seg.key), inner)])
            });
        // A write at a root replaces it, and its `Change` says so (I106).
        let change = old_value.map(|old| {
            let mut change = Change::write(Arc::clone(at.dotted()), old, value.clone());
            change.replaced = at.is_root();
            change
        });
        (value, change)
    }
}

/// One mirrored record: v3's `new_error_record`, its keys in its order.
fn record(workflow: &str, task: &str, code: &str, status: u16) -> OwnedDataValue {
    OwnedDataValue::Object(vec![
        (
            "workflow_id".to_owned(),
            OwnedDataValue::String(workflow.to_owned()),
        ),
        (
            "task_id".to_owned(),
            OwnedDataValue::String(task.to_owned()),
        ),
        ("code".to_owned(), OwnedDataValue::String(code.to_owned())),
        ("status".to_owned(), OwnedDataValue::from(status)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Tuning;

    /// v3's `append_error_records`, ported over owned values: the reference
    /// the mirror is held to. Every segment is a key, with one `#` stripped.
    fn v3_append(context: &mut OwnedDataValue, path: &str, limit: usize, new: &[OwnedDataValue]) {
        fn object(node: &mut OwnedDataValue) -> &mut Vec<(String, OwnedDataValue)> {
            if !matches!(node, OwnedDataValue::Object(_)) {
                *node = OwnedDataValue::Object(Vec::new());
            }
            match node {
                OwnedDataValue::Object(fields) => fields,
                _ => unreachable!(),
            }
        }
        fn child<'v>(node: &'v mut OwnedDataValue, key: &str) -> &'v mut OwnedDataValue {
            let fields = object(node);
            let at = match fields.iter().position(|(k, _)| k == key) {
                Some(at) => at,
                None => {
                    fields.push((key.to_owned(), OwnedDataValue::Null));
                    fields.len() - 1
                }
            };
            &mut fields[at].1
        }
        let mut node = context;
        for raw in path.split('.') {
            node = child(node, raw.strip_prefix('#').unwrap_or(raw));
        }
        if !matches!(node, OwnedDataValue::Array(_)) {
            *node = OwnedDataValue::Array(Vec::new());
        }
        let OwnedDataValue::Array(items) = node else {
            unreachable!()
        };
        items.extend(new.iter().cloned());
        if items.len() > limit {
            items.drain(..items.len() - limit);
        }
    }

    fn parse(text: &str) -> OwnedDataValue {
        serde_json::from_str(text).unwrap()
    }

    fn text(v: &OwnedDataValue) -> String {
        serde_json::to_string(v).unwrap()
    }

    #[test]
    fn with_and_without_a_view_the_mirror_leaves_what_v3_leaves() {
        let seeds = [
            r#"{"data":{},"metadata":{},"temp_data":{}}"#,
            r#"{"data":{"items":[1,2],"n":5,"z":null,"o":{"k":1}},"metadata":{"errors":"s"},"temp_data":{}}"#,
            r#"{"data":[1],"metadata":{"errors":[{"old":1}]},"temp_data":{}}"#,
        ];
        let paths = [
            "metadata.errors",
            "data.a.b.c",
            "data.#x",
            "data.items.0",
            "data.items.k",
            "data.items.0.deep",
            "data.n.x",
            "data.z.x",
            "data.o.list",
            "temp_data.e",
        ];
        let calls: [&[&str]; 3] = [&["A"], &["B", "C"], &["D"]];
        for seed in seeds {
            for path in paths {
                let ec = ErrorContext::new(path.parse().unwrap(), 3).unwrap();
                let mut expected = parse(seed);
                for codes in calls {
                    let new: Vec<_> = codes.iter().map(|c| record("w", "t", c, 500)).collect();
                    v3_append(&mut expected, path, 3, &new);
                }
                for viewed in [false, true] {
                    let mut ctx = parse(seed);
                    let mut rc = RunContext::new(&mut ctx, Tuning::default());
                    for codes in calls {
                        if viewed {
                            rc.read(|_, _| ());
                        }
                        let records: Vec<_> =
                            codes.iter().map(|c| ErrorRecord::new(*c, "m")).collect();
                        ec.mirror(&mut rc, &records, "w", "t", 500, false);
                    }
                    assert_eq!(rc.has_view(), viewed);
                    rc.finish();
                    assert_eq!(
                        text(&ctx),
                        text(&expected),
                        "{path} over {seed}, viewed {viewed}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_write_at_a_root_says_it_replaced_it() {
        // Review A M-3, by I106's rule: a root that holds no object is
        // replaced, and the trace's change says so; a hop below a root is
        // assigned, and its change says nothing.
        for (path, seed, at, replaced) in [
            (
                "data.errors",
                r#"{"data":[1],"metadata":{},"temp_data":{}}"#,
                "data",
                true,
            ),
            (
                "data.n.list",
                r#"{"data":{"n":5},"metadata":{},"temp_data":{}}"#,
                "data.n",
                false,
            ),
        ] {
            let ec = ErrorContext::new(path.parse().unwrap(), 3).unwrap();
            let mut ctx = parse(seed);
            let mut rc = RunContext::new(&mut ctx, Tuning::default());
            let records = [ErrorRecord::new("A", "m")];
            let change = ec
                .mirror(&mut rc, &records, "w", "t", 500, true)
                .expect("a traced mirror records its change");
            rc.finish();
            assert_eq!((&*change.path, change.replaced), (at, replaced), "{path}");
            let written: Path = at.parse().unwrap();
            assert_eq!(
                text(written.get(&ctx).unwrap()),
                text(&change.new_value),
                "{path}"
            );
        }
    }

    #[test]
    fn the_reset_removes_only_what_the_write_would_find() {
        let seed = r#"{"data":{"items":[1,2],"o":{"k":1,"j":2}},"metadata":{"errors":[1]},"temp_data":{}}"#;
        for (path, expected) in [
            (
                "metadata.errors",
                r#"{"data":{"items":[1,2],"o":{"k":1,"j":2}},"metadata":{},"temp_data":{}}"#,
            ),
            (
                "data.o.k",
                r#"{"data":{"items":[1,2],"o":{"j":2}},"metadata":{"errors":[1]},"temp_data":{}}"#,
            ),
            // The write reads `0` as a key, so an array element is not its.
            ("data.items.0", seed),
            ("data.missing.x", seed),
        ] {
            let mut ctx = parse(seed);
            ErrorContext::new(path.parse().unwrap(), 1)
                .unwrap()
                .reset(&mut ctx);
            assert_eq!(text(&ctx), expected, "{path}");
        }
    }
}
