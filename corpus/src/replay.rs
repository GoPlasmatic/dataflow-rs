//! Rebuilding a trace's per-step contexts from an initial context and each
//! step's changes: what v4's trace is (R17), and what the UI will do to show
//! a step. Phase 1 runs it against v3's own traces, which carry both the
//! snapshots and the changes, to learn what the changes alone do not rebuild.

use crate::case::{Case, MessageSpec, Step};
use crate::json::Json;
use crate::path;
use std::collections::BTreeSet;

/// The context after each step, applying that step's `changes` in order (a
/// `set` of `new_value`, or a removal when `removed` is true). `None` for a
/// step that carries no `changes` key; the running context then goes on
/// without it.
pub fn rebuild(initial: &Json, steps: &[Json]) -> Vec<Option<Json>> {
    let mut context = initial.clone();
    steps
        .iter()
        .map(|step| {
            let changes = step.get("changes")?.as_array()?;
            for change in changes {
                apply(&mut context, change);
            }
            Some(context.clone())
        })
        .collect()
}

/// One recorded `Change`: `{path, old_value, new_value, removed?, replaced?}`.
///
/// A change whose path is a whole root (`data`, `metadata`, `temp_data`) is
/// merged into it, because that is what `map` does there and what its change
/// records: only the keys it wrote. A handler's `ctx.set` at a root replaces
/// it instead. v3's format cannot tell the two apart, and the corpus has no
/// such write; v4's change says `replaced: true` for the replacement (I106),
/// which is then set like any other write.
pub fn apply(context: &mut Json, change: &Json) {
    let Some(at) = change.get("path").and_then(Json::as_str) else {
        return;
    };
    if change.get("removed").and_then(Json::as_bool) == Some(true) {
        path::remove(context, at);
        return;
    }
    let value = change.get("new_value").cloned().unwrap_or(Json::Null);
    let replaced = change.get("replaced").and_then(Json::as_bool) == Some(true);
    let merges = !replaced && matches!(at, "data" | "metadata" | "temp_data");
    match (merges, value, context.get_mut(at)) {
        (true, Json::Object(pairs), Some(root @ Json::Object(_))) => {
            for (key, v) in pairs {
                root.insert(key, v);
            }
        }
        (_, value, _) => path::set(context, at, value),
    }
}

/// Where replay falls short on a recorded case: for every traced run, the
/// context paths at which a step's snapshot differs from what its run's input
/// context plus the steps' changes rebuild. Array indices are `*`, and key
/// order is ignored. A path names the outermost point of difference.
pub fn unrebuilt(case: &Case, recording: &Json) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (i, step) in case.steps.iter().enumerate() {
        let Step::Run(run) = step else { continue };
        let pointer = format!("/steps/{i}/run/trace/steps");
        let Some(steps) = recording.pointer(&pointer).and_then(Json::as_array) else {
            continue;
        };
        let rebuilt = rebuild(&initial_context(&run.message), steps);
        for (step, rebuilt) in steps.iter().zip(rebuilt) {
            if let (Some(snapshot), Some(rebuilt)) = (step.pointer("/message/context"), rebuilt) {
                differences(snapshot, &rebuilt, "", &mut out);
            }
        }
    }
    out
}

/// What a run starts from: each root as the case seeds it, or `{}` when it
/// does not seed an object (v3 ignores a non-object seed).
pub fn initial_context(message: &MessageSpec) -> Json {
    let root = |seed: &Option<Json>| match seed {
        Some(object @ Json::Object(_)) => object.clone(),
        _ => Json::object(),
    };
    Json::Object(vec![
        ("data".to_owned(), root(&message.data)),
        ("metadata".to_owned(), root(&message.metadata)),
        ("temp_data".to_owned(), root(&message.temp_data)),
    ])
}

fn differences(snapshot: &Json, rebuilt: &Json, at: &str, out: &mut BTreeSet<String>) {
    let join = |segment: &str| {
        if at.is_empty() {
            segment.to_owned()
        } else {
            format!("{at}.{segment}")
        }
    };
    match (snapshot, rebuilt) {
        (Json::Object(a), Json::Object(b)) => {
            let keys: BTreeSet<&str> = a.iter().chain(b).map(|(k, _)| k.as_str()).collect();
            for key in keys {
                match (snapshot.get(key), rebuilt.get(key)) {
                    (Some(x), Some(y)) => differences(x, y, &join(key), out),
                    _ => {
                        out.insert(join(key));
                    }
                }
            }
        }
        (Json::Array(a), Json::Array(b)) if a.len() == b.len() => {
            for (x, y) in a.iter().zip(b) {
                differences(x, y, &join("*"), out);
            }
        }
        _ if snapshot == rebuilt => {}
        _ => {
            out.insert(at.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_apply_in_order_and_a_step_without_changes_is_unknown() {
        let initial = Json::from_text(r#"{"data":{"a":1},"metadata":{},"temp_data":{}}"#).unwrap();
        let steps = [
            Json::from_text(r#"{"changes":[{"path":"data.b","old_value":null,"new_value":2}]}"#)
                .unwrap(),
            Json::from_text(r#"{"result":"skipped"}"#).unwrap(),
            Json::from_text(
                r#"{"changes":[{"path":"data.a","old_value":1,"new_value":null,"removed":true}]}"#,
            )
            .unwrap(),
        ];
        let rebuilt = rebuild(&initial, &steps);
        assert_eq!(
            rebuilt[0].as_ref().unwrap().to_text(),
            r#"{"data":{"a":1,"b":2},"metadata":{},"temp_data":{}}"#
        );
        assert!(rebuilt[1].is_none());
        assert_eq!(
            rebuilt[2].as_ref().unwrap().to_text(),
            r#"{"data":{"b":2},"metadata":{},"temp_data":{}}"#
        );
    }

    #[test]
    fn a_change_at_a_root_that_says_replaced_replaces_it() {
        let mut ctx = Json::from_text(r#"{"data":{"b":2},"metadata":{},"temp_data":{}}"#).unwrap();
        apply(
            &mut ctx,
            &Json::from_text(
                r#"{"path":"data","old_value":{"b":2},"new_value":{"a":1},"replaced":true}"#,
            )
            .unwrap(),
        );
        assert_eq!(
            ctx.to_text(),
            r#"{"data":{"a":1},"metadata":{},"temp_data":{}}"#
        );
    }

    #[test]
    fn a_change_at_a_root_merges_into_it() {
        let mut ctx = Json::from_text(r#"{"data":{"b":2},"metadata":{},"temp_data":{}}"#).unwrap();
        apply(
            &mut ctx,
            &Json::from_text(r#"{"path":"data","old_value":{"b":2},"new_value":{"a":1}}"#).unwrap(),
        );
        assert_eq!(
            ctx.to_text(),
            r#"{"data":{"b":2,"a":1},"metadata":{},"temp_data":{}}"#
        );
    }
}
