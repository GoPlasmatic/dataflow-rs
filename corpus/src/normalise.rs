//! Replacing the fields that differ between two runs of the same case: clock
//! readings and the engine's version. Each is replaced in place, keeping its
//! key's position, so key order is still compared.

use crate::json::Json;
use crate::path;

pub const TIMESTAMP: &str = "<timestamp>";
pub const DURATION: &str = "<duration>";
pub const ENGINE_VERSION: &str = "<engine_version>";

/// A wire message: `{id, payload, context, audit_trail, errors}`.
/// `error_context` is the engine's error-context path, when it has one.
pub fn message(m: &mut Json, error_context: Option<&str>) {
    if let Some(ctx) = m.get_mut("context") {
        context(ctx, error_context);
    }
    for list in ["audit_trail", "errors"] {
        if let Some(Json::Array(items)) = m.get_mut(list) {
            for item in items {
                replace(item, "timestamp", TIMESTAMP);
            }
        }
    }
}

/// A context: `{data, metadata, temp_data}`.
pub fn context(ctx: &mut Json, error_context: Option<&str>) {
    if let Some(metadata) = ctx.get_mut("metadata") {
        replace(metadata, "processed_at", TIMESTAMP);
        replace(metadata, "engine_version", ENGINE_VERSION);
    }
    if let Some(at) = error_context
        && let Some(Json::Array(records)) = path::get_mut(ctx, at)
    {
        for record in records {
            replace(record, "timestamp", TIMESTAMP);
        }
    }
}

/// One trace step, with its snapshot message and its mapping contexts.
pub fn trace_step(step: &mut Json, error_context: Option<&str>) {
    replace(step, "started_at", TIMESTAMP);
    replace(step, "duration_us", DURATION);
    if let Some(m @ Json::Object(_)) = step.get_mut("message") {
        message(m, error_context);
    }
    if let Some(Json::Array(contexts)) = step.get_mut("mapping_contexts") {
        for ctx in contexts {
            context(ctx, error_context);
        }
    }
}

/// Replace `key`'s value with `placeholder` when it is present and not null.
fn replace(object: &mut Json, key: &str, placeholder: &str) {
    if let Some(value) = object.get_mut(key)
        && *value != Json::Null
    {
        *value = Json::str(placeholder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volatile_fields_are_replaced_in_place() {
        let mut m = Json::from_text(r#"{"id":"m1","payload":{},"context":{"data":{},"metadata":{"processed_at":"2026-10-12T10:00:00Z","engine_version":"3.15.0","x":1},"temp_data":{}},"audit_trail":[{"workflow_id":"w","task_id":"t","timestamp":"2026","changes":[],"status":200}],"errors":[{"code":"C","message":"m","timestamp":"2026"}]}"#).unwrap();
        message(&mut m, None);
        assert_eq!(
            m.to_text(),
            r#"{"id":"m1","payload":{},"context":{"data":{},"metadata":{"processed_at":"<timestamp>","engine_version":"<engine_version>","x":1},"temp_data":{}},"audit_trail":[{"workflow_id":"w","task_id":"t","timestamp":"<timestamp>","changes":[],"status":200}],"errors":[{"code":"C","message":"m","timestamp":"<timestamp>"}]}"#
        );
    }

    #[test]
    fn error_context_records_lose_their_timestamps() {
        let mut m = Json::from_text(
            r#"{"context":{"metadata":{"failures":[{"code":"C","timestamp":"t"}]}}}"#,
        )
        .unwrap();
        message(&mut m, Some("metadata.failures"));
        assert_eq!(
            m.pointer("/context/metadata/failures/0/timestamp"),
            Some(&Json::str(TIMESTAMP))
        );
    }

    #[test]
    fn a_trace_step_loses_its_clock_and_its_snapshot_is_normalised() {
        let mut s = Json::from_text(r#"{"workflow_id":"w","started_at":"t","duration_us":12,"message":{"context":{"metadata":{"processed_at":"t"}}},"mapping_contexts":[{"metadata":{"engine_version":"3"}}]}"#).unwrap();
        trace_step(&mut s, None);
        assert_eq!(
            s.to_text(),
            r#"{"workflow_id":"w","started_at":"<timestamp>","duration_us":"<duration>","message":{"context":{"metadata":{"processed_at":"<timestamp>"}}},"mapping_contexts":[{"metadata":{"engine_version":"<engine_version>"}}]}"#
        );
    }
}
