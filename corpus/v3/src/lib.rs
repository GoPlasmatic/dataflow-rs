//! v3's corpus driver: runs a case on dataflow-rs 3.x and records what
//! happened, normalised. See `corpus/README.md` for the formats.
//!
//! A recording is `{definitions, build, steps}`:
//! - each definition's parse result, `validate_authored` issues and
//!   `check_workflow` issues;
//! - the build's result;
//! - per run step, the outcome, the message, the observer events and log lines
//!   when the case asks for them, and the trace of a second, traced run of the
//!   same input; per reload step, its result.

pub mod features;
pub mod logs;
pub mod observe;
pub mod operators;
pub mod script;

use dataflow_corpus::case::{AuditSpec, EngineSpec, MessageSpec, RunSpec, Step, TraceSpec};
use dataflow_corpus::compare::{IGNORED, diff};
use dataflow_corpus::{Case, Json, normalise};
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::{
    AuditMode, AuditTrailScope, DataflowError, Engine, EngineBuilder, ErrorInfo, ExecutionTrace,
    Message, TraceOptions, Workflow, WorkflowIssue,
};
use std::sync::Arc;

pub use features::Features;

/// Run `case` on v3 and return its recording, normalised.
pub async fn record_case(case: &Case) -> Json {
    let error_context = case.engine.error_context.as_ref().map(|e| e.path.as_str());
    let recorder = Arc::new(observe::Recorder::default());

    let mut definitions = Vec::new();
    let mut parsed = Vec::new();
    for raw in &case.workflows {
        let (record, workflow) = parse(raw);
        definitions.push(record);
        parsed.extend(workflow);
    }

    let builder = builder(&case.engine, &recorder);
    let mut workflows = parsed.iter();
    for record in &mut definitions {
        let checked = match record.get("parse") {
            Some(Json::String(ok)) if ok == "ok" => {
                let workflow = workflows
                    .next()
                    .expect("one workflow per parsed definition");
                issues(&builder.check_workflow(workflow))
            }
            _ => Json::Array(Vec::new()),
        };
        record.insert("checked", checked);
    }

    let mut out = Json::object();
    out.insert("definitions", Json::Array(definitions));
    let mut engine = match builder.with_workflows(parsed).build() {
        Ok(engine) => {
            out.insert("build", Json::str("ok"));
            engine
        }
        Err(e) => {
            out.insert("build", error(&e, false));
            out.insert("steps", Json::Array(Vec::new()));
            return out;
        }
    };

    let mut steps: Vec<Json> = Vec::new();
    for step in &case.steps {
        let record = match step {
            Step::Run(run) => {
                let record = run_step(&engine, run, &case.engine, &recorder, error_context).await;
                if let Some(n) = run.same_as {
                    same_as(&steps, n, &record);
                }
                record
            }
            Step::Reload(reload) => {
                let mut workflows = Vec::new();
                let mut failure = None;
                for raw in &reload.workflows {
                    match Workflow::from_json(&raw.to_text()) {
                        Ok(w) => workflows.push(w),
                        Err(e) => {
                            failure.get_or_insert(e);
                        }
                    }
                }
                let result = match failure {
                    Some(e) => error(&e, false),
                    None => match engine.with_new_workflows(workflows) {
                        Ok(next) => {
                            engine = next;
                            Json::str("ok")
                        }
                        Err(e) => error(&e, false),
                    },
                };
                Json::Object(vec![("reload".to_owned(), result)])
            }
        };
        steps.push(record);
    }
    out.insert("steps", Json::Array(steps));
    out
}

/// Parse one raw definition: its record (`parse`, `authored`) and, when it
/// parsed, the workflow.
fn parse(raw: &Json) -> (Json, Option<Workflow>) {
    let text = raw.to_text();
    let value: serde_json::Value = serde_json::from_str(&text).expect("a Json is valid JSON");
    let authored = issues(&Workflow::validate_authored(&value));
    let (result, workflow) = match Workflow::from_json(&text) {
        Ok(workflow) => (Json::str("ok"), Some(workflow)),
        Err(e) => (error(&e, false), None),
    };
    let record = Json::Object(vec![
        ("parse".to_owned(), result),
        ("authored".to_owned(), authored),
    ]);
    (record, workflow)
}

fn builder(spec: &EngineSpec, recorder: &Arc<observe::Recorder>) -> EngineBuilder {
    let mut builder = Engine::builder().register("script", script::Script);
    for name in &spec.operators {
        builder = match name.as_str() {
            "shout" => builder.with_datalogic_operator("shout", operators::Shout),
            other => panic!("unknown corpus operator {other}"),
        };
    }
    if let Some(secrets) = &spec.secrets {
        builder = builder.with_secrets(owned(secrets));
    }
    if let Some(context) = &spec.error_context {
        builder = builder.with_error_context_path(context.path.clone());
        if let Some(limit) = context.limit {
            builder = builder.with_error_context_limit(limit);
        }
    }
    if let Some(budget) = spec.ops_budget {
        builder = with_budget(builder, budget);
    }
    if spec.observer {
        builder = builder.with_observer(Arc::clone(recorder) as _);
    }
    builder
}

#[cfg(feature = "budget")]
fn with_budget(builder: EngineBuilder, budget: u64) -> EngineBuilder {
    builder.with_ops_budget(budget)
}

#[cfg(not(feature = "budget"))]
fn with_budget(_: EngineBuilder, _: u64) -> EngineBuilder {
    panic!("ops_budget needs the budget feature; the case must require it")
}

async fn run_step(
    engine: &Engine,
    run: &RunSpec,
    spec: &EngineSpec,
    recorder: &observe::Recorder,
    error_context: Option<&str>,
) -> Json {
    let channel = run.channel.as_deref();

    let mut message = build_message(&run.message, run);
    recorder.take();
    logs::begin(spec.logs.as_deref());
    let result = match channel {
        Some(c) => engine.process_message_for_channel(c, &mut message).await,
        None => engine.process_message(&mut message).await,
    };
    let lines = logs::end();
    let events = recorder.take();
    let message = normalised_message(&message, error_context);

    let mut record = Json::object();
    record.insert("outcome", outcome(&result));
    record.insert("message", message.clone());
    if spec.observer {
        record.insert("observer", Json::Array(events));
    }
    if spec.logs.is_some() {
        record.insert("logs", Json::Array(lines));
    }

    if run.trace.enabled {
        let mut traced = build_message(&run.message, run);
        let mut trace = ExecutionTrace::with_options(trace_options(&run.trace));
        logs::begin(spec.logs.as_deref());
        let result = match channel {
            Some(c) => {
                engine
                    .process_message_for_channel_tracing(c, &mut traced, &mut trace)
                    .await
            }
            None => {
                engine
                    .process_message_tracing(&mut traced, &mut trace)
                    .await
            }
        };
        logs::end();
        recorder.take();
        let mut steps = Json::from_text(
            &serde_json::to_string(&trace.steps).expect("a trace's steps serialize"),
        )
        .expect("serde_json's output parses");
        if let Json::Array(items) = &mut steps {
            for step in items {
                normalise::trace_step(step, error_context);
            }
        }
        let matches = normalised_message(&traced, error_context) == message;
        record.insert(
            "trace",
            Json::Object(vec![
                ("outcome".to_owned(), outcome(&result)),
                ("steps".to_owned(), steps),
                ("truncated".to_owned(), Json::Bool(trace.truncated())),
                ("message_matches".to_owned(), Json::Bool(matches)),
            ]),
        );
    }
    Json::Object(vec![("run".to_owned(), record)])
}

fn build_message(spec: &MessageSpec, run: &RunSpec) -> Message {
    let mut builder = Message::builder()
        .id(spec.id.clone())
        .payload(Arc::new(owned(&spec.payload)));
    if let Some(data) = &spec.data {
        builder = builder.data(owned(data));
    }
    if let Some(metadata) = &spec.metadata {
        builder = builder.metadata(owned(metadata));
    }
    if let Some(temp_data) = &spec.temp_data {
        builder = builder.temp_data(owned(temp_data));
    }
    if let Some(bucket) = run.routing_bucket {
        builder = builder.routing_bucket(bucket);
    }
    if let Some(on) = run.capture_changes {
        builder = builder.capture_changes(on);
    }
    if let Some(mode) = &run.audit_mode {
        builder = builder.audit_mode(match mode {
            AuditSpec::Full => AuditMode::Full,
            AuditSpec::Off => AuditMode::Off,
            AuditSpec::Last(n) => AuditMode::Last(*n),
        });
    }
    builder.build()
}

fn trace_options(spec: &TraceSpec) -> TraceOptions {
    TraceOptions {
        snapshots: spec.snapshots,
        mapping_contexts: spec.mapping_contexts,
        changes: spec.changes,
        max_snapshot_bytes: spec.max_snapshot_bytes,
        redact_paths: spec.redact_paths.clone(),
        snapshot_audit_trail: match spec.snapshot_audit_trail.as_str() {
            "full" => AuditTrailScope::Full,
            "own" => AuditTrailScope::Own,
            "none" => AuditTrailScope::None,
            other => panic!("unknown snapshot_audit_trail {other}"),
        },
    }
}

/// Parse a case's JSON into v3's value through text, so key order survives.
fn owned(json: &Json) -> OwnedDataValue {
    serde_json::from_str(&json.to_text()).expect("a Json is a valid OwnedDataValue")
}

fn normalised_message(message: &Message, error_context: Option<&str>) -> Json {
    let text = serde_json::to_string(message).expect("a message serializes");
    let mut json = Json::from_text(&text).expect("serde_json's output parses");
    normalise::message(&mut json, error_context);
    json
}

fn outcome(result: &dataflow_rs::Result<()>) -> Json {
    match result {
        Ok(()) => Json::str("ok"),
        Err(e) => error(e, true),
    }
}

/// `{"error": {code, message, retryable?}}`, the code from v3's one classifier.
fn error(e: &DataflowError, with_retryable: bool) -> Json {
    let code = ErrorInfo::new(None, None, e.clone()).code;
    let mut fields = vec![
        ("code".to_owned(), Json::str(code)),
        ("message".to_owned(), Json::str(e.to_string())),
    ];
    if with_retryable {
        fields.push(("retryable".to_owned(), Json::Bool(e.retryable())));
    }
    Json::Object(vec![("error".to_owned(), Json::Object(fields))])
}

fn issues(found: &[WorkflowIssue]) -> Json {
    let opt = |v: &Option<String>| v.as_ref().map_or(Json::Null, |s| Json::str(s.clone()));
    Json::Array(
        found
            .iter()
            .map(|issue| {
                Json::Object(vec![
                    ("code".to_owned(), Json::str(issue.code.as_str())),
                    ("severity".to_owned(), Json::str(issue.severity().as_str())),
                    ("path".to_owned(), opt(&issue.path)),
                    ("task_id".to_owned(), opt(&issue.task_id)),
                    ("message".to_owned(), Json::str(issue.message.clone())),
                ])
            })
            .collect(),
    )
}

/// `same_as: n`: this run's message must equal step `n`'s.
fn same_as(steps: &[Json], n: usize, record: &Json) {
    let earlier = steps
        .get(n)
        .and_then(|s| s.pointer("/run/message"))
        .unwrap_or_else(|| panic!("same_as: step {n} is not an earlier run"));
    let now = record
        .pointer("/run/message")
        .expect("a run records its message");
    let diffs = diff(earlier, now, IGNORED);
    assert!(
        diffs.is_empty(),
        "same_as {n}: the messages differ:\n{}",
        diffs
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}
