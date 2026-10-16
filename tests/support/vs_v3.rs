//! The run differential both `tests/run_vs_v3.rs` and `tests/run_loops.rs`
//! hold v4 to: one message run on v3's build and on v4's, compared by its
//! normalised wire text and the outcome's code.
//!
//! The text is normalised as the corpus normalises it: `processed_at`,
//! `engine_version` and the audit and error timestamps. One more difference
//! is removed, and it is break 4: v3 records a `WORKFLOW_ERROR` wrapper for a
//! workflow whose failure stops it or is stepped past (`continue_on_error` on
//! the workflow), and v4 records none. v3's `WORKFLOW_ERROR` records are
//! dropped before comparing.

use dataflow_corpus::json::Json;
use dataflow_corpus::normalise;
use dataflow_rs::run::{AuditMode, EngineBuilder, RunStatus};
use dataflow_rs::{Engine, Message, RunOptions};
use datavalue::OwnedDataValue;
use serde_json::Value;

/// One run: the message's seeds and the run's options.
#[derive(Clone, Default)]
pub struct Run {
    pub payload: Value,
    pub data: Value,
    pub metadata: Value,
    pub channel: Option<&'static str>,
    pub bucket: Option<u8>,
    pub audit: Option<usize>,
    /// `AuditMode::Off`, over `audit`.
    pub audit_off: bool,
    /// Change capture off.
    pub capture_off: bool,
}

/// A run over `data`.
pub fn data(data: Value) -> Run {
    Run {
        data,
        ..Run::default()
    }
}

/// Adds `name` (its id) to the workflow and to every task that lacks one,
/// setup steps included: v3 requires it on both (break 6), and a group needs
/// none.
pub fn named(mut workflow: Value) -> Value {
    fn name(node: &mut Value) {
        if node.get("name").is_none() {
            node["name"] = node["id"].clone();
        }
    }
    fn steps(list: Option<&mut Value>) {
        if let Some(Value::Array(tasks)) = list {
            for step in tasks {
                if step.get("function").is_some() {
                    name(step);
                } else {
                    steps(step.get_mut("tasks"));
                }
            }
        }
    }
    name(&mut workflow);
    steps(workflow.get_mut("tasks"));
    if let Some(r#loop) = workflow.get_mut("loop") {
        steps(r#loop.get_mut("setup"));
    }
    workflow
}

/// v3's build of `workflows` (named first), or why it refused them.
pub fn v3_engine(workflows: &[Value]) -> Result<dataflow_rs_v3::Engine, String> {
    v3_engine_with(workflows, |b| b)
}

/// [`v3_engine`] with `register` adding host functions to the builder:
/// the twins of what [`v4_engine_with`] registers on v4.
pub fn v3_engine_with(
    workflows: &[Value],
    register: impl FnOnce(dataflow_rs_v3::EngineBuilder) -> dataflow_rs_v3::EngineBuilder,
) -> Result<dataflow_rs_v3::Engine, String> {
    let workflows = workflows
        .iter()
        .map(|w| dataflow_rs_v3::Workflow::from_json(&named(w.clone()).to_string()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    register(dataflow_rs_v3::Engine::builder())
        .with_workflows(workflows)
        .build()
        .map_err(|e| e.to_string())
}

/// v4's build of `workflows` (named first, as v3's is), or the issues it
/// rejected them with.
pub fn v4_engine(workflows: &[Value]) -> Result<Engine, String> {
    v4_engine_with(workflows, |b| b)
}

/// [`v4_engine`] with `register` adding host functions to the builder:
/// the twins of what [`v3_engine_with`] registers on v3.
pub fn v4_engine_with(
    workflows: &[Value],
    register: impl FnOnce(EngineBuilder) -> EngineBuilder,
) -> Result<Engine, String> {
    workflows
        .iter()
        .fold(register(Engine::builder()), |b, w| {
            b.workflow_json(&named(w.clone()))
        })
        .build()
        .map_err(|issues| format!("{issues:?}"))
}

/// v3's build of `workflows`, run over `run`: the normalised wire text,
/// `WORKFLOW_ERROR` records dropped (break 4), and the outcome's code.
pub async fn on_v3(engine: &dataflow_rs_v3::Engine, run: &Run) -> (String, String) {
    let mut builder = dataflow_rs_v3::Message::builder()
        .id("m")
        .payload_json(&run.payload)
        .data_json(&run.data)
        .metadata_json(&run.metadata);
    if let Some(bucket) = run.bucket {
        builder = builder.routing_bucket(bucket);
    }
    if let Some(n) = run.audit {
        builder = builder.audit_mode(dataflow_rs_v3::AuditMode::Last(n));
    }
    if run.audit_off {
        builder = builder.audit_mode(dataflow_rs_v3::AuditMode::Off);
    }
    if run.capture_off {
        builder = builder.capture_changes(false);
    }
    let mut message = builder.build();
    let result = match run.channel {
        Some(channel) => {
            engine
                .process_message_for_channel(channel, &mut message)
                .await
        }
        None => engine.process_message(&mut message).await,
    };
    let code = match result {
        Ok(()) => "ok".to_owned(),
        Err(e) => dataflow_rs_v3::ErrorInfo::new(None, None, e).code,
    };
    (normalised(&message, true), code)
}

/// v4's run of `run`: the normalised wire text, the outcome's code, and the
/// run's report.
pub async fn on_v4(engine: &Engine, run: &Run) -> (String, String, dataflow_rs::Run) {
    let mut message = Message::from_parts(
        OwnedDataValue::from(&run.payload),
        OwnedDataValue::from(&run.data),
        OwnedDataValue::from(&run.metadata),
    )
    .with_id("m");
    let mut opts = RunOptions::default();
    if let Some(channel) = run.channel {
        opts = opts.channel(channel);
    }
    if let Some(bucket) = run.bucket {
        opts = opts.routing_bucket(bucket);
    }
    if let Some(n) = run.audit {
        opts = opts.audit(AuditMode::Last(n));
    }
    if run.audit_off {
        opts = opts.audit(AuditMode::Off);
    }
    if run.capture_off {
        opts = opts.capture_changes(false);
    }
    let report = engine.run_with(&mut message, &opts).await;
    let code = match report.status() {
        RunStatus::Stopped(e) => e.code().to_owned(),
        _ => "ok".to_owned(),
    };
    (normalised(&message, false), code, report)
}

/// The wire text, normalised; without `WORKFLOW_ERROR` records when `v3`.
pub fn normalised(message: &impl serde::Serialize, v3: bool) -> String {
    let text = serde_json::to_string(message).expect("a message serializes");
    let mut json = Json::from_text(&text).expect("a message parses");
    if v3 && let Some(Json::Array(errors)) = json.get_mut("errors") {
        errors.retain(|e| e.get("code") != Some(&Json::str("WORKFLOW_ERROR")));
    }
    normalise::message(&mut json, None);
    json.to_text()
}
