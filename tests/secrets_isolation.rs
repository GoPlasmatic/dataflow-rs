//! The secrets guarantee, exit by exit.
//!
//! `tests/secrets.rs` covers what `{"secret": …}` resolves to and what the
//! static rules refuse. This file covers the other half of the contract: a
//! workflow that reads a secret from every place it is allowed to, run under
//! every capture policy the engine has, and then every surface a host could
//! observe — serialized message, `Debug`, audit trail, error list, every
//! `TraceOptions` shape, mapping contexts, the error-context mirror, observer
//! events, build errors, `check_workflow` output, and the process log — is
//! checked for the value.
//!
//! It also pins the access vectors that must *not* reach the store: `var`
//! paths that look like a root, `TaskContext::get`, the whole-context read,
//! and a message that carries its own `secret` / `secrets` keys.

use async_trait::async_trait;
use dataflow_rs::engine::functions::AsyncFunctionHandler;
use dataflow_rs::engine::message::Message;
use dataflow_rs::{
    AuditTrailScope, DataflowError, Engine, EngineBuilder, ExecutionTrace, Result, TaskContext,
    TaskOutcome, TemplateCompiler, TraceOptions, Workflow,
};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

mod common;

use common::{NESTED, RecordingObserver, SECRET, Sign, SignInput, dv, eval, secrets, workflow};

/// Fail if `text` carries either secret value.
fn assert_clean(label: &str, text: &str) {
    assert!(!text.contains(SECRET), "{label} leaked the secret: {text}");
    assert!(
        !text.contains(NESTED),
        "{label} leaked the nested secret: {text}"
    );
}

// -----------------------------------------------------------------------------
// A process-wide capturing logger — every `log` record the engine emits.
// -----------------------------------------------------------------------------

struct CaptureLogger;

static LOGGER: CaptureLogger = CaptureLogger;
static RECORDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

impl log::Log for CaptureLogger {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        RECORDS.lock().unwrap().push(format!(
            "{} {} {}",
            record.level(),
            record.target(),
            record.args()
        ));
    }
    fn flush(&self) {}
}

/// Install once per test binary; later calls are no-ops. Every test in this
/// file shares the sink, which only makes the final assertion stronger.
fn install_logger() {
    // `set_logger` fails if another test got there first — that is fine.
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Trace);
}

fn captured_logs() -> Vec<String> {
    RECORDS.lock().unwrap().clone()
}

// -----------------------------------------------------------------------------
// Handlers
// -----------------------------------------------------------------------------

/// Asserts, from inside a handler, that nothing but the operator and the
/// accessor reach the store. A failed assertion propagates out of
/// `process_message` like any other panic — the engine catches nothing.
struct Probe;

#[async_trait]
impl AsyncFunctionHandler for Probe {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _: &Value) -> Result<TaskOutcome> {
        let dl = Arc::clone(ctx.datalogic());
        let compile = |v: Value| dl.compile_arc(&v).unwrap();

        let context_keys: Vec<&str> = ctx
            .context()
            .as_object()
            .expect("context is an object")
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(
            context_keys,
            ["data", "metadata", "temp_data"],
            "the context has exactly the three recorded roots"
        );
        let whole = serde_json::to_string(&ctx.eval(&compile(json!({ "var": "" })))?).unwrap();
        assert_clean("whole-context read", &whole);

        assert!(ctx.get("secret").is_none());
        assert!(ctx.get("secrets").is_none());
        assert!(ctx.get("secrets.partner_key").is_none());
        assert_eq!(
            ctx.eval(&compile(json!({ "var": "secrets.partner_key" })))?,
            OwnedDataValue::Null
        );
        assert_eq!(
            ctx.eval(&compile(json!({ "var": "secret" })))?,
            OwnedDataValue::Null
        );

        assert_eq!(
            ctx.secret("partner_key").and_then(|v| v.as_str()),
            Some(SECRET)
        );
        assert_eq!(
            ctx.secret("partner.hmac").and_then(|v| v.as_str()),
            Some(NESTED)
        );
        assert!(ctx.secret("nope").is_none());
        assert!(ctx.secret("").is_none());
        assert_eq!(
            ctx.eval_json(&compile(json!({ "secret": "partner_key" })))?,
            json!(SECRET)
        );
        // The anchor for `assert_ran`: every assertion above is silent when it
        // passes, so without a mark the task could stop running — skipped by a
        // condition, dropped from the workflow — and every test in this file
        // would still pass while checking nothing.
        ctx.set("data.probe_ran", dv(json!(true)));
        Ok(TaskOutcome::Success)
    }
}

/// Reads the key and then fails with its own fixed text — the shape that
/// exercises the engine's error wrappers.
struct FailAfterReading;

#[async_trait]
impl AsyncFunctionHandler for FailAfterReading {
    type Input = SignInput;

    fn compile_input(input: &mut Self::Input, c: &TemplateCompiler) -> Result<()> {
        input.key.compile(c, "key")?;
        input.body.compile(c, "body")
    }

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &Self::Input) -> Result<TaskOutcome> {
        let _: String = input.key.eval_into(ctx)?;
        Err(DataflowError::Task("handler failed after reading".into()))
    }
}

// -----------------------------------------------------------------------------
// The workflow: a secret read from every permitted place, on both the
// shared-arena sync stretch and the post-async non-arena path.
// -----------------------------------------------------------------------------

fn everything_workflow() -> Workflow {
    workflow(json!({
        "id": "all", "name": "all", "priority": 0,
        "condition": { "==": [ { "secret": "partner_key" }, SECRET ] },
        "tasks": [
            { "id": "seed", "name": "seed", "function": { "name": "map", "input": {
                "mappings": [
                    { "path": "data.body", "logic": "payload-body" },
                    { "path": "data.dump", "logic": { "var": "" } }
                ] } } },
            { "id": "check", "name": "check",
              "condition": { "!!": { "secret": "partner.hmac" } },
              "function": { "name": "validation", "input": { "rules": [
                  { "logic": { "==": [ { "secret": "partner_key" }, SECRET ] },
                    "message": "key mismatch" } ] } } },
            { "id": "gate", "name": "gate", "function": { "name": "filter", "input": {
                "condition": { "!=": [ { "secret": "partner_key" }, "" ] } } } },
            { "id": "grouped", "name": "grouped",
              "condition": { "==": [ { "secret": "partner.hmac" }, NESTED ] },
              "tasks": [
                  { "id": "note", "name": "note", "function": { "name": "log", "input": {
                      "level": "info",
                      "message": { "cat": [ "body=", { "var": "data.body" } ] },
                      "fields": { "len": { "var": "data.sig" } } } } }
              ] },
            { "id": "sign", "name": "sign", "function": { "name": "sign", "input": {
                "key": { "secret": "partner_key" },
                "body": { "var": "data.body" } } } },
            { "id": "probe", "name": "probe", "function": { "name": "probe", "input": {} } },
            { "id": "after", "name": "after",
              "condition": { "==": [ { "secret": "partner_key" }, SECRET ] },
              "function": { "name": "map", "input": {
                "mappings": [ { "path": "data.done", "logic": { "var": "data.sig" } } ] } } }
        ]
    }))
}

/// A second workflow whose secret-reading rules *fail*, so the error list, the
/// error-context mirror and the error log all carry something.
fn failing_workflow() -> Workflow {
    workflow(json!({
        "id": "failing", "name": "failing", "priority": 1, "continue_on_error": true,
        "tasks": [
            { "id": "mismatch", "name": "mismatch", "continue_on_error": true,
              "function": { "name": "validation", "input": { "rules": [
                  { "logic": { "==": [ { "secret": "partner_key" }, "something-else" ] },
                    "message": "expected mismatch" },
                  { "logic": { "==": [ { "secret": { "var": "data.missing_key" } }, 1 ] },
                    "message": "dynamic name that does not exist" } ] } } },
            { "id": "boom", "name": "boom", "continue_on_error": true,
              "function": { "name": "fail_after_reading", "input": {
                "key": { "secret": "partner_key" }, "body": "x" } } }
        ]
    }))
}

fn builder() -> EngineBuilder {
    Engine::builder()
        .with_secrets_json(&secrets())
        .with_workflow(everything_workflow())
        .with_workflow(failing_workflow())
        .with_error_context_path("metadata.errors")
        .register("sign", Sign)
        .register("probe", Probe)
        .register("fail_after_reading", FailAfterReading)
}

fn fresh_message() -> Message {
    Message::builder()
        .data_json(&json!({ "missing_key": "nope-key" }))
        .build()
}

/// Every surface a host could observe after a run, checked for the value.
/// `run` names the run in a failure message.
fn assert_exits_clean(run: &str, message: &Message, trace: Option<&ExecutionTrace>) {
    let clean = |label: &str, text: &str| assert_clean(&format!("[{run}] {label}"), text);

    clean(
        "Serialize for Message",
        &serde_json::to_string(message).unwrap(),
    );
    clean("Debug for Message", &format!("{message:?}"));
    clean(
        "audit_trail",
        &serde_json::to_string(message.audit_trail()).unwrap(),
    );
    clean("errors", &serde_json::to_string(message.errors()).unwrap());
    clean("errors Debug", &format!("{:?}", message.errors()));
    clean("context", &serde_json::to_string(&message.context).unwrap());
    if let Some(trace) = trace {
        clean(
            "Serialize for ExecutionTrace",
            &serde_json::to_string(trace).unwrap(),
        );
        clean("Debug for ExecutionTrace", &format!("{trace:?}"));
        if let Some(last) = trace.final_message() {
            clean("final_message", &serde_json::to_string(last).unwrap());
        }
    }
}

/// The run did what it should — every allowed read resolved, the derived
/// value landed, the failing workflow failed the way it was told to. The
/// probe's own assertions ran on the way here, or the run would have panicked.
fn assert_ran(message: &Message) {
    assert_eq!(
        message.context["data"]["probe_ran"],
        dv(json!(true)),
        "the probe task ran, so its isolation assertions were exercised: {:?}",
        message.context
    );
    assert_eq!(
        message.context["data"]["done"],
        dv(json!("12:17")),
        "every task on the happy path ran: {:?}",
        message.context
    );
    let codes: Vec<&str> = message.errors().iter().map(|e| e.code.as_str()).collect();
    assert!(codes.contains(&"VALIDATION_ERROR"), "{codes:?}");
    assert!(codes.contains(&"EVALUATION_ERROR"), "{codes:?}");
    assert!(codes.contains(&"TASK_ERROR"), "{codes:?}");
    assert!(
        message.context["metadata"]["errors"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "the error-context mirror recorded the failures"
    );
}

// =============================================================================
// Every exit, under every capture policy
// =============================================================================

#[tokio::test]
async fn no_exit_carries_the_value_without_a_trace() {
    install_logger();
    let observer = Arc::new(RecordingObserver::default());
    let engine = builder()
        .with_observer(Arc::clone(&observer) as Arc<dyn dataflow_rs::ExecutionObserver>)
        .build()
        .unwrap();

    let mut message = fresh_message();
    engine.process_message(&mut message).await.unwrap();
    assert_ran(&message);

    assert_exits_clean("no trace", &message, None);
    assert_clean("observer events", &format!("{:?}", observer.seen()));
    assert!(!observer.seen().is_empty());
}

#[tokio::test]
async fn no_exit_carries_the_value_under_any_trace_policy() {
    install_logger();
    let engine = builder().build().unwrap();

    let policies: Vec<(&str, TraceOptions)> = vec![
        ("default", TraceOptions::default()),
        ("timings_only", TraceOptions::timings_only()),
        (
            "changes",
            TraceOptions {
                changes: true,
                ..Default::default()
            },
        ),
        (
            "audit trail: own",
            TraceOptions {
                snapshot_audit_trail: AuditTrailScope::Own,
                ..Default::default()
            },
        ),
        (
            "audit trail: none",
            TraceOptions {
                snapshot_audit_trail: AuditTrailScope::None,
                ..Default::default()
            },
        ),
        (
            "mapping contexts only",
            TraceOptions {
                snapshots: false,
                mapping_contexts: true,
                ..Default::default()
            },
        ),
        (
            "everything on, unbounded",
            TraceOptions {
                snapshots: true,
                mapping_contexts: true,
                changes: true,
                max_snapshot_bytes: 0,
                ..Default::default()
            },
        ),
    ];

    for (policy, options) in policies {
        let mut message = fresh_message();
        let trace = engine
            .process_message_with_trace_options(&mut message, options)
            .await
            .unwrap();
        assert_ran(&message);
        assert!(
            trace.executed_count() >= 7,
            "{policy}: {}",
            trace.executed_count()
        );
        assert_exits_clean(policy, &message, Some(&trace));
    }
}

#[tokio::test]
async fn the_default_trace_entry_points_and_the_channel_path_are_clean() {
    install_logger();
    let engine = builder().build().unwrap();

    let mut message = fresh_message();
    let trace = engine
        .process_message_with_trace(&mut message)
        .await
        .unwrap();
    assert_ran(&message);
    let serialized = serde_json::to_string(&trace).unwrap();
    assert!(
        serialized.contains("mapping_contexts"),
        "the default trace keeps mapping contexts"
    );
    assert_exits_clean("with_trace", &message, Some(&trace));

    let mut message = fresh_message();
    let trace = engine
        .process_message_for_channel_with_trace_options(
            "default",
            &mut message,
            TraceOptions::default(),
        )
        .await
        .unwrap();
    assert_ran(&message);
    assert_exits_clean("channel", &message, Some(&trace));

    // Caller-owned trace survives a hard failure; still clean.
    let mut message = fresh_message();
    let mut trace = ExecutionTrace::with_options(TraceOptions::default());
    engine
        .process_message_tracing(&mut message, &mut trace)
        .await
        .unwrap();
    assert_exits_clean("tracing", &message, Some(&trace));
}

#[tokio::test]
async fn a_message_that_round_trips_through_json_stays_clean() {
    install_logger();
    let engine = builder().build().unwrap();

    let mut message = fresh_message();
    engine.process_message(&mut message).await.unwrap();
    let json = serde_json::to_string(&message).unwrap();
    let back: Message = serde_json::from_str(&json).unwrap();
    assert_clean(
        "round-tripped message",
        &serde_json::to_string(&back).unwrap(),
    );
    assert_eq!(back.context["data"]["done"], dv(json!("12:17")));
}

#[tokio::test]
async fn the_process_log_never_carries_the_value() {
    install_logger();
    let engine = builder().build().unwrap();

    // Both a clean run and one that produces evaluation errors, at every
    // level, with the `log` task's target enabled.
    for _ in 0..3 {
        let mut message = fresh_message();
        let _ = engine
            .process_message_with_trace_options(&mut message, TraceOptions::default())
            .await
            .unwrap();
        assert_ran(&message);
    }

    let logs = captured_logs();
    assert!(
        logs.iter().any(|l| l.contains("dataflow::log")),
        "the log task must have emitted: {} records",
        logs.len()
    );
    assert!(
        logs.iter().any(|l| l.contains("nope-key")),
        "the dynamic-name failure must have been logged (naming the key): {logs:#?}"
    );
    for line in &logs {
        assert_clean("log record", line);
    }
}

// =============================================================================
// Access vectors that must not reach the store
// =============================================================================

#[test]
fn no_var_path_reaches_the_store() {
    let engine = builder().build().unwrap();
    let ctx = json!({ "data": {}, "metadata": {}, "temp_data": {} });

    for path in [
        "",
        "secret",
        "secrets",
        "secrets.partner_key",
        "secret.partner_key",
    ] {
        let got = eval(&engine, json!({ "var": path }), ctx.clone()).unwrap();
        assert_clean(&format!("var {path:?}"), &got.to_string());
    }
    // The whole-context read is exactly the three roots.
    let whole = eval(&engine, json!({ "var": "" }), ctx.clone()).unwrap();
    assert_eq!(whole, ctx);
    // And a `missing` probe agrees there is no such root.
    let missing = eval(&engine, json!({ "missing": ["secrets", "secret"] }), ctx).unwrap();
    assert_eq!(missing, json!(["secrets", "secret"]));
}

#[tokio::test]
async fn context_keys_named_secret_neither_shadow_nor_expose_the_store() {
    let wf = workflow(json!({
        "id": "w", "name": "w", "priority": 0,
        "tasks": [ { "id": "t", "name": "t",
          "condition": { "==": [ { "secret": "partner_key" }, SECRET ] },
          "function": { "name": "map", "input": {
            "mappings": [
                { "path": "data.from_store_matches", "logic": true },
                { "path": "data.from_context", "logic": { "var": "data.secret.partner_key" } },
                { "path": "data.from_metadata", "logic": { "var": "metadata.secrets.partner_key" } }
            ] } } } ]
    }));
    let engine = Engine::builder()
        .with_secrets_json(&secrets())
        .with_workflow(wf)
        .build()
        .unwrap();

    // A host that puts its own `secret` / `secrets` keys in the context gets
    // ordinary, recorded data — and the operator still reads the store.
    let mut message = Message::builder()
        .data_json(&json!({ "secret": { "partner_key": "decoy-in-data" } }))
        .metadata_json(&json!({ "secrets": { "partner_key": "decoy-in-metadata" } }))
        .build();
    engine.process_message(&mut message).await.unwrap();

    assert_eq!(
        message.context["data"]["from_store_matches"],
        dv(json!(true))
    );
    assert_eq!(
        message.context["data"]["from_context"],
        dv(json!("decoy-in-data"))
    );
    assert_eq!(
        message.context["data"]["from_metadata"],
        dv(json!("decoy-in-metadata"))
    );
    assert_clean(
        "message with decoy keys",
        &serde_json::to_string(&message).unwrap(),
    );

    // Same through a deserialized message carrying a top-level `secrets` root.
    let mut smuggled: Message = serde_json::from_value(json!({
        "id": "m", "payload": {},
        "context": { "data": {}, "metadata": {}, "temp_data": {},
                     "secrets": { "partner_key": "decoy-root" } },
        "audit_trail": [], "errors": []
    }))
    .unwrap();
    engine.process_message(&mut smuggled).await.unwrap();
    assert_eq!(
        smuggled.context["data"]["from_store_matches"],
        dv(json!(true))
    );
    assert_clean(
        "smuggled-root message",
        &serde_json::to_string(&smuggled).unwrap(),
    );
}

// =============================================================================
// Errors and diagnostics
// =============================================================================

#[test]
fn build_errors_and_issues_never_carry_a_value() {
    // A store that is not an object is refused without echoing it.
    for store in [
        json!(SECRET),
        json!([SECRET]),
        json!({ "k": SECRET }).to_string().into(),
    ] {
        let err = Engine::builder()
            .with_secrets_json(&store)
            .build()
            .err()
            .expect("non-object store must not build");
        assert_clean("non-object store error", &err.to_string());
        assert_clean("non-object store Debug", &format!("{err:?}"));
    }

    // Unknown-name and message-write refusals name keys and paths only.
    let leaky = workflow(json!({
        "id": "w", "name": "w", "priority": 0,
        "condition": { "secret": "nope" },
        "tasks": [ { "id": "t", "name": "t", "function": { "name": "map", "input": {
            "mappings": [ { "path": "data.x", "logic": { "secret": "partner_key" } } ] } } } ]
    }));
    let b = Engine::builder().with_secrets_json(&secrets());
    let issues = b.check_workflow(&leaky);
    assert_eq!(issues.len(), 2);
    assert_clean("issues Debug", &format!("{issues:?}"));
    for issue in &issues {
        assert_clean("issue Display", &issue.to_string());
    }
    let err = b.with_workflow(leaky).build().err().unwrap();
    assert_clean("build error", &err.to_string());
    assert!(err.to_string().contains("nope") || err.to_string().contains("mappings[0]"));

    // The store's own Debug masks values.
    let engine = builder().build().unwrap();
    let names: Vec<&str> = engine.declared_secrets().collect();
    assert_clean("declared_secrets", &format!("{names:?}"));
}

#[tokio::test]
async fn a_hard_failure_after_reading_a_secret_is_clean_on_both_error_channels() {
    install_logger();
    let wf = workflow(json!({
        "id": "strict", "name": "strict", "priority": 0, "continue_on_error": false,
        "tasks": [ { "id": "boom", "name": "boom", "function": {
            "name": "fail_after_reading",
            "input": { "key": { "secret": "partner_key" }, "body": "x" } } } ]
    }));
    let engine = Engine::builder()
        .with_secrets_json(&secrets())
        .with_workflow(wf)
        .register("fail_after_reading", FailAfterReading)
        .build()
        .unwrap();

    let mut message = Message::from_value(&json!({}));
    let mut trace = ExecutionTrace::with_options(TraceOptions::default());
    let err = engine
        .process_message_tracing(&mut message, &mut trace)
        .await
        .expect_err("continue_on_error: false must surface the failure");

    assert_clean("Result::Err Display", &err.to_string());
    assert_clean("Result::Err Debug", &format!("{err:?}"));
    assert_exits_clean("hard failure", &message, Some(&trace));
}

// =============================================================================
// Execution shapes that take different engine paths
// =============================================================================

#[tokio::test]
async fn a_looping_workflow_reading_a_secret_each_sweep_is_clean() {
    install_logger();
    let wf = workflow(json!({
        "id": "loop", "name": "loop", "priority": 0,
        "condition": { "==": [ { "secret": "partner_key" }, SECRET ] },
        "loop": { "counter": "i", "max": 4 },
        "tasks": [
            { "id": "step", "name": "step",
              "condition": { "!!": { "secret": "partner.hmac" } },
              "function": { "name": "map", "input": { "mappings": [
                  { "path": "data.sweeps", "logic": { "+": [ { "var": "data.sweeps" }, 1 ] } } ] } } }
        ]
    }));
    let engine = Engine::builder()
        .with_secrets_json(&secrets())
        .with_workflow(wf)
        .build()
        .unwrap();

    let mut message = Message::builder()
        .data_json(&json!({ "sweeps": 0 }))
        .build();
    let trace = engine
        .process_message_with_trace_options(&mut message, TraceOptions::default())
        .await
        .unwrap();

    assert_eq!(message.context["data"]["sweeps"], dv(json!(4)));
    assert_eq!(trace.executed_count(), 4);
    assert_exits_clean("loop", &message, Some(&trace));
}

#[tokio::test]
async fn concurrent_messages_on_one_engine_all_resolve_and_stay_clean() {
    let engine = Arc::new(builder().build().unwrap());

    let mut handles = Vec::new();
    for _ in 0..16 {
        let engine = Arc::clone(&engine);
        handles.push(tokio::spawn(async move {
            let mut message = fresh_message();
            let trace = engine
                .process_message_with_trace_options(&mut message, TraceOptions::default())
                .await
                .unwrap();
            assert_ran(&message);
            assert_exits_clean("concurrent", &message, Some(&trace));
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn a_hot_reload_keeps_the_guarantee_and_still_refuses_a_leak() {
    let engine = builder().build().unwrap();

    let reloaded = engine
        .with_new_workflows(vec![everything_workflow(), failing_workflow()])
        .unwrap();
    let mut message = fresh_message();
    let trace = reloaded
        .process_message_with_trace_options(&mut message, TraceOptions::default())
        .await
        .unwrap();
    assert_ran(&message);
    assert_exits_clean("reloaded", &message, Some(&trace));

    // The old engine is untouched.
    let mut message = fresh_message();
    engine.process_message(&mut message).await.unwrap();
    assert_ran(&message);

    // A reload cannot smuggle in what a build refuses.
    let leaky = workflow(json!({
        "id": "leak", "name": "leak", "priority": 0,
        "tasks": [ { "id": "t", "name": "t", "function": { "name": "map", "input": {
            "mappings": [ { "path": "data.x", "logic": { "secret": "partner_key" } } ] } } } ]
    }));
    let err = engine.with_new_workflows(vec![leaky]).err().unwrap();
    assert_clean("reload refusal", &err.to_string());
    assert!(err.to_string().contains("SECRET_IN_MESSAGE_WRITE"), "{err}");
}

#[tokio::test]
async fn a_message_processed_with_no_store_is_unchanged_in_shape() {
    // The operator exists on every engine but adds nothing to the context.
    let wf = workflow(json!({
        "id": "w", "name": "w", "priority": 0,
        "tasks": [ { "id": "t", "name": "t", "function": { "name": "map", "input": {
            "mappings": [ { "path": "data.dump", "logic": { "var": "" } } ] } } } ]
    }));
    let engine = Engine::builder().with_workflow(wf).build().unwrap();
    let mut message = Message::from_value(&json!({}));
    engine.process_message(&mut message).await.unwrap();

    let keys: Vec<&str> = message.context["data"]["dump"]
        .as_object()
        .expect("the dump is the whole context object")
        .iter()
        .map(|(k, _)| k.as_str())
        .collect();
    assert_eq!(keys, ["data", "metadata", "temp_data"]);
}
