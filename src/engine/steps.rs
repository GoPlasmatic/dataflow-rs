//! The authored step grammar: how a workflow's `tasks` array is read.
//!
//! An element of `tasks` is either a [`Task`] or a [`TaskGroup`], and the
//! parser flattens that tree into `Workflow::tasks` at deserialization time so
//! the executor keeps walking a flat slice. This module owns both halves of
//! that grammar:
//!
//! - `flatten` — the parser, which builds `Vec<Task>` and fails on the first
//!   malformed element.
//! - [`walk_authored_steps`] — a public walker over the *authored* JSON, which
//!   never fails and yields every node with the coordinate the author typed.
//!
//! They live together deliberately. The group test and the depth cap are the
//! two facts a downstream host would otherwise have to mirror, and keeping the
//! parser and the walker in one file puts both users of those facts on screen
//! for anyone who changes them.
//!
//! # Why a host needs the authored shape
//!
//! By the time a host holds a [`Workflow`](crate::Workflow), the tree is gone:
//! `tasks` is flat and `Task::group_starts` is not part of the stable API. But
//! a validation error, a lint finding or a dependency extraction has to point
//! at `tasks[1].tasks[0].id` — the coordinate in the document the author
//! actually wrote. That is what this walker provides.

use super::task::{Task, TaskGroup};
use serde::Deserialize;
use serde::de::{Deserializer, Error as DeError};
use serde_json::Value;

/// Maximum group nesting the parser accepts.
///
/// Deeper than this is a generated-JSON accident rather than an authored
/// control-flow shape, and the bound keeps the per-task `group_starts` vector
/// trivially small.
///
/// Public so a host validating authored JSON reads the engine's real limit
/// instead of copying the number. Depth counts *enclosing groups*: a top-level
/// group is at depth 0, so groups are accepted at depths `0..MAX_GROUP_DEPTH`
/// and a group at `MAX_GROUP_DEPTH` is rejected by the parser and reported as
/// [`StepKind::TooDeep`] by the walker.
pub const MAX_GROUP_DEPTH: usize = 8;

/// Whether this authored step element parses as a task group.
///
/// The test is **presence of a `tasks` key, nothing else** — the same test the
/// parser makes. In particular a `tasks` key holding a non-array is still a
/// group, and a malformed one: the parser will reject it as a bad group rather
/// than silently reading it as a task.
///
/// An element carrying neither `tasks` nor `function` is *not* a group, so a
/// caller reports a broken task — which is what the parser's own diagnostic
/// says (`missing field 'function'`).
///
/// ```
/// use dataflow_rs::engine::steps::is_group;
/// use serde_json::json;
///
/// assert!(is_group(&json!({"id": "g", "tasks": []})));
/// assert!(
///     is_group(&json!({"id": "g", "tasks": "oops"})),
///     "presence of the key, not its type — this is a malformed group"
/// );
/// assert!(!is_group(&json!({"id": "t", "function": {"name": "map"}})));
/// assert!(!is_group(&json!({"id": "t"})), "neither key: a broken task");
/// assert!(!is_group(&json!("not even an object")));
/// ```
#[inline]
pub fn is_group(step: &Value) -> bool {
    step.get("tasks").is_some()
}

/// What an authored step element is.
///
/// Deliberately not `#[non_exhaustive]`: a caller matching on this is deciding
/// how to report a node, and a fourth kind would need that decision revisited
/// at every site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// A task — or an element malformed enough that it is not a group either.
    Leaf,
    /// A task group. Its members follow it in the walk.
    Group,
    /// A group nested at or beyond [`MAX_GROUP_DEPTH`]. The parser rejects the
    /// whole workflow here; the walker reports it and does **not** descend, so
    /// nothing is silently truncated without a node to point at.
    TooDeep,
}

/// One node of an authored `tasks` tree.
#[derive(Debug, Clone)]
pub struct AuthoredStep<'a> {
    /// The coordinate the author typed, rooted at the workflow: `tasks[1]`,
    /// `tasks[1].tasks[0]`. Append your own field segment to point at a
    /// property — `format!("{}.id", step.path)`.
    pub path: String,
    /// The element itself, borrowed from the input.
    pub node: &'a Value,
    /// Whether this is a task, a group, or a group too deeply nested.
    pub kind: StepKind,
    /// Enclosing groups. `0` for a top-level element.
    pub depth: usize,
}

/// Walk an authored `tasks` array, yielding every node with its path.
///
/// Traversal is document order, pre-order: a group is yielded before its
/// members, so filtering to [`StepKind::Leaf`] reproduces the engine's
/// flattened `Workflow::tasks` exactly, in order.
///
/// **This walker never fails.** Where `flatten` returns `Err` on the first
/// malformed element, an empty group or an over-deep group, the walker yields
/// those nodes so a validator can collect every violation in one pass. A
/// `tasks` value that is not an array yields nothing at all — whether `tasks`
/// is a non-empty array is a rule for the caller to report, not for this walk
/// to fail on.
///
/// ```
/// use dataflow_rs::engine::steps::{StepKind, walk_authored_steps};
/// use serde_json::json;
///
/// let tasks = json!([
///     {"id": "first", "function": {"name": "map", "input": {"mappings": []}}},
///     {"id": "guard", "condition": true, "tasks": [
///         {"id": "inner", "function": {"name": "map", "input": {"mappings": []}}}
///     ]}
/// ]);
///
/// let steps: Vec<_> = walk_authored_steps(&tasks).collect();
/// let seen: Vec<(&str, StepKind)> =
///     steps.iter().map(|s| (s.path.as_str(), s.kind)).collect();
///
/// assert_eq!(seen, vec![
///     ("tasks[0]", StepKind::Leaf),
///     ("tasks[1]", StepKind::Group),
///     ("tasks[1].tasks[0]", StepKind::Leaf),
/// ]);
/// ```
pub fn walk_authored_steps(tasks: &Value) -> AuthoredSteps<'_> {
    AuthoredSteps {
        stack: match tasks.as_array() {
            Some(items) => vec![Frame {
                items,
                idx: 0,
                prefix: "tasks".to_string(),
                depth: 0,
            }],
            // Not an array: nothing to walk. The caller reports the shape.
            None => Vec::new(),
        },
    }
}

/// One level of the walk: the array being iterated, how far through it we are,
/// and the path prefix its elements hang off.
struct Frame<'a> {
    items: &'a [Value],
    idx: usize,
    prefix: String,
    depth: usize,
}

/// Iterator returned by [`walk_authored_steps`].
///
/// Lazy, over an explicit stack rather than recursion, so nothing is allocated
/// beyond each node's `path` and the stack itself — which is bounded by
/// [`MAX_GROUP_DEPTH`].
pub struct AuthoredSteps<'a> {
    stack: Vec<Frame<'a>>,
}

impl<'a> Iterator for AuthoredSteps<'a> {
    type Item = AuthoredStep<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let frame = self.stack.last_mut()?;
            let Some(node) = frame.items.get(frame.idx) else {
                // This level is exhausted; resume the one that opened it.
                self.stack.pop();
                continue;
            };

            let path = format!("{}[{}]", frame.prefix, frame.idx);
            let depth = frame.depth;
            frame.idx += 1;

            if !is_group(node) {
                return Some(AuthoredStep {
                    path,
                    node,
                    kind: StepKind::Leaf,
                    depth,
                });
            }

            // A group at the cap is what the parser rejects. Report it and do
            // not descend — the members are unreachable either way, and this
            // gives the caller a node to point at instead of a silent gap.
            if depth >= MAX_GROUP_DEPTH {
                return Some(AuthoredStep {
                    path,
                    node,
                    kind: StepKind::TooDeep,
                    depth,
                });
            }

            // Descend only into a well-formed `tasks` array. A `tasks` key
            // holding anything else is still a group — a malformed one — and is
            // reported as such with no members.
            if let Some(children) = node.get("tasks").and_then(Value::as_array) {
                self.stack.push(Frame {
                    items: children,
                    idx: 0,
                    prefix: format!("{path}.tasks"),
                    depth: depth + 1,
                });
            }

            return Some(AuthoredStep {
                path,
                node,
                kind: StepKind::Group,
                depth,
            });
        }
    }
}

/// The non-`tasks` half of a group element. `tasks` is carried too so the
/// whole element deserializes in one pass; unknown keys are ignored, as
/// everywhere else in the workflow schema.
#[derive(Deserialize)]
struct GroupHeader {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "crate::engine::utils::default_condition")]
    condition: Value,
    #[serde(default)]
    terminal: bool,
    tasks: Vec<Value>,
}

/// `deserialize_with` target for `Workflow::tasks`.
///
/// Fails fast, unlike [`walk_authored_steps`]: the engine will not run a
/// workflow it cannot fully parse, so the first malformed element ends the
/// attempt. A host that wants every problem at once walks the authored JSON
/// instead.
pub(crate) fn flatten<'de, D>(deserializer: D) -> Result<Vec<Task>, D::Error>
where
    D: Deserializer<'de>,
{
    let steps = Vec::<Value>::deserialize(deserializer)?;
    let mut tasks = Vec::with_capacity(steps.len());
    walk(&steps, 0, &mut tasks).map_err(D::Error::custom)?;
    Ok(tasks)
}

/// Append `steps` to `out` in document order, recording group spans.
fn walk(steps: &[Value], depth: usize, out: &mut Vec<Task>) -> Result<(), String> {
    for step in steps {
        if !is_group(step) {
            let task: Task = serde_json::from_value(step.clone())
                .map_err(|e| format!("invalid task in workflow tasks: {e}"))?;
            out.push(task);
            continue;
        }

        if depth >= MAX_GROUP_DEPTH {
            return Err(format!(
                "task groups nested deeper than {MAX_GROUP_DEPTH} levels"
            ));
        }

        let header: GroupHeader = serde_json::from_value(step.clone())
            .map_err(|e| format!("invalid task group in workflow tasks: {e}"))?;

        let start = out.len();
        walk(&header.tasks, depth + 1, out)?;
        let end = out.len();
        if end == start {
            return Err(format!(
                "task group '{}' contains no tasks — an empty group can only be a mistake",
                header.id
            ));
        }

        // Outermost first: an inner group nested at the same start index
        // has already pushed its own entry, so this one goes in front of
        // it. Bounded by `MAX_GROUP_DEPTH`, so the shift is trivial.
        out[start].group_starts.insert(
            0,
            TaskGroup {
                id: header.id,
                name: header.name,
                description: header.description,
                condition: header.condition,
                compiled_condition: None,
                terminal: header.terminal,
                end,
            },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::workflow::Workflow;
    use serde_json::json;

    fn leaf(id: &str) -> Value {
        json!({"id": id, "name": id, "function": {"name": "map", "input": {"mappings": []}}})
    }

    /// `n` groups nested one inside the next, innermost holding one task.
    /// `n == 1` is a single top-level group, which sits at depth 0.
    fn nested_groups(n: usize) -> Value {
        let mut node = leaf("innermost");
        for level in (0..n).rev() {
            node = json!({"id": format!("g{level}"), "condition": true, "tasks": [node]});
        }
        json!([node])
    }

    fn workflow_with(tasks: &Value) -> Result<Workflow, String> {
        Workflow::from_json(
            &json!({"id": "w", "name": "w", "priority": 0, "tasks": tasks}).to_string(),
        )
        .map_err(|e| e.to_string())
    }

    fn kinds(tasks: &Value) -> Vec<(String, StepKind, usize)> {
        walk_authored_steps(tasks)
            .map(|s| (s.path, s.kind, s.depth))
            .collect()
    }

    /// Acceptance criterion: the walker's leaf set is the parser's flattened
    /// `Workflow::tasks`, by id and by order. This is what pins the two
    /// recursions to each other — sharing `is_group` alone would not catch a
    /// divergence in the walk itself.
    #[test]
    fn walker_leaves_match_the_parsers_flattened_tasks() {
        let fixtures = vec![
            json!([leaf("a"), leaf("b")]),
            json!([{"id": "g", "condition": true, "tasks": [leaf("a"), leaf("b")]}]),
            json!([
                leaf("before"),
                {"id": "g1", "condition": true, "tasks": [
                    leaf("in1"),
                    {"id": "g2", "condition": true, "tasks": [leaf("deep")]},
                    leaf("in2"),
                ]},
                leaf("after"),
            ]),
            nested_groups(MAX_GROUP_DEPTH),
        ];

        for tasks in fixtures {
            let parsed = workflow_with(&tasks).expect("fixture parses");
            let from_parser: Vec<&str> = parsed.tasks.iter().map(|t| t.id.as_str()).collect();

            let from_walker: Vec<&str> = walk_authored_steps(&tasks)
                .filter(|s| s.kind == StepKind::Leaf)
                .map(|s| s.node["id"].as_str().unwrap())
                .collect();

            assert_eq!(
                from_walker, from_parser,
                "walker leaves must equal the flattened tasks, in order, for {tasks}"
            );
        }
    }

    #[test]
    fn paths_are_the_coordinates_the_author_typed() {
        let tasks = json!([
            leaf("first"),
            {"id": "g", "condition": true, "tasks": [leaf("inner"), leaf("second")]},
        ]);

        let paths: Vec<String> = walk_authored_steps(&tasks).map(|s| s.path).collect();
        assert_eq!(
            paths,
            vec![
                "tasks[0]",
                "tasks[1]",
                "tasks[1].tasks[0]",
                "tasks[1].tasks[1]"
            ]
        );
    }

    #[test]
    fn groups_are_yielded_before_their_members() {
        let tasks = json!([{"id": "g", "condition": true, "tasks": [leaf("inner")]}]);
        assert_eq!(
            kinds(&tasks),
            vec![
                ("tasks[0]".to_string(), StepKind::Group, 0),
                ("tasks[0].tasks[0]".to_string(), StepKind::Leaf, 1),
            ],
            "pre-order, so filtering to Leaf reproduces parse order"
        );
    }

    #[test]
    fn max_group_depth_is_the_value_the_parser_enforces() {
        // Exactly at the cap parses: MAX_GROUP_DEPTH groups occupy depths
        // 0..MAX_GROUP_DEPTH.
        let ok = nested_groups(MAX_GROUP_DEPTH);
        assert!(
            workflow_with(&ok).is_ok(),
            "{MAX_GROUP_DEPTH} levels of nesting is accepted"
        );
        assert!(
            walk_authored_steps(&ok).all(|s| s.kind != StepKind::TooDeep),
            "and the walker agrees nothing is too deep"
        );

        // One more is rejected by the parser…
        let too_deep = nested_groups(MAX_GROUP_DEPTH + 1);
        let err = workflow_with(&too_deep).expect_err("one level past the cap is rejected");
        assert!(
            err.contains("nested deeper than"),
            "parser reports the depth cap, got: {err}"
        );

        // …and reported — not silently dropped — by the walker, at the same node.
        let flagged: Vec<_> = walk_authored_steps(&too_deep)
            .filter(|s| s.kind == StepKind::TooDeep)
            .collect();
        assert_eq!(flagged.len(), 1, "exactly the one offending group");
        assert_eq!(flagged[0].depth, MAX_GROUP_DEPTH);
        assert_eq!(flagged[0].node["id"], json!(format!("g{MAX_GROUP_DEPTH}")));
    }

    #[test]
    fn a_too_deep_group_is_not_descended_into() {
        let tasks = nested_groups(MAX_GROUP_DEPTH + 1);
        let deepest = walk_authored_steps(&tasks).map(|s| s.depth).max().unwrap();
        assert_eq!(
            deepest, MAX_GROUP_DEPTH,
            "the walk stops at the offending group; its members are never yielded"
        );
        assert!(
            !walk_authored_steps(&tasks).any(|s| s.node["id"] == json!("innermost")),
            "the leaf below the cap is unreachable, and reported as such by its absent parent"
        );
    }

    #[test]
    fn a_leaf_is_never_too_deep() {
        // A task sitting inside the maximum legal nesting is fine — the parser
        // checks depth only when it opens a group.
        let tasks = nested_groups(MAX_GROUP_DEPTH);
        let innermost = walk_authored_steps(&tasks)
            .find(|s| s.node["id"] == json!("innermost"))
            .expect("the deepest leaf is yielded");
        assert_eq!(innermost.kind, StepKind::Leaf);
        assert_eq!(innermost.depth, MAX_GROUP_DEPTH);
    }

    #[test]
    fn an_element_with_neither_tasks_nor_function_is_a_leaf() {
        // Reported as a broken *task*, matching the parser's own diagnostic —
        // not as a broken group.
        let tasks = json!([{"id": "orphan"}]);
        assert_eq!(
            kinds(&tasks),
            vec![("tasks[0]".to_string(), StepKind::Leaf, 0)]
        );

        let err = workflow_with(&tasks).expect_err("the parser rejects it");
        assert!(
            err.contains("invalid task in workflow tasks"),
            "and calls it a task, got: {err}"
        );
    }

    #[test]
    fn a_tasks_key_that_is_not_an_array_is_still_a_group() {
        // Presence of the key decides, not its type. This is exactly where the
        // TypeScript `isTaskGroup` used to disagree with the engine.
        let tasks = json!([{"id": "g", "tasks": "oops"}]);
        assert!(is_group(&tasks[0]));
        assert_eq!(
            kinds(&tasks),
            vec![("tasks[0]".to_string(), StepKind::Group, 0)],
            "a malformed group with no members, not a task"
        );

        let err = workflow_with(&tasks).expect_err("the parser rejects it");
        assert!(
            err.contains("invalid task group"),
            "and calls it a group, got: {err}"
        );
    }

    #[test]
    fn an_empty_group_is_yielded_not_an_error() {
        // The walker is total where the parser fails fast, so a validator can
        // collect this alongside every other violation in one pass.
        let tasks = json!([{"id": "empty", "condition": true, "tasks": []}]);
        assert_eq!(
            kinds(&tasks),
            vec![("tasks[0]".to_string(), StepKind::Group, 0)]
        );

        let err = workflow_with(&tasks).expect_err("the parser rejects an empty group");
        assert!(err.contains("contains no tasks"), "got: {err}");
    }

    #[test]
    fn a_non_array_input_yields_nothing() {
        for input in [
            Value::Null,
            json!({}),
            json!("tasks"),
            json!(7),
            json!({"tasks": []}),
        ] {
            assert_eq!(
                walk_authored_steps(&input).count(),
                0,
                "not an array, so nothing to walk: {input}"
            );
        }
    }

    #[test]
    fn an_empty_array_yields_nothing_and_leaves_no_frame_behind() {
        assert_eq!(walk_authored_steps(&json!([])).count(), 0);
        // Nested empties must not hang or double-yield the parent.
        let tasks = json!([{"id": "g", "tasks": [{"id": "inner", "tasks": []}]}]);
        assert_eq!(
            kinds(&tasks),
            vec![
                ("tasks[0]".to_string(), StepKind::Group, 0),
                ("tasks[0].tasks[0]".to_string(), StepKind::Group, 1),
            ]
        );
    }
}
