//! The message and its audit trail, on v3's wire shape.

use crate::base::{Change, ErrorRecord, Result};
use crate::expr::Path;
use chrono::{DateTime, Utc};
use datavalue::OwnedDataValue;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// One task's record, v3's `AuditTrail` on the wire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AuditEntry {
    /// The workflow the task belongs to.
    pub workflow_id: Arc<str>,
    /// The task.
    pub task_id: Arc<str>,
    /// When the entry was recorded.
    pub timestamp: DateTime<Utc>,
    /// The writes the task made, when changes are captured.
    pub changes: Vec<Change>,
    /// The task's status code.
    pub status: usize,
    /// The loop counter of the sweep that produced the entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_counter: Option<i64>,
    /// The fan-out element the entry records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_index: Option<usize>,
}

/// Which audit entries a run keeps on the message. `Full` by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuditMode {
    /// Keep every entry.
    #[default]
    Full,
    /// Keep the most recent `n` entries; `Last(0)` keeps none.
    Last(usize),
    /// Keep none.
    Off,
}

/// `{data, metadata, temp_data}`: always these three keys, in that order.
/// Each root starts as an object, and a `map` to a root of a value that is
/// not an object replaces it, as v3's does.
#[derive(Clone, Debug, PartialEq)]
pub struct Context {
    root: OwnedDataValue,
}

impl Serialize for Context {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        self.root.serialize(s)
    }
}

fn slot(seed: OwnedDataValue) -> OwnedDataValue {
    if seed.is_object() {
        seed
    } else {
        OwnedDataValue::Object(Vec::new())
    }
}

impl Context {
    fn new(data: OwnedDataValue, metadata: OwnedDataValue) -> Self {
        Self {
            root: OwnedDataValue::Object(vec![
                ("data".to_string(), slot(data)),
                ("metadata".to_string(), slot(metadata)),
                ("temp_data".to_string(), slot(OwnedDataValue::Null)),
            ]),
        }
    }

    fn root(&self, key: &str) -> &OwnedDataValue {
        &self.root[key]
    }

    /// The `data` root.
    pub fn data(&self) -> &OwnedDataValue {
        self.root("data")
    }

    /// The `metadata` root.
    pub fn metadata(&self) -> &OwnedDataValue {
        self.root("metadata")
    }

    /// The `temp_data` root.
    pub fn temp_data(&self) -> &OwnedDataValue {
        self.root("temp_data")
    }

    /// The value at `path`, if any.
    pub fn get(&self, path: &Path) -> Option<&OwnedDataValue> {
        path.get(&self.root)
    }

    /// Writes `value` at `path` by [`Path::set`]'s rules: a root takes only
    /// an object.
    pub fn set(&mut self, path: &Path, value: OwnedDataValue) -> Result<()> {
        path.set(&mut self.root, value)
    }

    /// Removes and returns the value at `path`. A root cannot be removed.
    pub fn remove(&mut self, path: &Path) -> Result<Option<OwnedDataValue>> {
        path.remove(&mut self.root)
    }

    /// The object holding the three roots: what expressions evaluate against.
    pub fn as_value(&self) -> &OwnedDataValue {
        &self.root
    }
}

/// Pure wire data: `{id, payload, context, audit_trail, errors}`, in that
/// order.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    id: String,
    payload: Arc<OwnedDataValue>,
    context: Context,
    audit: Vec<AuditEntry>,
    errors: Vec<ErrorRecord>,
}

impl Serialize for Message {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("Message", 5)?;
        st.serialize_field("id", &self.id)?;
        st.serialize_field("payload", &self.payload)?;
        st.serialize_field("context", &self.context)?;
        st.serialize_field("audit_trail", &self.audit)?;
        st.serialize_field("errors", &self.errors)?;
        st.end()
    }
}

/// The parts a run borrows separately (05: only `run` holds a `&mut Message`).
#[derive(Debug)]
pub(crate) struct Parts<'a> {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "phase 6's observer events carry the message id")
    )]
    pub(crate) id: &'a str,
    pub(crate) payload: &'a OwnedDataValue,
    pub(crate) context: &'a mut OwnedDataValue,
    pub(crate) audit: &'a mut Vec<AuditEntry>,
    pub(crate) errors: &'a mut Vec<ErrorRecord>,
}

thread_local! {
    /// The v7 context a thread's message ids are made with. One per thread,
    /// so making an id takes no lock: uuid's process-wide context (v3's
    /// `Uuid::now_v7`) serialises every message's construction across a
    /// host's worker threads behind one mutex.
    static IDS: uuid::ContextV7 = const { uuid::ContextV7::new() };
}

impl Message {
    /// A message with a fresh UUID v7 id, as v3. Ids made on one thread sort
    /// in the order they were made; ids made in the same millisecond on two
    /// threads are distinct but not ordered. A payload passed as an `Arc` is
    /// shared, not copied: messages built from one `Arc` hold one payload.
    pub fn new(payload: impl Into<Arc<OwnedDataValue>>) -> Self {
        Self::from_parts(payload, OwnedDataValue::Null, OwnedDataValue::Null)
    }

    /// A message seeded with `data` and `metadata`. A seed that is not an
    /// object is ignored, as v3. A payload passed as an `Arc` is shared, not
    /// copied.
    pub fn from_parts(
        payload: impl Into<Arc<OwnedDataValue>>,
        data: OwnedDataValue,
        metadata: OwnedDataValue,
    ) -> Self {
        // Encoded into a buffer, not `to_string`, which formats through
        // `Display`: the same lowercase hyphenated text.
        let mut id = [0u8; uuid::fmt::Hyphenated::LENGTH];
        let uuid = uuid::Uuid::new_v7(uuid::Timestamp::now(
            uuid::timestamp::context::ThreadLocalContext::new(&IDS),
        ));
        let id = String::from(&*uuid.hyphenated().encode_lower(&mut id));
        Self {
            id,
            payload: payload.into(),
            context: Context::new(data, metadata),
            audit: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// A message whose payload is `payload`, converted.
    pub fn from_json(payload: &Value) -> Self {
        Self::new(OwnedDataValue::from(payload))
    }

    /// Replaces the id.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// The id: a UUID v7 unless set.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The payload the message arrived with. It is not part of the evaluation
    /// context.
    pub fn payload(&self) -> &OwnedDataValue {
        &self.payload
    }

    /// The context.
    pub fn context(&self) -> &Context {
        &self.context
    }

    /// The context, mutably.
    pub fn context_mut(&mut self) -> &mut Context {
        &mut self.context
    }

    /// The audit entries the run kept.
    pub fn audit(&self) -> &[AuditEntry] {
        &self.audit
    }

    /// The errors recorded while processing.
    pub fn errors(&self) -> &[ErrorRecord] {
        &self.errors
    }

    /// The message as a JSON value holding `id`, `payload`, `context`,
    /// `audit_trail` and `errors`.
    ///
    /// A [`serde_json::Value`] does not keep key order here (the workspace
    /// builds serde_json without `preserve_order`), so the keys come back
    /// sorted. The wire text, in wire order, is `serde_json::to_string(&message)`.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("a message always serializes")
    }

    pub(crate) fn parts(&mut self) -> Parts<'_> {
        let Self {
            id,
            payload,
            context,
            audit,
            errors,
        } = self;
        Parts {
            id,
            payload,
            context: &mut context.root,
            audit,
            errors,
        }
    }
}

/// Whether `mode` keeps any entry: `Off` and `Last(0)` keep none, so the
/// engine records no changes under them (I67) and the recorder builds no
/// entry.
pub(crate) fn keeps_entries(mode: AuditMode) -> bool {
    !matches!(mode, AuditMode::Off | AuditMode::Last(0))
}

/// Appends `entry` under `mode`, trimming `Last(n)` in v3's batches: the Vec
/// holds up to `2n` entries until [`finish_audit`].
pub(crate) fn record_audit(audit: &mut Vec<AuditEntry>, mode: AuditMode, entry: AuditEntry) {
    match mode {
        AuditMode::Full => audit.push(entry),
        AuditMode::Off | AuditMode::Last(0) => {}
        AuditMode::Last(n) => {
            audit.push(entry);
            if audit.len() >= n.saturating_mul(2) {
                let excess = audit.len() - n;
                audit.drain(..excess);
            }
        }
    }
}

/// Trims `Last(n)` to exactly `n`: the message no longer carries the mode.
pub(crate) fn finish_audit(audit: &mut Vec<AuditEntry>, mode: AuditMode) {
    match mode {
        AuditMode::Full => {}
        AuditMode::Off | AuditMode::Last(0) => audit.clear(),
        AuditMode::Last(n) => {
            let excess = audit.len().saturating_sub(n);
            audit.drain(..excess);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::ErrorKind;
    use dataflow_rs_v3 as v3;
    use serde_json::json;

    fn text<T: Serialize>(v: &T) -> String {
        serde_json::to_string(v).unwrap()
    }

    fn v4_msg(payload: &Value, data: &Value, metadata: &Value) -> Message {
        Message::from_parts(
            OwnedDataValue::from(payload),
            OwnedDataValue::from(data),
            OwnedDataValue::from(metadata),
        )
        .with_id("m")
    }

    fn v3_msg(payload: &Value, data: &Value, metadata: &Value) -> v3::engine::message::Message {
        v3::engine::message::Message::builder()
            .id("m")
            .payload_json(payload)
            .data_json(data)
            .metadata_json(metadata)
            .build()
    }

    #[test]
    fn the_wire_shape_is_v3s() {
        let payload = json!({"a": [1, {"b": null}], "z": "s", "k": 1.5});
        let data = json!({"order": {"items": [1, 2, {"x": true}], "total": 10}, "a": 1});
        let meta = json!({"source": "api", "tags": ["x", "y"]});
        assert_eq!(
            text(&v4_msg(&payload, &data, &meta)),
            text(&v3_msg(&payload, &data, &meta))
        );
        let (n, e) = (json!(null), json!({}));
        assert_eq!(text(&v4_msg(&n, &e, &e)), text(&v3_msg(&n, &e, &e)));
    }

    #[test]
    fn a_non_object_seed_is_ignored() {
        let (d, m, p) = (json!(7), json!("x"), json!(null));
        let v4 = v4_msg(&p, &d, &m);
        assert_eq!(
            text(v4.context()),
            r#"{"data":{},"metadata":{},"temp_data":{}}"#
        );
        assert_eq!(text(&v4), text(&v3_msg(&p, &d, &m)));
    }

    #[test]
    fn audit_entries_and_changes_serialize_as_v3s() {
        let ts = Utc::now();
        for (lc, ei, removed) in [
            (None, None, false),
            (Some(3), Some(2), true),
            (Some(-1), None, false),
        ] {
            let c3 = v3::engine::message::Change {
                path: Arc::from("data.a"),
                old_value: OwnedDataValue::from(&json!({"o": 1})),
                new_value: OwnedDataValue::from(&json!([1])),
                removed,
            };
            let mut c4 = Change::write(
                "data.a",
                OwnedDataValue::from(&json!({"o": 1})),
                OwnedDataValue::from(&json!([1])),
            );
            c4.removed = removed;
            assert_eq!(text(&c4), text(&c3));
            let a3 = v3::engine::message::AuditTrail {
                workflow_id: Arc::from("w"),
                task_id: Arc::from("t"),
                timestamp: ts,
                changes: vec![c3],
                status: 200,
                loop_counter: lc,
                element_index: ei,
            };
            let a4 = AuditEntry {
                workflow_id: Arc::from("w"),
                task_id: Arc::from("t"),
                timestamp: ts,
                changes: vec![c4],
                status: 200,
                loop_counter: lc,
                element_index: ei,
            };
            assert_eq!(text(&a4), text(&a3));
        }
    }

    #[test]
    fn the_id_is_the_uuids_lowercase_hyphenated_text() {
        let m = Message::new(OwnedDataValue::Null);
        let id = uuid::Uuid::parse_str(m.id()).unwrap();
        assert_eq!(m.id(), id.hyphenated().to_string());
        assert_eq!(m.id(), m.id().to_lowercase());
    }

    #[test]
    fn a_new_message_has_a_uuid_v7_id() {
        let m = Message::new(OwnedDataValue::Null);
        let id = uuid::Uuid::parse_str(m.id()).unwrap();
        assert_eq!(id.get_version_num(), 7);
        assert_ne!(m.id(), Message::new(OwnedDataValue::Null).id());
        assert_eq!(
            Message::from_json(&json!({"a": 1})).payload(),
            &OwnedDataValue::from(&json!({"a": 1}))
        );
    }

    #[test]
    fn ids_made_on_one_thread_sort_in_the_order_they_were_made() {
        let ids: Vec<String> = (0..10_000)
            .map(|_| Message::new(OwnedDataValue::Null).id().to_owned())
            .collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn ids_made_on_several_threads_are_distinct() {
        let threads: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(|| {
                    (0..10_000)
                        .map(|_| Message::new(OwnedDataValue::Null).id().to_owned())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let ids: std::collections::HashSet<String> = threads
            .into_iter()
            .flat_map(|t| t.join().unwrap())
            .collect();
        assert_eq!(ids.len(), 40_000);
    }

    #[test]
    fn messages_built_from_one_arc_share_its_payload() {
        let payload = Arc::new(OwnedDataValue::from(&json!({"a": [1, 2, 3]})));
        let a = Message::new(Arc::clone(&payload));
        let b = Message::from_parts(
            Arc::clone(&payload),
            OwnedDataValue::Null,
            OwnedDataValue::Null,
        );
        assert_eq!(Arc::strong_count(&payload), 3);
        assert!(std::ptr::eq(a.payload(), b.payload()));
        assert!(std::ptr::eq(a.payload(), &*payload));
    }

    fn entry(task: &str) -> AuditEntry {
        AuditEntry {
            workflow_id: Arc::from("w"),
            task_id: Arc::from(task),
            timestamp: Utc::now(),
            changes: Vec::new(),
            status: 200,
            loop_counter: None,
            element_index: None,
        }
    }

    #[tokio::test]
    async fn record_audit_matches_v3s_trim() {
        let tasks: Vec<Value> = (0..10)
            .map(|i| {
                json!({"id": format!("t{i}"), "name": "t", "function": {"name": "map", "input": {
                    "mappings": [{"path": "data.x", "logic": i}]}}})
            })
            .collect();
        let workflow =
            v3::Workflow::from_json(&json!({"id": "w", "name": "w", "tasks": tasks}).to_string())
                .unwrap();
        let engine = v3::Engine::builder()
            .with_workflow(workflow)
            .build()
            .unwrap();
        for mode in [
            AuditMode::Full,
            AuditMode::Off,
            AuditMode::Last(0),
            AuditMode::Last(1),
            AuditMode::Last(3),
            AuditMode::Last(10),
            AuditMode::Last(25),
        ] {
            let mut m3 = v3::engine::message::Message::builder()
                .audit_mode(mode_v3(mode))
                .build();
            engine.process_message(&mut m3).await.unwrap();
            let want: Vec<String> = m3
                .audit_trail()
                .iter()
                .map(|e| e.task_id.to_string())
                .collect();
            let mut audit = Vec::new();
            for i in 0..10 {
                record_audit(&mut audit, mode, entry(&format!("t{i}")));
                if let AuditMode::Last(n) = mode {
                    assert!(audit.len() < (2 * n).max(1), "{mode:?} bounds the Vec");
                }
            }
            finish_audit(&mut audit, mode);
            let got: Vec<String> = audit.iter().map(|e| e.task_id.to_string()).collect();
            assert_eq!(got, want, "{mode:?}");
        }
    }

    fn mode_v3(mode: AuditMode) -> v3::AuditMode {
        match mode {
            AuditMode::Full => v3::AuditMode::Full,
            AuditMode::Off => v3::AuditMode::Off,
            AuditMode::Last(n) => v3::AuditMode::Last(n),
        }
    }

    #[test]
    fn context_set_at_a_root_takes_only_an_object() {
        let mut m = Message::new(OwnedDataValue::Null);
        let root: Path = "data".parse().unwrap();
        let err = m
            .context_mut()
            .set(&root, OwnedDataValue::from(&json!(1)))
            .unwrap_err();
        assert_eq!(err.kind(), &ErrorKind::Config);
        m.context_mut()
            .set(&root, OwnedDataValue::from(&json!({"a": 1})))
            .unwrap();
        let a: Path = "data.a".parse().unwrap();
        assert_eq!(m.context().get(&a), Some(&OwnedDataValue::from(&json!(1))));
        assert_eq!(m.context().data(), &OwnedDataValue::from(&json!({"a": 1})));
        assert_eq!(m.context().as_value()["data"], *m.context().data());
        assert!(m.context().metadata().is_object() && m.context().temp_data().is_object());
    }

    #[test]
    fn to_json_holds_the_wire_content_but_not_its_order() {
        let m = v4_msg(&json!({"b": 1, "a": 2}), &json!({"x": [1]}), &json!({}));
        let wire = serde_json::to_string(&m).unwrap();
        assert!(wire.starts_with(r#"{"id":"#), "{wire}");
        assert_eq!(m.to_json(), serde_json::from_str::<Value>(&wire).unwrap());
    }

    #[test]
    fn context_remove_refuses_a_root() {
        let mut m = Message::from_parts(
            OwnedDataValue::Null,
            OwnedDataValue::from(&json!({"a": 1})),
            OwnedDataValue::Null,
        );
        let root: Path = "metadata".parse().unwrap();
        assert_eq!(
            m.context_mut().remove(&root).unwrap_err().kind(),
            &ErrorKind::Config
        );
        let a: Path = "data.a".parse().unwrap();
        assert_eq!(
            m.context_mut().remove(&a).unwrap(),
            Some(OwnedDataValue::from(&json!(1)))
        );
        assert_eq!(m.context_mut().remove(&a).unwrap(), None);
    }

    #[test]
    fn parts_borrow_the_fields_separately() {
        let mut m = Message::new(OwnedDataValue::from(&json!(5)));
        let p = m.parts();
        p.audit.push(entry("t"));
        p.errors.push(ErrorRecord::new("C", "m"));
        *p.context = OwnedDataValue::from(&json!({"data": {}, "metadata": {}, "temp_data": {}}));
        assert_eq!(p.payload, &OwnedDataValue::from(&json!(5)));
        assert_eq!(p.id.len(), 36);
        assert_eq!((m.audit().len(), m.errors().len()), (1, 1));
    }
}
