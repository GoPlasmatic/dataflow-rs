use crate::engine::error::{DataflowError, ErrorInfo};
use chrono::{DateTime, Utc};
use datavalue::OwnedDataValue;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::sync::Arc;
use uuid::Uuid;

/// A message flowing through the dataflow engine.
///
/// Construct via [`Message::builder`] for the full API, or use the shortcuts
/// [`Message::new`] (already-owned `Arc<OwnedDataValue>` payload — the perf
/// path) and [`Message::from_value`] (bridges from `serde_json::Value`).
///
/// `context` is held as an [`OwnedDataValue`] tree (not `serde_json::Value`)
/// so the JSONLogic evaluator can borrow it into its arena via
/// `OwnedDataValue::to_arena` with a single deep walk in, and project the
/// result back via `DataValue::to_owned` with a single deep walk out — no
/// `serde_json::Value` round-trip in the hot path. The on-the-wire JSON
/// shape is preserved by datavalue's native `Serialize` / `Deserialize`
/// impls.
///
/// Every other field is encapsulated — read via `id()`, `payload()`,
/// `audit_trail()`, `errors()`, `capture_changes()`; mutate `errors` via
/// [`Message::add_error`]; mutate `context` via [`crate::TaskContext::set`].
/// Direct mutation of `audit_trail` is engine-internal.
#[derive(Debug, Clone)]
pub struct Message {
    pub(crate) id: String,
    pub(crate) payload: Arc<OwnedDataValue>,
    /// Unified context containing `data`, `metadata`, and `temp_data` keys.
    /// Always an `OwnedDataValue::Object`; the engine populates the three
    /// top-level keys at construction. Public for read access (tests do
    /// `message.context["data"]["x"]` lookups); inside handlers prefer
    /// [`crate::TaskContext::set`] which records audit-trail changes.
    pub context: OwnedDataValue,
    pub(crate) audit_trail: Vec<AuditTrail>,
    /// Errors that occurred during message processing. Read via
    /// `errors()`, append via `add_error()`.
    pub(crate) errors: Vec<ErrorInfo>,
    /// When `true` (default), built-in functions emit per-write `Change`
    /// entries into `audit_trail`, capturing `old_value` and `new_value` deep
    /// clones. When `false`, `AuditTrail` entries are still recorded
    /// (workflow_id, task_id, status, timestamp) but `changes` is empty —
    /// the bulk-pipeline fast path. UI debug consumers should leave this at
    /// `true`. Wire shape is unchanged either way.
    pub(crate) capture_changes: bool,
}

// Custom Serialize: stable wire format ({id, payload, context, audit_trail, errors}).
// `capture_changes` is an in-memory hint only — never serialized.
impl Serialize for Message {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Message", 5)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("payload", &self.payload)?;
        state.serialize_field("context", &self.context)?;
        state.serialize_field("audit_trail", &self.audit_trail)?;
        state.serialize_field("errors", &self.errors)?;
        state.end()
    }
}

// Custom Deserialize: mirrors the Serialize shape; no cache field to seed.
// `capture_changes` defaults to `true` for back-compat.
impl<'de> Deserialize<'de> for Message {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct MessageData {
            id: String,
            payload: Arc<OwnedDataValue>,
            context: OwnedDataValue,
            audit_trail: Vec<AuditTrail>,
            errors: Vec<ErrorInfo>,
        }

        let data = MessageData::deserialize(deserializer)?;
        Ok(Message {
            id: data.id,
            payload: data.payload,
            context: data.context,
            audit_trail: data.audit_trail,
            errors: data.errors,
            capture_changes: true,
        })
    }
}

impl Message {
    /// Start building a message. The recommended constructor — chains
    /// `.id(...)`, `.payload(...)` / `.payload_json(...)`, and
    /// `.capture_changes(...)` calls, then `.build()`.
    pub fn builder() -> MessageBuilder {
        MessageBuilder::new()
    }

    /// Construct a message from an already-owned payload `Arc`. The perf
    /// path: zero `serde_json::Value` walk, one Arc refcount bump per
    /// message. Use this from a hot loop with a payload `Arc` shared across
    /// messages (e.g. a benchmark harness or an HTTP handler that receives
    /// already-parsed payloads).
    pub fn new(payload: Arc<OwnedDataValue>) -> Self {
        Self {
            // UUID v7: ms-precision timestamp in the high bits, random tail.
            // Time-ordered and sortable — better for databases/logs than v4
            // (random-only) and the same `rng` backend cost.
            id: Uuid::now_v7().to_string(),
            payload,
            context: empty_context(),
            audit_trail: vec![],
            errors: vec![],
            capture_changes: true,
        }
    }

    /// Construct a message from a `serde_json::Value` payload. Convenience
    /// for code that already speaks serde_json; goes through the
    /// `OwnedDataValue::from(&Value)` bridge (one deep walk).
    pub fn from_value(payload: &JsonValue) -> Self {
        Self::new(Arc::new(OwnedDataValue::from(payload)))
    }

    /// Construct a message from a JSON payload string. Parses with
    /// `serde_json` and bridges into `OwnedDataValue`. Returns
    /// `DataflowError::Deserialization` on parse failure.
    pub fn from_json_str(payload: &str) -> crate::engine::error::Result<Self> {
        let value: JsonValue = serde_json::from_str(payload).map_err(DataflowError::from_serde)?;
        Ok(Self::from_value(&value))
    }

    /// Add an error to the message
    pub fn add_error(&mut self, error: ErrorInfo) {
        self.errors.push(error);
    }

    /// Check if message has errors
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Message id (UUID v7 string by default; caller-supplied if set via
    /// [`MessageBuilder::id`]).
    #[inline]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Original payload as the engine received it. Immutable for the
    /// lifetime of the message — the engine reads it through this Arc and
    /// copies into `context` only as needed by handlers.
    #[inline]
    pub fn payload(&self) -> &OwnedDataValue {
        &self.payload
    }

    /// The shared payload `Arc` itself. Useful when forwarding the same
    /// payload to multiple messages without recloning the underlying
    /// `OwnedDataValue` tree.
    #[inline]
    pub fn payload_arc(&self) -> &Arc<OwnedDataValue> {
        &self.payload
    }

    /// Audit-trail entries recorded by the engine, one per task that ran
    /// (skipped tasks are absent unless `Trace` mode is on).
    #[inline]
    pub fn audit_trail(&self) -> &[AuditTrail] {
        &self.audit_trail
    }

    /// Errors collected while processing — both validation failures and
    /// task errors that the workflow swallowed via `continue_on_error`.
    #[inline]
    pub fn errors(&self) -> &[ErrorInfo] {
        &self.errors
    }

    /// Whether per-write `Change` capture is on. When `false`, audit-trail
    /// entries are still emitted but their `changes` lists are empty —
    /// the bulk-pipeline fast path.
    #[inline]
    pub fn capture_changes(&self) -> bool {
        self.capture_changes
    }

    /// Get a reference to the `data` field in context. Returns
    /// `&OwnedDataValue::Null` if missing (matches `serde_json::Value`'s
    /// `Index` fallback semantics).
    pub fn data(&self) -> &OwnedDataValue {
        &self.context["data"]
    }

    /// Get a reference to the `metadata` field in context.
    pub fn metadata(&self) -> &OwnedDataValue {
        &self.context["metadata"]
    }

    /// Get a reference to the `temp_data` field in context.
    pub fn temp_data(&self) -> &OwnedDataValue {
        &self.context["temp_data"]
    }
}

/// Builder for [`Message`]. Collapses the historical
/// `new` / `with_id` / `from_value` / `without_change_capture` four-way
/// constructor split into a single fluent shape.
///
/// ```
/// use dataflow_rs::Message;
/// use serde_json::json;
///
/// // Minimal: serde_json payload, default UUID id, capture on.
/// let m = Message::builder()
///     .payload_json(&json!({"order": {"total": 1500}}))
///     .build();
/// assert!(m.id().len() > 0);
/// assert!(m.capture_changes());
/// ```
#[must_use = "MessageBuilder must be `.build()` to produce a Message"]
#[derive(Default)]
pub struct MessageBuilder {
    id: Option<String>,
    payload: Option<Arc<OwnedDataValue>>,
    capture_changes: Option<bool>,
}

impl MessageBuilder {
    /// Create an empty builder. Equivalent to [`MessageBuilder::default`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Caller-supplied id (typically a correlation id from upstream).
    /// Defaults to a freshly-generated UUID v7.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Already-owned payload `Arc` — zero serde_json walk, refcount-only
    /// share. Mutually exclusive with [`Self::payload_json`]; whichever is
    /// called last wins.
    pub fn payload(mut self, payload: Arc<OwnedDataValue>) -> Self {
        self.payload = Some(payload);
        self
    }

    /// Construct the payload from a `serde_json::Value`. Goes through the
    /// `OwnedDataValue::from(&Value)` bridge (one deep walk).
    pub fn payload_json(mut self, payload: &JsonValue) -> Self {
        self.payload = Some(Arc::new(OwnedDataValue::from(payload)));
        self
    }

    /// When `false`, built-in functions skip per-write `Change` capture —
    /// audit-trail entries are still recorded but their `changes` list is
    /// empty. Defaults to `true`.
    pub fn capture_changes(mut self, on: bool) -> Self {
        self.capture_changes = Some(on);
        self
    }

    /// Finalize. Defaults: id = UUID v7, payload = `OwnedDataValue::Null`,
    /// capture_changes = `true`.
    pub fn build(self) -> Message {
        Message {
            id: self.id.unwrap_or_else(|| Uuid::now_v7().to_string()),
            payload: self
                .payload
                .unwrap_or_else(|| Arc::new(OwnedDataValue::Null)),
            context: empty_context(),
            audit_trail: vec![],
            errors: vec![],
            capture_changes: self.capture_changes.unwrap_or(true),
        }
    }
}

/// Build the canonical empty context shape used by `Message::new` and
/// `MessageBuilder::build`.
fn empty_context() -> OwnedDataValue {
    OwnedDataValue::Object(vec![
        ("data".to_string(), OwnedDataValue::Object(Vec::new())),
        ("metadata".to_string(), OwnedDataValue::Object(Vec::new())),
        ("temp_data".to_string(), OwnedDataValue::Object(Vec::new())),
    ])
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AuditTrail {
    pub workflow_id: Arc<str>,
    pub task_id: Arc<str>,
    pub timestamp: DateTime<Utc>,
    pub changes: Vec<Change>,
    pub status: usize,
}

/// A single recorded mutation in the audit trail.
///
/// `old_value` and `new_value` are owned `OwnedDataValue`s rather than
/// `Arc<OwnedDataValue>` — eliminates one heap allocation per Change on the
/// hot path. External consumers that need to share a `Change` across threads
/// can wrap it themselves; in-process pipelines (audit-on map mappings) don't
/// pay the Arc cost they were never going to use.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Change {
    pub path: Arc<str>,
    pub old_value: OwnedDataValue,
    pub new_value: OwnedDataValue,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_json_str_parses_valid_payload() {
        let msg =
            Message::from_json_str(r#"{"order": {"total": 42}}"#).expect("valid JSON should parse");
        let payload_json = serde_json::to_value(msg.payload()).unwrap();
        assert_eq!(payload_json, serde_json::json!({"order": {"total": 42}}));
    }

    #[test]
    fn from_json_str_rejects_malformed_payload() {
        let err = Message::from_json_str("{ not json").expect_err("malformed input should fail");
        assert!(matches!(err, DataflowError::Deserialization(_)));
    }
}
