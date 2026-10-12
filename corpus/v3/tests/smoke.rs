//! The driver records a scripted write end to end.

use dataflow_corpus::{Case, Json};
use dataflow_corpus_v3::record_case;

#[tokio::test]
async fn a_scripted_write_is_recorded() {
    let case: Case = serde_json::from_str(
        r#"{
          "description": "smoke",
          "covers": [],
          "workflows": [{"id": "w", "name": "w", "tasks": [
            {"id": "t", "name": "t", "function": {"name": "script", "input": {
              "set": [{"path": "data.x", "value": {"+": [1, 2]}}]}}}]}],
          "steps": [{"run": {"message": {}}}]
        }"#,
    )
    .unwrap();
    let recording = record_case(&case).await;
    let run = recording.pointer("/steps/0/run").expect("one run");
    assert_eq!(run.pointer("/outcome"), Some(&Json::str("ok")));
    assert_eq!(
        run.pointer("/message/context/data/x"),
        Some(&Json::Number(3.into()))
    );
    assert_eq!(
        run.pointer("/message/audit_trail/0/status"),
        Some(&Json::Number(200.into()))
    );
    assert_eq!(
        run.pointer("/trace/message_matches"),
        Some(&Json::Bool(true))
    );
    assert_eq!(
        run.pointer("/message/context/metadata/processed_at"),
        Some(&Json::str("<timestamp>"))
    );
}
