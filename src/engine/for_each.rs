//! Task-level fan-out: run one task's handler once per element of an array.
//!
//! The config lives here; the driver is `WorkflowExecutor::run_for_each`.

use crate::engine::functions::FunctionConfig;
use crate::engine::utils::compute_path_parts;
use crate::engine::workflow::{is_slot_path, over_literal_problem, slots_overlap};
use datalogic_rs::Logic;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// Run a task's function once per element of an array.
///
/// ```json
/// { "id": "infer", "name": "One move per participant",
///   "for_each": { "over": { "var": "data.participants" }, "as": "p",
///                 "max_concurrency": 8,
///                 "collect": "temp_data.move", "into": "temp_data.moves" },
///   "continue_on_error": true,
///   "function": { "name": "model_infer", "input": {
///     "model": { "var": "temp_data.p.model" },
///     "output": "temp_data.move" } } }
/// ```
///
/// The task's `condition` is evaluated once. [`over`](Self::over) is then
/// evaluated once, and the function runs once per element with the element at
/// `temp_data.<as>` and its index at `temp_data.<as>_index`.
///
/// # Every call is isolated
///
/// Each call runs against **its own copy** of the message, taken before the
/// first call, so no call sees another's writes — not even when they run one
/// at a time. When the calls finish, each is folded back into the message in
/// element order: the errors it recorded, then its writes, then its audit
/// entry. That is what makes [`max_concurrency`](Self::max_concurrency) a
/// timing knob only: the message comes out the same at every setting.
///
/// The fold replays the writes a handler made through `TaskContext::set`.
/// A write made by reaching into `ctx.message_mut()` directly is not carried
/// back. The bindings themselves live only in each call's copy, so neither
/// `temp_data.<as>` nor `temp_data.<as>_index` is left behind — unless a
/// handler writes under one of them itself, which is replayed like any other
/// write. [`collect`](Self::collect) may not name a path under either.
///
/// # Results
///
/// [`collect`](Self::collect) names the slot each call writes its result to,
/// and [`into`](Self::into) the array the results land in, in element order.
/// An element that failed (a returned `Err` or a status of `400` or more), or
/// never ran, or wrote nothing at `collect`, leaves `null` at its index.
///
/// # Failures
///
/// The task's `continue_on_error` applies per element. A call that fails the
/// task — an `Err` or a `5xx` without `continue_on_error` — or returns
/// `TaskOutcome::Halt` stops new calls from starting; calls already running
/// finish. Folding stops at the first element whose record fails the task or
/// halts, so the elements after it contribute nothing — however many had
/// already finished. `terminal` and `halt_on` apply once, after the whole
/// fan-out.
///
/// Only handler-backed functions may fan out (`http_call`, `enrich`,
/// `publish_kafka`, custom handlers): built-ins run inline, and JSONLogic's
/// `map`, `filter` and `reduce` already cover per-element transforms.
///
/// `#[non_exhaustive]`: the compiled fields are engine internals. Parse a
/// workflow from JSON to get one.
#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct ForEach {
    /// JSONLogic evaluated once, after the task's condition. It must yield an
    /// array; anything else, `null` included, fails the task. An empty array
    /// runs no call.
    pub over: Value,

    /// `temp_data` field holding the element in each call's view. The JSON
    /// key is `as`. The element's index is at `<as>_index` beside it.
    #[serde(rename = "as")]
    pub item: String,

    /// Calls in flight at once. `1`, the default, runs them one at a time.
    #[serde(default = "one")]
    pub max_concurrency: usize,

    /// Context path each call writes its result to, such as
    /// `temp_data.move`. Requires [`into`](Self::into). Must not overlap
    /// `temp_data.<as>` or `temp_data.<as>_index`.
    #[serde(default)]
    pub collect: Option<String>,

    /// Context path that receives the results as an array in element order.
    /// Requires [`collect`](Self::collect).
    #[serde(default)]
    pub into: Option<String>,

    /// Engine-internal: compiled `over`. Not part of the stable API.
    #[doc(hidden)]
    #[serde(skip)]
    pub compiled_over: Option<Arc<Logic>>,

    /// Engine-internal: pre-split `temp_data.<as>`.
    #[doc(hidden)]
    #[serde(skip)]
    pub item_parts: Arc<[Arc<str>]>,

    /// Engine-internal: pre-split `temp_data.<as>_index`.
    #[doc(hidden)]
    #[serde(skip)]
    pub index_parts: Arc<[Arc<str>]>,

    /// Engine-internal: pre-split `collect`.
    #[doc(hidden)]
    #[serde(skip)]
    pub collect_parts: Arc<[Arc<str>]>,

    /// Engine-internal: pre-split `into`.
    #[doc(hidden)]
    #[serde(skip)]
    pub into_parts: Arc<[Arc<str>]>,
}

fn one() -> usize {
    1
}

fn no_parts() -> Arc<[Arc<str>]> {
    Arc::from([] as [Arc<str>; 0])
}

/// The three roots every context path starts at.
const ROOTS: [&str; 3] = ["data", "metadata", "temp_data"];

/// Whether `path` names a key below a context root, such as `temp_data.m`.
fn is_context_path(path: &str) -> bool {
    let Some((root, rest)) = path.split_once('.') else {
        return false;
    };
    ROOTS.contains(&root) && is_slot_path(rest)
}

/// A rule an authored `for_each` breaks, and the key it is reported at.
///
/// `field` is the key under `for_each` (`"max_concurrency"`), or empty when
/// the problem is the `for_each` as a whole.
pub(crate) struct ForEachProblem {
    pub field: &'static str,
    pub message: String,
}

impl ForEach {
    /// `<as>_index`: the sibling slot holding the element's index.
    pub fn index_slot(&self) -> String {
        format!("{}_index", self.item)
    }

    /// The one statement of the `for_each` rules.
    ///
    /// `Workflow::validate` refuses a workflow on it, and
    /// `authoring::check_steps` reports it at the offending key, so the two
    /// cannot disagree about what loads.
    pub(crate) fn problem(&self, function: &FunctionConfig) -> Option<ForEachProblem> {
        let at = |field, message: String| Some(ForEachProblem { field, message });

        if function.is_sync_builtin() {
            return at(
                "",
                format!(
                    "for_each runs a handler-backed function once per element, and '{}' is \
                     a built-in that runs inline — use JSONLogic's map, filter or reduce over \
                     the array instead",
                    function.function_name()
                ),
            );
        }
        if let Some(problem) = over_literal_problem(&self.over) {
            return at("over", problem.replacen("loop over", "for_each over", 1));
        }
        if !is_slot_path(&self.item) {
            return at(
                "as",
                format!(
                    "for_each as must be a non-empty temp_data field path, got {:?}",
                    self.item
                ),
            );
        }
        if self.max_concurrency == 0 {
            return at(
                "max_concurrency",
                "for_each max_concurrency must be at least 1".to_string(),
            );
        }

        let (collect, into) = match (&self.collect, &self.into) {
            (None, None) => return None,
            (Some(collect), Some(into)) => (collect, into),
            (collect, _) => {
                return at(
                    if collect.is_some() { "collect" } else { "into" },
                    "for_each collect and into go together: collect names what each call \
                     writes, into names the array the results land in"
                        .to_string(),
                );
            }
        };
        for (field, path) in [("collect", collect), ("into", into)] {
            if !is_context_path(path) {
                return at(
                    field,
                    format!(
                        "for_each {field} must be a context path below data, metadata or \
                         temp_data, got {path:?}"
                    ),
                );
            }
        }
        let item = format!("temp_data.{}", self.item);
        let index = format!("temp_data.{}", self.index_slot());
        for binding in [&item, &index] {
            if slots_overlap(collect, binding) {
                return at(
                    "collect",
                    format!(
                        "for_each collect ({collect:?}) and {binding:?} overlap — the bindings \
                         live only in each call's copy, and a result written under one would be \
                         replayed into the message and left behind"
                    ),
                );
            }
        }
        for other in [collect.as_str(), &item, &index] {
            if slots_overlap(into, other) {
                return at(
                    "into",
                    format!(
                        "for_each into ({into:?}) and {other:?} overlap — the results array \
                         would overwrite, or be overwritten by, a slot written per call"
                    ),
                );
            }
        }
        None
    }

    /// Pre-split every path the driver writes or reads, once, at build time.
    /// Idempotent, so a hot reload can run it again.
    #[doc(hidden)]
    pub fn precompute_paths(&mut self) {
        fn context(path: &Option<String>) -> Arc<[Arc<str>]> {
            match path.as_deref().and_then(|p| p.split_once('.')) {
                Some((root, rest)) => compute_path_parts(root, rest),
                None => no_parts(),
            }
        }
        self.item_parts = compute_path_parts("temp_data", &self.item);
        self.index_parts = compute_path_parts("temp_data", &self.index_slot());
        self.collect_parts = context(&self.collect);
        self.into_parts = context(&self.into);
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::workflow::Workflow;
    use std::sync::Arc;

    fn task_with(for_each: &str, function: &str) -> Result<Workflow, String> {
        let wf = Workflow::from_json(&format!(
            r#"{{ "id": "w", "name": "w", "tasks": [
                 {{ "id": "t", "name": "t", "for_each": {for_each}, "function": {function} }}] }}"#
        ))
        .map_err(|e| e.to_string())?;
        wf.validate().map_err(|e| e.to_string())?;
        Ok(wf)
    }

    const CUSTOM: &str = r#"{"name": "infer", "input": {}}"#;

    #[test]
    fn a_for_each_parses_with_its_defaults() {
        let wf = task_with(r#"{"over": {"var": "data.ps"}, "as": "p"}"#, CUSTOM).unwrap();
        let fe = wf.tasks[0].for_each.as_ref().unwrap();
        assert_eq!(fe.item, "p");
        assert_eq!(fe.max_concurrency, 1, "sequential by default");
        assert!(fe.collect.is_none() && fe.into.is_none());
        assert_eq!(fe.index_slot(), "p_index");
        assert!(fe.compiled_over.is_none(), "compiled at build");
    }

    #[test]
    fn a_task_without_for_each_has_none() {
        let wf = Workflow::from_json(
            r#"{"id": "w", "name": "w", "tasks": [
                {"id": "t", "name": "t", "function": {"name": "infer", "input": {}}}]}"#,
        )
        .unwrap();
        assert!(wf.tasks[0].for_each.is_none());
    }

    #[test]
    fn every_rule_refuses_its_shape() {
        for (for_each, needle) in [
            (
                r#"{"over": "data.ps", "as": "p"}"#,
                "can never evaluate to an array",
            ),
            (
                r#"{"over": 3, "as": "p"}"#,
                "can never evaluate to an array",
            ),
            (r#"{"over": [], "as": "a..b"}"#, "for_each as"),
            (r#"{"over": [], "as": ""}"#, "for_each as"),
            (
                r#"{"over": [], "as": "p", "max_concurrency": 0}"#,
                "max_concurrency",
            ),
            (
                r#"{"over": [], "as": "p", "collect": "temp_data.m"}"#,
                "collect and into",
            ),
            (
                r#"{"over": [], "as": "p", "into": "temp_data.ms"}"#,
                "collect and into",
            ),
            (
                r#"{"over": [], "as": "p", "collect": "m", "into": "temp_data.ms"}"#,
                "context path",
            ),
            (
                r#"{"over": [], "as": "p", "collect": "temp_data.m", "into": "temp_data"}"#,
                "context path",
            ),
            (
                r#"{"over": [], "as": "p", "collect": "temp_data.m", "into": "payload.ms"}"#,
                "context path",
            ),
            (
                r#"{"over": [], "as": "p", "collect": "temp_data.m", "into": "temp_data.m.x"}"#,
                "overlap",
            ),
            (
                r#"{"over": [], "as": "p", "collect": "temp_data.m", "into": "temp_data.p_index"}"#,
                "overlap",
            ),
            (
                r#"{"over": [], "as": "p", "collect": "temp_data.m", "into": "temp_data.p"}"#,
                "overlap",
            ),
            // A result under the binding would be replayed into the message
            // and leave the binding behind. The needle is unique to the
            // `collect` message; the `into` one starts `for_each into (`.
            (
                r#"{"over": [], "as": "p", "collect": "temp_data.p.out", "into": "temp_data.outs"}"#,
                "for_each collect (",
            ),
            (
                r#"{"over": [], "as": "p", "collect": "temp_data.p", "into": "temp_data.outs"}"#,
                "for_each collect (",
            ),
            (
                r#"{"over": [], "as": "p", "collect": "temp_data.p_index", "into": "temp_data.outs"}"#,
                "for_each collect (",
            ),
        ] {
            let err = task_with(for_each, CUSTOM).expect_err(for_each);
            assert!(err.contains(needle), "{for_each}: {err}");
        }
        assert!(
            task_with(r#"{"as": "p"}"#, CUSTOM).is_err(),
            "over is required"
        );
        assert!(
            task_with(r#"{"over": []}"#, CUSTOM).is_err(),
            "as is required"
        );
    }

    #[test]
    fn an_explicit_null_over_is_refused() {
        // `over: Value` is required, so an explicit null deserializes to
        // `Value::Null` and is refused by the literal rule, not read as absent.
        let err = task_with(r#"{"over": null, "as": "p"}"#, CUSTOM).expect_err("null");
        assert!(err.contains("null"), "{err}");
    }

    #[test]
    fn valid_shapes_are_accepted() {
        for for_each in [
            r#"{"over": [1, 2], "as": "p"}"#,
            r#"{"over": {"var": "data.ps"}, "as": "cur.p", "max_concurrency": 8}"#,
            r#"{"over": [], "as": "p", "collect": "temp_data.m", "into": "data.ms"}"#,
            // `_out` is not a segment boundary, so this is beside the binding,
            // not under it.
            r#"{"over": [], "as": "p", "collect": "temp_data.p_out", "into": "temp_data.outs"}"#,
        ] {
            assert!(task_with(for_each, CUSTOM).is_ok(), "{for_each}");
        }
    }

    #[test]
    fn a_for_each_on_a_sync_builtin_is_refused() {
        let err = task_with(
            r#"{"over": [], "as": "p"}"#,
            r#"{"name": "map", "input": {"mappings": []}}"#,
        )
        .expect_err("map is a sync built-in");
        assert!(err.contains("handler-backed"), "{err}");
        for (name, input) in [
            ("http_call", r#"{"connector": "c"}"#),
            ("enrich", r#"{"connector": "c", "merge_path": "data.x"}"#),
            ("publish_kafka", r#"{"connector": "c", "topic": "t"}"#),
        ] {
            let function = format!(r#"{{"name": "{name}", "input": {input}}}"#);
            assert!(
                task_with(r#"{"over": [], "as": "p"}"#, &function).is_ok(),
                "{name} is handler-backed"
            );
        }
    }

    #[test]
    fn a_group_carrying_for_each_is_refused_at_parse() {
        let err = Workflow::from_json(
            r#"{ "id": "w", "name": "w", "tasks": [
                 {"id": "g", "for_each": {"over": [], "as": "p"}, "tasks": [
                   {"id": "t", "name": "t", "function": {"name": "infer", "input": {}}}]}] }"#,
        )
        .expect_err("groups cannot fan out");
        assert!(err.to_string().contains("for_each"), "{err}");
    }

    #[test]
    fn a_setup_step_is_validated_too() {
        let wf = Workflow::from_json(
            r#"{ "id": "w", "name": "w",
                 "loop": {"max": 2, "setup": [
                   {"id": "s", "name": "s",
                    "for_each": {"over": [], "as": "p", "max_concurrency": 0},
                    "function": {"name": "infer", "input": {}}}]},
                 "tasks": [{"id": "t", "name": "t", "function": {"name": "infer", "input": {}}}] }"#,
        )
        .unwrap();
        assert!(wf.validate().is_err());
    }

    #[test]
    fn precompute_paths_splits_every_slot() {
        let mut fe = task_with(
            r#"{"over": [], "as": "p", "collect": "temp_data.m", "into": "data.ms"}"#,
            CUSTOM,
        )
        .unwrap()
        .tasks[0]
            .for_each
            .clone()
            .unwrap();
        fe.precompute_paths();
        let s = |p: &Arc<[Arc<str>]>| p.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(s(&fe.item_parts), ["temp_data", "p"]);
        assert_eq!(s(&fe.index_parts), ["temp_data", "p_index"]);
        assert_eq!(s(&fe.collect_parts), ["temp_data", "m"]);
        assert_eq!(s(&fe.into_parts), ["data", "ms"]);

        let mut bare = task_with(r#"{"over": [], "as": "p"}"#, CUSTOM)
            .unwrap()
            .tasks[0]
            .for_each
            .clone()
            .unwrap();
        bare.precompute_paths();
        assert!(bare.collect_parts.is_empty() && bare.into_parts.is_empty());
    }
}
