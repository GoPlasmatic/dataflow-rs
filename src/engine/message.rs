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
/// Internal id storage for [`Message`]. The default UUID v7 id is written
/// into a fixed inline buffer (36 hyphenated lowercase ASCII bytes) — no
/// per-message heap allocation; a caller-supplied id keeps its `String`.
/// Private repr: every public surface exposes it as `&str` via
/// [`Message::id`], and the serialized wire shape (a JSON string) is
/// unchanged.
#[derive(Clone)]
pub(crate) enum MessageId {
    Uuid([u8; 36]),
    Custom(String),
}

impl MessageId {
    /// Generate a fresh UUID v7 id directly into the inline buffer.
    fn new_uuid_v7() -> Self {
        let mut buf = [0u8; 36];
        Uuid::now_v7().hyphenated().encode_lower(&mut buf);
        Self::Uuid(buf)
    }

    fn as_str(&self) -> &str {
        match self {
            // encode_lower fills all 36 bytes with hyphenated lowercase
            // ASCII, so the buffer is always valid UTF-8.
            Self::Uuid(buf) => std::str::from_utf8(buf).expect("uuid ids are ascii"),
            Self::Custom(s) => s,
        }
    }
}

impl std::fmt::Debug for MessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.as_str(), f)
    }
}

#[derive(Debug, Clone)]
pub struct Message {
    pub(crate) id: MessageId,
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
    /// Routing bucket `0..=99` for a workflow traffic split
    /// ([`crate::Workflow::rollout`]).
    ///
    /// Deliberately **not** stored in `context`, so it never appears in `data` /
    /// `metadata` / `temp_data` and never has to be stripped before the context
    /// is serialized — that is the point of the field.
    ///
    /// Like `capture_changes`, an in-memory hint: the hand-written `Serialize` /
    /// `Deserialize` impls keep the 5-field wire shape, so the bucket does not
    /// survive a JSON round trip.
    pub(crate) routing_bucket: Option<u8>,
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
        state.serialize_field("id", &self.id.as_str())?;
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
        Ok(Self {
            id: MessageId::Custom(data.id),
            payload: data.payload,
            context: data.context,
            audit_trail: data.audit_trail,
            errors: data.errors,
            capture_changes: true,
            routing_bucket: None,
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
            // (random-only) and the same `rng` backend cost. Encoded into the
            // inline id buffer — no `String` allocation per message.
            id: MessageId::new_uuid_v7(),
            payload,
            context: empty_context(),
            audit_trail: vec![],
            errors: vec![],
            capture_changes: true,
            routing_bucket: None,
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
        self.id.as_str()
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

    /// Routing bucket for the traffic-split gate, if one was set.
    ///
    /// `None` means every workflow admits this message regardless of its
    /// [`crate::Workflow::rollout`].
    #[inline]
    pub fn routing_bucket(&self) -> Option<u8> {
        self.routing_bucket
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
    data: Option<OwnedDataValue>,
    metadata: Option<OwnedDataValue>,
    temp_data: Option<OwnedDataValue>,
    routing_bucket: Option<u8>,
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

    /// Seed `context.data`.
    ///
    /// Replaces the empty `Object` that [`Self::build`] would otherwise install;
    /// the other two root fields are unaffected. Seeding records no audit-trail
    /// entry and no `Change` — it is initial state, not a mutation.
    ///
    /// Keys are taken **literally**: unlike
    /// [`crate::engine::utils::set_nested_value`], a key containing `.` stays a
    /// single key and a leading `#` is not stripped.
    ///
    /// A non-`Object` value is **ignored**, preserving the crate-wide invariant
    /// that the three root fields are always objects. Calling this twice keeps
    /// the last value.
    ///
    /// ```
    /// use dataflow_rs::Message;
    /// use serde_json::json;
    ///
    /// let m = Message::builder().data_json(&json!({"order": {"total": 1500}})).build();
    /// assert_eq!(m.data()["order"]["total"], json!(1500).into());
    /// ```
    pub fn data(mut self, data: OwnedDataValue) -> Self {
        if data.is_object() {
            self.data = Some(data);
        }
        self
    }

    /// [`Self::data`] from a `serde_json::Value` (one `OwnedDataValue::from`
    /// deep walk).
    pub fn data_json(self, data: &JsonValue) -> Self {
        self.data(OwnedDataValue::from(data))
    }

    /// Seed `context.metadata` — request headers, correlation ids, routing
    /// hints.
    ///
    /// `Engine::process_message` adds `processed_at` and `engine_version` on top
    /// of whatever is seeded here; a seeded `channel` key is overwritten by
    /// `process_message_for_channel`. Same literal-key and non-object rules as
    /// [`Self::data`].
    ///
    /// ```
    /// use dataflow_rs::Message;
    /// use serde_json::json;
    ///
    /// let m = Message::builder().metadata_json(&json!({"source": "api"})).build();
    /// assert_eq!(m.metadata()["source"], json!("api").into());
    /// ```
    pub fn metadata(mut self, metadata: OwnedDataValue) -> Self {
        if metadata.is_object() {
            self.metadata = Some(metadata);
        }
        self
    }

    /// [`Self::metadata`] from a `serde_json::Value`.
    pub fn metadata_json(self, metadata: &JsonValue) -> Self {
        self.metadata(OwnedDataValue::from(metadata))
    }

    /// Seed `context.temp_data` — scratch space for intermediate task output.
    ///
    /// Same literal-key and non-object rules as [`Self::data`].
    ///
    /// ```
    /// use dataflow_rs::Message;
    /// use serde_json::json;
    ///
    /// let m = Message::builder().temp_data_json(&json!({"scratch": 1})).build();
    /// assert_eq!(m.temp_data()["scratch"], json!(1).into());
    /// ```
    pub fn temp_data(mut self, temp_data: OwnedDataValue) -> Self {
        if temp_data.is_object() {
            self.temp_data = Some(temp_data);
        }
        self
    }

    /// [`Self::temp_data`] from a `serde_json::Value`.
    pub fn temp_data_json(self, temp_data: &JsonValue) -> Self {
        self.temp_data(OwnedDataValue::from(temp_data))
    }

    /// Routing bucket `0..=99` for [`crate::Workflow::rollout`] matching.
    ///
    /// Values `>= 100` are clamped to `99`, keeping the builder infallible like
    /// the rest of its methods. A message with no bucket is admitted by every
    /// workflow, split or not.
    ///
    /// ```
    /// use dataflow_rs::Message;
    ///
    /// assert_eq!(Message::builder().routing_bucket(7).build().routing_bucket(), Some(7));
    /// assert_eq!(Message::builder().routing_bucket(200).build().routing_bucket(), Some(99));
    /// assert_eq!(Message::builder().build().routing_bucket(), None);
    /// ```
    pub fn routing_bucket(mut self, bucket: u8) -> Self {
        self.routing_bucket = Some(bucket.min(99));
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
            id: self
                .id
                .map(MessageId::Custom)
                .unwrap_or_else(MessageId::new_uuid_v7),
            payload: self
                .payload
                .unwrap_or_else(|| Arc::new(OwnedDataValue::Null)),
            context: context_from(self.data, self.metadata, self.temp_data),
            audit_trail: vec![],
            errors: vec![],
            capture_changes: self.capture_changes.unwrap_or(true),
            routing_bucket: self.routing_bucket,
        }
    }
}

/// Build the canonical context object, substituting any caller-seeded root field
/// for the empty `Object`.
///
/// Key order is fixed at `data, metadata, temp_data` so the serialized shape is
/// identical whether or not a seed was supplied.
fn context_from(
    data: Option<OwnedDataValue>,
    metadata: Option<OwnedDataValue>,
    temp_data: Option<OwnedDataValue>,
) -> OwnedDataValue {
    fn slot(v: Option<OwnedDataValue>) -> OwnedDataValue {
        v.unwrap_or_else(|| OwnedDataValue::Object(Vec::new()))
    }
    OwnedDataValue::Object(vec![
        ("data".to_string(), slot(data)),
        ("metadata".to_string(), slot(metadata)),
        ("temp_data".to_string(), slot(temp_data)),
    ])
}

/// Build the canonical empty context shape used by `Message::new`. One
/// implementation of the shape, shared with the seeded path.
fn empty_context() -> OwnedDataValue {
    context_from(None, None, None)
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AuditTrail {
    pub workflow_id: Arc<str>,
    pub task_id: Arc<str>,
    pub timestamp: DateTime<Utc>,
    pub changes: Vec<Change>,
    pub status: usize,
    /// Loop counter value for the sweep that produced this entry, for
    /// workflows carrying a [`crate::engine::workflow::LoopConfig`]; `None`
    /// otherwise.
    ///
    /// The counter rather than a sweep ordinal: `increment >= 1` makes it
    /// strictly increasing, so it identifies the iteration *and* carries the
    /// business meaning — in the per-item pattern it is the array index this
    /// entry refers to. Recorded even when the loop leaves its counter
    /// unnamed, since the engine tracks the value either way.
    ///
    /// Skipped when `None`, so a non-looping workflow's audit JSON is
    /// byte-identical to what it was before loops existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_counter: Option<i64>,
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
    fn audit_trail_loop_counter_is_absent_from_json_when_none() {
        // A non-looping workflow must keep the historical wire shape byte for
        // byte — dataflow-ui and any stored audit JSON depend on it.
        let entry = AuditTrail {
            workflow_id: Arc::from("w"),
            task_id: Arc::from("t"),
            timestamp: Utc::now(),
            changes: vec![],
            status: 200,
            loop_counter: None,
        };
        let json = serde_json::to_value(&entry).expect("should serialize");
        assert!(json.get("loop_counter").is_none());

        let with_counter = AuditTrail {
            loop_counter: Some(7),
            ..entry
        };
        assert_eq!(
            serde_json::to_value(&with_counter).expect("should serialize")["loop_counter"],
            serde_json::json!(7)
        );
    }

    #[test]
    fn audit_trail_without_a_loop_counter_key_deserializes() {
        // Audit JSON written before loops existed must still round-trip.
        let entry: AuditTrail = serde_json::from_value(serde_json::json!({
            "workflow_id": "w",
            "task_id": "t",
            "timestamp": "2026-08-11T00:00:00Z",
            "changes": [],
            "status": 200
        }))
        .expect("legacy audit JSON should deserialize");
        assert_eq!(entry.loop_counter, None);
    }

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

    #[test]
    fn builder_with_no_seed_matches_the_historical_empty_context() {
        let m = Message::builder().build();
        let v = serde_json::to_value(&m).unwrap();
        // Key order is part of the shape, so assert on the serialized object.
        let ctx = v["context"].as_object().unwrap();
        assert_eq!(
            ctx.keys().collect::<Vec<_>>(),
            vec!["data", "metadata", "temp_data"]
        );
        assert_eq!(
            v["context"],
            serde_json::json!({
                "data": {}, "metadata": {}, "temp_data": {}
            })
        );
    }

    #[test]
    fn each_setter_lands_in_its_own_root_field() {
        let m = Message::builder()
            .data_json(&serde_json::json!({"d": 1}))
            .build();
        assert_eq!(
            serde_json::Value::from(m.data()),
            serde_json::json!({"d": 1})
        );
        assert_eq!(serde_json::Value::from(m.metadata()), serde_json::json!({}));
        assert_eq!(
            serde_json::Value::from(m.temp_data()),
            serde_json::json!({})
        );

        let m = Message::builder()
            .metadata_json(&serde_json::json!({"m": 1}))
            .build();
        assert_eq!(
            serde_json::Value::from(m.metadata()),
            serde_json::json!({"m": 1})
        );
        assert_eq!(serde_json::Value::from(m.data()), serde_json::json!({}));

        let m = Message::builder()
            .temp_data_json(&serde_json::json!({"t": 1}))
            .build();
        assert_eq!(
            serde_json::Value::from(m.temp_data()),
            serde_json::json!({"t": 1})
        );
        assert_eq!(serde_json::Value::from(m.data()), serde_json::json!({}));
    }

    #[test]
    fn the_owned_and_json_setter_forms_agree() {
        let v = serde_json::json!({"a": {"b": [1, 2]}});
        let via_json = Message::builder().data_json(&v).build();
        let via_owned = Message::builder().data(OwnedDataValue::from(&v)).build();
        assert_eq!(via_json.context, via_owned.context);
    }

    #[test]
    fn seeding_records_no_audit_entry_or_change() {
        for capture in [true, false] {
            let m = Message::builder()
                .capture_changes(capture)
                .data_json(&serde_json::json!({"d": 1}))
                .build();
            assert!(
                m.audit_trail().is_empty(),
                "seeding is initial state, not a mutation"
            );
            assert_eq!(
                serde_json::Value::from(m.data()),
                serde_json::json!({"d": 1})
            );
        }
    }

    #[test]
    fn calling_a_setter_twice_keeps_the_last_value() {
        let m = Message::builder()
            .data_json(&serde_json::json!({"first": 1}))
            .data_json(&serde_json::json!({"second": 2}))
            .build();
        assert_eq!(
            serde_json::Value::from(m.data()),
            serde_json::json!({"second": 2})
        );
    }

    #[test]
    fn an_empty_object_seed_is_indistinguishable_from_no_seed() {
        let seeded = Message::builder().data_json(&serde_json::json!({})).build();
        let bare = Message::builder().build();
        assert_eq!(seeded.context, bare.context);
    }

    #[test]
    fn seed_keys_are_literal_not_paths() {
        use crate::engine::utils::get_nested_value;

        let m = Message::builder()
            .metadata_json(&serde_json::json!({"a.b": 1}))
            .build();
        // One literal key, not nested.
        assert_eq!(
            serde_json::Value::from(m.metadata()),
            serde_json::json!({"a.b": 1})
        );
        assert!(
            get_nested_value(&m.context, "metadata.a.b").is_none(),
            "a dotted key must not become a nested path"
        );

        // Leading `#` is not stripped.
        let m = Message::builder()
            .metadata_json(&serde_json::json!({"#20": 1}))
            .build();
        assert_eq!(
            serde_json::Value::from(m.metadata()),
            serde_json::json!({"#20": 1})
        );

        // A numeric-string key stays an object key, not an Array.
        let m = Message::builder()
            .data_json(&serde_json::json!({"0": 1}))
            .build();
        assert!(matches!(m.data(), OwnedDataValue::Object(_)));
        assert_eq!(
            serde_json::Value::from(m.data()),
            serde_json::json!({"0": 1})
        );
    }

    #[test]
    fn non_ascii_keys_and_values_round_trip() {
        let v = serde_json::json!({"régión": "東京"});
        let m = Message::builder().data_json(&v).build();
        assert_eq!(serde_json::Value::from(m.data()), v);
        assert_eq!(serde_json::to_value(&m).unwrap()["context"]["data"], v);
    }

    #[test]
    fn nested_seeds_resolve_through_the_path_api() {
        use crate::engine::utils::get_nested_value;

        let m = Message::builder()
            .data_json(&serde_json::json!({"order": {"items": [1, 2]}}))
            .build();
        assert_eq!(
            get_nested_value(&m.context, "data.order.items.1"),
            Some(&OwnedDataValue::from(&serde_json::json!(2)))
        );
    }

    #[test]
    fn non_object_seeds_are_ignored_to_preserve_the_context_invariant() {
        // The recorded decision: the three root fields are always Objects.
        // Storing a scalar verbatim would make `set_processing_metadata` bail out
        // and silently drop `processed_at` / `engine_version`.
        for bad in [
            serde_json::json!("scalar"),
            serde_json::json!([1, 2]),
            serde_json::json!(null),
            serde_json::json!(7),
        ] {
            let m = Message::builder()
                .data_json(&bad)
                .metadata_json(&bad)
                .temp_data_json(&bad)
                .build();
            assert_eq!(
                serde_json::Value::from(&m.context),
                serde_json::json!({"data": {}, "metadata": {}, "temp_data": {}}),
                "non-object seed {bad} must be ignored"
            );
        }
    }
}
