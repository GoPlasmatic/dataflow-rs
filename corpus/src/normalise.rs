//! Replacing the fields that differ between two runs of the same case: clock
//! readings and the engine's version. Each is replaced in place, keeping its
//! key's position, so key order is still compared.

use crate::json::Json;
use crate::path;

pub const TIMESTAMP: &str = "<timestamp>";
pub const DURATION: &str = "<duration>";
pub const ENGINE_VERSION: &str = "<engine_version>";
pub const DEPENDENCY_MESSAGE: &str = "<dependency message>";

/// Prefixes our own code writes before a dependency's error text
/// (`quick-xml` or `datalogic-rs`). Everything after the prefix is replaced
/// with [`DEPENDENCY_MESSAGE`] so a patch bump of the dependency cannot break
/// golden files.
const DEPENDENCY_PREFIXES: &[&str] = &[
    "Failed to parse XML: ",
    "Failed to serialize XML: ",
    "Thrown: ",
    "Operation budget exceeded: ",
];

/// `text` with any dependency-originated tail replaced by
/// [`DEPENDENCY_MESSAGE`].
pub fn dependency_text(text: &str) -> String {
    for prefix in DEPENDENCY_PREFIXES {
        if let Some(at) = text.find(prefix) {
            let keep = at + prefix.len();
            return format!("{}{DEPENDENCY_MESSAGE}", &text[..keep]);
        }
    }
    text.to_owned()
}

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
                normalise_error_field(item, "message");
            }
        }
    }
}

/// A serialized `Run`: normalises dependency-originated error messages on the
/// run and its workflow records.
pub fn run(r: &mut Json) {
    if let Some(err) = r.get_mut("error") {
        normalise_error_field(err, "message");
    }
    if let Some(Json::Array(workflows)) = r.get_mut("workflows") {
        for wf in workflows {
            if let Some(err) = wf.get_mut("error") {
                normalise_error_field(err, "message");
            }
        }
    }
}

fn normalise_error_field(object: &mut Json, key: &str) {
    if let Some(Json::String(text)) = object.get_mut(key) {
        *text = dependency_text(text);
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

    #[test]
    fn dependency_error_details_are_normalised_after_our_prefix() {
        let mut m = Json::from_text(
            r#"{"errors":[{"code":"VALIDATION_ERROR","message":"Task t error: Validation error: Failed to parse XML: ill-formed","timestamp":"t"}]}"#,
        )
        .unwrap();
        message(&mut m, None);
        assert_eq!(
            m.pointer("/errors/0/message"),
            Some(&Json::str(
                "Task t error: Validation error: Failed to parse XML: <dependency message>"
            ))
        );
        let mut r = Json::from_text(
            r#"{"workflows":[{"error":{"code":"BUDGET_EXCEEDED","message":"Operation budget exceeded: 42 ops"}}]}"#,
        )
        .unwrap();
        run(&mut r);
        assert_eq!(
            r.pointer("/workflows/0/error/message"),
            Some(&Json::str(
                "Operation budget exceeded: <dependency message>"
            ))
        );
    }
}
