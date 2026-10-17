//! Rebuilding a trace's per-step contexts from an initial context and each
//! step's changes: an independent model of `Trace::contexts`, which the
//! engine's trace tests hold the replay to.

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
