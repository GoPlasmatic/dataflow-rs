//! Rebuilding a trace's per-step contexts from an initial context and each
//! step's changes: what v4's trace is (R17), and what the UI will do to show
//! a step. Phase 1 runs it against v3's own traces, which carry both the
//! snapshots and the changes, to learn what the changes alone do not rebuild.

use crate::json::Json;
use crate::path;

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

/// One recorded `Change`: `{path, old_value, new_value, removed?}`.
pub fn apply(context: &mut Json, change: &Json) {
    let Some(at) = change.get("path").and_then(Json::as_str) else {
        return;
    };
    if change.get("removed").and_then(Json::as_bool) == Some(true) {
        path::remove(context, at);
    } else {
        let value = change.get("new_value").cloned().unwrap_or(Json::Null);
        path::set(context, at, value);
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
}
