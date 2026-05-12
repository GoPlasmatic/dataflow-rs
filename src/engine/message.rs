use crate::engine::error::ErrorInfo;
use chrono::{DateTime, Utc};
use datavalue::OwnedDataValue;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::sync::{Arc, LazyLock};
use uuid::Uuid;

/// Process-wide singleton for `Arc<OwnedDataValue::Null>`. The audit-on path
/// captures `old_value` as an `Arc` for every `Change`; on first write to a
/// path the old value is `Null`. Sharing one `Arc` saves the per-mapping
/// `Arc::new(OwnedDataValue::Null)` heap allocation that showed up in the
/// allocator-pressure portion of the flamegraph.
static NULL_ARC: LazyLock<Arc<OwnedDataValue>> =
    LazyLock::new(|| Arc::new(OwnedDataValue::Null));

/// Cheap clone of the shared null `Arc`.
#[inline]
pub fn null_arc() -> Arc<OwnedDataValue> {
    Arc::clone(&NULL_ARC)
}

/// A message flowing through the dataflow engine.
///
/// `context` is held as an [`OwnedDataValue`] tree (not `serde_json::Value`)
/// so the JSONLogic evaluator can borrow it into its arena via
/// `OwnedDataValue::to_arena` with a single deep walk in, and project the
/// result back via `DataValue::to_owned` with a single deep walk out — no
/// `serde_json::Value` round-trip in the hot path. The on-the-wire JSON
/// shape is preserved by datavalue's native `Serialize` / `Deserialize`
/// impls.
#[derive(Debug, Clone)]
pub struct Message {
    pub id: String,
    pub payload: Arc<OwnedDataValue>,
    /// Unified context containing `data`, `metadata`, and `temp_data` keys.
    /// Always an `OwnedDataValue::Object`; the engine populates the three
    /// top-level keys at construction.
    pub context: OwnedDataValue,
    pub audit_trail: Vec<AuditTrail>,
    /// Errors that occurred during message processing
    pub errors: Vec<ErrorInfo>,
    /// When `true` (default), built-in functions emit per-write `Change`
    /// entries into `audit_trail`, capturing `old_value` and `new_value` deep
    /// clones. When `false`, `AuditTrail` entries are still recorded
    /// (workflow_id, task_id, status, timestamp) but `changes` is empty —
    /// the bulk-pipeline fast path. UI debug consumers should leave this at
    /// `true`. Wire shape is unchanged either way.
    pub capture_changes: bool,
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
    pub fn new(payload: Arc<OwnedDataValue>) -> Self {
        Self {
            // UUID v7: ms-precision timestamp in the high bits, random tail.
            // Time-ordered and sortable — better for databases/logs than v4
            // and the same `rng` backend cost.
            id: Uuid::now_v7().to_string(),
            payload,
            context: empty_context(),
            audit_trail: vec![],
            errors: vec![],
            capture_changes: true,
        }
    }

    /// Construct a message with a caller-supplied `id`, bypassing the per-call
    /// `Uuid::new_v4().to_string()` allocation. Useful for benchmarks and for
    /// pipelines that already carry an upstream request/correlation id.
    pub fn with_id(id: impl Into<String>, payload: Arc<OwnedDataValue>) -> Self {
        Self {
            id: id.into(),
            payload,
            context: empty_context(),
            audit_trail: vec![],
            errors: vec![],
            capture_changes: true,
        }
    }

    /// Builder method: disable per-write `Change` capture for this message.
    /// Audit trail entries are still recorded but their `changes` lists are
    /// empty — the bulk-pipeline fast path.
    pub fn without_change_capture(mut self) -> Self {
        self.capture_changes = false;
        self
    }

    /// Construct a message from a `serde_json::Value` payload. Convenience
    /// for code that already speaks serde_json; goes through the
    /// `OwnedDataValue::from(&Value)` bridge.
    pub fn from_value(payload: &JsonValue) -> Self {
        Self::new(Arc::new(OwnedDataValue::from(payload)))
    }

    /// Construct a message from an already-owned `OwnedDataValue` payload —
    /// the native zero-conversion entry point.
    pub fn from_owned(payload: Arc<OwnedDataValue>) -> Self {
        Self::new(payload)
    }

    /// Construct a message from an `Arc<OwnedDataValue>` directly. Same as
    /// `from_owned`; kept as an alias for compatibility with the v4-style
    /// `from_arc` naming.
    #[inline]
    pub fn from_arc(payload: Arc<OwnedDataValue>) -> Self {
        Self::new(payload)
    }

    /// Add an error to the message
    pub fn add_error(&mut self, error: ErrorInfo) {
        self.errors.push(error);
    }

    /// Check if message has errors
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
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

/// Build the canonical empty context shape used by `Message::new`.
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Change {
    pub path: Arc<str>,
    pub old_value: Arc<OwnedDataValue>,
    pub new_value: Arc<OwnedDataValue>,
}
