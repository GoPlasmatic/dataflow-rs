extern crate dataflow_rs_v4 as dataflow_rs;

// The secrets guarantee, exit by exit: v3's `tests/secrets_isolation.rs`
// ported (I111).
//
// A workflow reads a secret from every place it is allowed to, runs under
// every trace policy v4 has, and then every surface a host could observe is
// checked for the value: the message's wire text and `Debug`, its audit,
// errors and context, the trace serialized, in `Debug` and replayed, the
// `Run`'s `Debug`, the observers' events, the error-context mirror, build
// and reload issues, the stop error on both channels, and the process log.
// v4's own surfaces are checked too: the `Debug` of the `Engine`, the
// `Registry`, the `ExprEngine`, the `Secrets`, a `Ctx` inside a call, a
// `Called` and every `Event`, and a `{"secret": …}` inside an `Interpolated`.
//
// It also pins the access vectors that must *not* reach the store: a `var`
// path that looks like a root, a `Path` naming one, the whole-context read,
// a message carrying its own `secret` / `secrets` keys, and one read from
// JSON with a fourth `secrets` root, which v4 refuses to read (break 22).
//
// Dropped with their v3 surfaces: mapping contexts, snapshot audit scopes and
// `final_message` (v4's trace is `initial` plus each step's changes; its
// replayed contexts stand in for `final_message`), `declared_secrets` (v4's
// `Secrets::names` and masked `Debug` stand in), the `process_message_*`
// entry points (v4 has `run` and `run_with`) and `TaskContext::with_view`.

use dataflow_rs::def::{ParseOptions, WorkflowDef, parse};
use dataflow_rs::expr::{ExprEngine, Interpolated, Output, Secrets};
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::function::testing;
use dataflow_rs::observe::{Event, Interest, Observer, StepResult, TraceOptions};
use dataflow_rs::program::{CompileOptions, IssueCode};
use dataflow_rs::run::{EngineConfig, ErrorContext, ErrorKind, RunStatus};
use dataflow_rs::{
    Ctx, Engine, Error, Expr, Function, Issue, Message, Outcome, Path, Registry, Result, Run,
    RunOptions,
};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

// -----------------------------------------------------------------------------
// Fixtures: v3's `tests/common` secrets, ported
// -----------------------------------------------------------------------------

/// Distinctive enough that a substring search is a real test.
const SECRET: &str = "s3cr3t-value-7f2a";
const NESTED: &str = "nested-hmac-91c0";

/// The store every test shares: one top-level key and one nested.
fn secrets() -> Value {
    json!({
        "partner_key": SECRET,
        "partner": { "hmac": NESTED }
    })
}

fn dv(v: Value) -> OwnedDataValue {
    OwnedDataValue::from(&v)
}

fn path(dotted: &str) -> Path {
    dotted.parse().unwrap()
}

/// An expression engine holding [`secrets()`].
fn exprs() -> ExprEngine {
    let store = Secrets::new(dv(secrets())).expect("the store is an object");
    ExprEngine::builder()
        .secrets(store)
        .build()
        .expect("the expression engine builds")
}

/// A definition from a `json!` literal.
fn def(json: &Value) -> WorkflowDef {
    let parsed = parse(json, ParseOptions::default());
    parsed
        .def
        .unwrap_or_else(|| panic!("the definition parses: {:?}", parsed.issues))
}

/// Fail if `text` carries either secret value.
fn assert_clean(label: &str, text: &str) {
    assert!(!text.contains(SECRET), "{label} leaked the secret: {text}");
    assert!(
        !text.contains(NESTED),
        "{label} leaked the nested secret: {text}"
    );
}

// -----------------------------------------------------------------------------
// A process-wide capturing logger: every `log` record the engine emits.
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
    // `set_logger` fails if another test got there first: that is fine.
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Trace);
}

fn captured_logs() -> Vec<String> {
    RECORDS.lock().unwrap().clone()
}

// -----------------------------------------------------------------------------
// Functions
// -----------------------------------------------------------------------------

/// `sign`'s config: the key and the body, both read through `Expr`.
#[derive(FunctionConfig)]
struct SignConfig {
    key: Expr<String>,
    body: Expr<String>,
    #[output(default = "data.sig")]
    out: Output,
}

/// The shape the store exists for: a function reads the key through an
/// `Expr` and through `Ctx::secret`, and writes only a derived value,
/// `data.sig = "<body length>:<key length>"`.
///
/// **Precondition:** the engine's store is [`secrets()`] and the task's `key`
/// resolves to [`SECRET`]: registering `Sign` against any other store panics
/// from inside the call rather than failing the caller's own assertion.
struct Sign;

impl Function for Sign {
    type Config = SignConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &SignConfig) -> Result<Outcome> {
        let key = ctx.eval(&cfg.key)?.into_owned();
        assert_eq!(key, SECRET, "Ctx::eval of an Expr<String>");
        assert_eq!(
            ctx.secret("partner_key").and_then(OwnedDataValue::as_str),
            Some(SECRET),
            "Ctx::secret"
        );
        let body = ctx.eval(&cfg.body)?.into_owned();
        let sig = format!("{}:{}", body.len(), key.len());
        ctx.emit(&cfg.out, OwnedDataValue::String(sig))?;
        Ok(Outcome::Ok)
    }
}

/// Asserts, from inside a call, that nothing but the operator and the
/// accessor reach the store. A failed assertion propagates out of `run_with`
/// like any other panic: the engine catches nothing.
struct Probe;

impl Function for Probe {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        let compile = |raw: Value| ctx.exprs().compile::<OwnedDataValue>(&raw);

        let whole = compile(json!({ "var": "" }))?;
        let whole = ctx.eval(&whole)?.into_owned();
        let context_keys: Vec<&str> = whole
            .as_object()
            .expect("the context is an object")
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(
            context_keys,
            ["data", "metadata", "temp_data"],
            "the context has exactly the three recorded roots"
        );
        assert_clean("whole-context read", &whole.to_json_string());

        // v3's `ctx.get("secret")`: in v4 no `Path` names anything but the
        // three roots, so the read cannot be written at all.
        for named in ["secret", "secrets", "secrets.partner_key"] {
            assert!(named.parse::<Path>().is_err(), "{named} is not a path");
        }
        let missing = compile(json!({ "var": "secrets.partner_key" }))?;
        assert_eq!(*ctx.eval(&missing)?, OwnedDataValue::Null);
        let missing = compile(json!({ "var": "secret" }))?;
        assert_eq!(*ctx.eval(&missing)?, OwnedDataValue::Null);

        assert_eq!(
            ctx.secret("partner_key").and_then(OwnedDataValue::as_str),
            Some(SECRET)
        );
        assert_eq!(
            ctx.secret("partner.hmac").and_then(OwnedDataValue::as_str),
            Some(NESTED)
        );
        assert!(ctx.secret("nope").is_none());
        assert!(ctx.secret("").is_none());
        let operator = compile(json!({ "secret": "partner_key" }))?;
        assert_eq!(*ctx.eval(&operator)?, dv(json!(SECRET)));

        // v4's own exit: a `Ctx` shows its facts, never the context.
        assert_clean("Debug for Ctx", &format!("{ctx:?}"));

        // The anchor for `assert_ran`: every assertion above is silent when it
        // passes, so without a mark the task could stop running (skipped by a
        // condition, dropped from the workflow) and every test in this file
        // would still pass while checking nothing.
        ctx.set(&path("data.probe_ran"), dv(json!(true)))?;
        Ok(Outcome::Ok)
    }
}

/// Reads the key and then fails with its own fixed text: the shape that
/// exercises the engine's error wrappers.
struct FailAfterReading;

impl Function for FailAfterReading {
    type Config = SignConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &SignConfig) -> Result<Outcome> {
        let _key = ctx.eval(&cfg.key)?;
        Err(Error::service("TASK_ERROR", "handler failed after reading"))
    }
}

/// `render`'s config: a document of `var` holes.
#[derive(FunctionConfig)]
struct RenderConfig {
    doc: Interpolated,
    #[output(default = "data.rendered")]
    out: Output,
}

/// Renders its document and writes it through its output.
struct Render;

impl Function for Render {
    type Config = RenderConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &RenderConfig) -> Result<Outcome> {
        let rendered = ctx.render(&cfg.doc).to_owned();
        ctx.emit(&cfg.out, rendered)?;
        Ok(Outcome::Ok)
    }
}

// -----------------------------------------------------------------------------
// Observers
// -----------------------------------------------------------------------------

/// What one `TaskFinished` carried, as v3's `RecordingObserver` kept it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SeenEvent {
    workflow_id: String,
    task_id: String,
    function: String,
    status: Option<u16>,
}

/// Asks for every event, timings included, and keeps each one's `Debug`
/// beside v3's task record. `Mutex` is fine for a test; a real observer must
/// not block.
#[derive(Default)]
struct Recording {
    events: Mutex<Vec<String>>,
    tasks: Mutex<Vec<SeenEvent>>,
}

impl Recording {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    fn tasks(&self) -> Vec<SeenEvent> {
        self.tasks.lock().unwrap().clone()
    }
}

impl Observer for Recording {
    fn interest(&self) -> Interest {
        Interest::ALL
    }

    fn on(&self, event: &Event<'_>) {
        if let Event::TaskFinished { info, status, .. } = event {
            self.tasks.lock().unwrap().push(SeenEvent {
                workflow_id: info.workflow_id.to_owned(),
                task_id: info.task_id.to_owned(),
                function: info.function.to_owned(),
                status: *status,
            });
        }
        self.events.lock().unwrap().push(format!("{event:?}"));
    }
}

/// Every event an observer saw, checked, and at least one task among them.
fn assert_observed_clean(label: &str, observer: &Recording) {
    let events = observer.events();
    assert!(
        events.iter().any(|e| e.starts_with("RunStarted"))
            && events.iter().any(|e| e.starts_with("RunFinished")),
        "{label}: {events:?}"
    );
    assert!(!observer.tasks().is_empty(), "{label}: no task events");
    for event in &events {
        assert_clean(&format!("{label}: Debug for Event"), event);
    }
    assert_clean(
        &format!("{label}: task events"),
        &format!("{:?}", observer.tasks()),
    );
}

// -----------------------------------------------------------------------------
// The workflow: a secret read from every permitted place, by built-ins and by
// host functions.
// -----------------------------------------------------------------------------

fn everything_workflow() -> Value {
    json!({
        "id": "all", "priority": 0,
        "condition": { "==": [ { "secret": "partner_key" }, SECRET ] },
        "tasks": [
            { "id": "seed", "function": { "name": "map", "input": {
                "mappings": [
                    { "path": "data.body", "logic": "payload-body" },
                    { "path": "data.dump", "logic": { "var": "" } }
                ] } } },
            { "id": "check",
              "condition": { "!!": { "secret": "partner.hmac" } },
              "function": { "name": "validation", "input": { "rules": [
                  { "logic": { "==": [ { "secret": "partner_key" }, SECRET ] },
                    "message": "key mismatch" } ] } } },
            { "id": "gate", "function": { "name": "filter", "input": {
                "condition": { "!=": [ { "secret": "partner_key" }, "" ] } } } },
            { "id": "grouped",
              "condition": { "==": [ { "secret": "partner.hmac" }, NESTED ] },
              "tasks": [
                  { "id": "note", "function": { "name": "log", "input": {
                      "level": "info",
                      "message": { "cat": [ "body=", { "var": "data.body" } ] },
                      "fields": { "len": { "var": "data.sig" } } } } }
              ] },
            { "id": "sign", "function": { "name": "sign", "input": {
                "key": { "secret": "partner_key" },
                "body": { "var": "data.body" } } } },
            { "id": "probe", "function": { "name": "probe", "input": {} } },
            { "id": "after",
              "condition": { "==": [ { "secret": "partner_key" }, SECRET ] },
              "function": { "name": "map", "input": {
                "mappings": [ { "path": "data.done", "logic": { "var": "data.sig" } } ] } } }
        ]
    })
}

/// A second workflow whose secret-reading rules *fail*, so the error list, the
/// error-context mirror and the error log all carry something.
fn failing_workflow() -> Value {
    json!({
        "id": "failing", "priority": 1, "on_failure": "continue",
        "tasks": [
            { "id": "mismatch", "on_failure": "continue",
              "function": { "name": "validation", "input": { "rules": [
                  { "logic": { "==": [ { "secret": "partner_key" }, "something-else" ] },
                    "message": "expected mismatch" },
                  { "logic": { "==": [ { "secret": { "var": "data.missing_key" } }, 1 ] },
                    "message": "dynamic name that does not exist" } ] } } },
            { "id": "boom", "on_failure": "continue",
              "function": { "name": "fail_after_reading", "input": {
                "key": { "secret": "partner_key" }, "body": "x" } } }
        ]
    })
}

/// The registry every engine here compiles against: the store and the three
/// host functions.
fn registry() -> Registry {
    let mut registry = Registry::new(exprs());
    registry.register("sign", Sign);
    registry.register("probe", Probe);
    registry.register("fail_after_reading", FailAfterReading);
    registry
}

/// The engine's config: the error-context mirror at `metadata.errors`.
fn config() -> EngineConfig {
    let mirror = ErrorContext::new(path("metadata.errors"), ErrorContext::DEFAULT_LIMIT)
        .expect("a slot inside metadata");
    EngineConfig::default().error_context(mirror)
}

fn engine_with(config: EngineConfig) -> Engine {
    Engine::builder()
        .registry(registry())
        .config(config)
        .workflow_json(&everything_workflow())
        .workflow_json(&failing_workflow())
        .build()
        .unwrap_or_else(|issues| panic!("the engine builds: {issues:?}"))
}

fn engine() -> Engine {
    engine_with(config())
}

fn fresh_message() -> Message {
    Message::from_parts(
        OwnedDataValue::Null,
        dv(json!({ "missing_key": "nope-key" })),
        OwnedDataValue::Null,
    )
}

/// The trace's task steps that ran and went on.
fn executed(run: &Run) -> usize {
    run.trace().map_or(0, |trace| {
        trace
            .steps
            .iter()
            .filter(|s| s.task.is_some() && s.result == StepResult::Executed)
            .count()
    })
}

/// Every surface a host could observe after a run, checked for the value.
/// `label` names the run in a failure message.
fn assert_exits_clean(label: &str, message: &Message, run: &Run) {
    let clean = |surface: &str, text: &str| assert_clean(&format!("[{label}] {surface}"), text);

    clean(
        "Serialize for Message",
        &serde_json::to_string(message).unwrap(),
    );
    clean("Debug for Message", &format!("{message:?}"));
    clean("audit", &serde_json::to_string(message.audit()).unwrap());
    clean("errors", &serde_json::to_string(message.errors()).unwrap());
    clean("errors Debug", &format!("{:?}", message.errors()));
    clean(
        "context",
        &serde_json::to_string(message.context()).unwrap(),
    );
    clean("Debug for Run", &format!("{run:?}"));
    if let RunStatus::Stopped(e) = run.status() {
        clean("stop error Display", &e.to_string());
        clean("stop error Debug", &format!("{e:?}"));
    }
    if let Some(trace) = run.trace() {
        clean(
            "Serialize for Trace",
            &serde_json::to_string(trace).unwrap(),
        );
        clean("Debug for Trace", &format!("{trace:?}"));
        for (i, context) in trace.contexts().enumerate() {
            clean(
                &format!("replayed context {i}"),
                &serde_json::to_string(&context).unwrap(),
            );
        }
    }
}

/// The run did what it should: every allowed read resolved, the derived
/// value landed, the failing workflow failed the way it was told to. The
/// probe's own assertions ran on the way here, or the run would have panicked.
fn assert_ran(message: &Message) {
    let data = Value::from(message.context().data());
    assert_eq!(
        data["probe_ran"],
        json!(true),
        "the probe task ran, so its isolation assertions were exercised: {data}"
    );
    assert_eq!(
        data["done"],
        json!("12:17"),
        "every task on the happy path ran: {data}"
    );
    let codes: Vec<&str> = message.errors().iter().map(|e| e.code.as_str()).collect();
    assert!(codes.contains(&"VALIDATION_ERROR"), "{codes:?}");
    assert!(codes.contains(&"EVALUATION_ERROR"), "{codes:?}");
    assert!(codes.contains(&"TASK_ERROR"), "{codes:?}");
    let metadata = Value::from(message.context().metadata());
    assert!(
        metadata["errors"].as_array().is_some_and(|a| !a.is_empty()),
        "the error-context mirror recorded the failures: {metadata}"
    );
}

// =============================================================================
// Every exit, under every trace policy
// =============================================================================

#[tokio::test]
async fn no_exit_carries_the_value_without_a_trace() {
    install_logger();
    let observer = Arc::new(Recording::default());
    let engine = engine_with(config().observer(Arc::clone(&observer) as Arc<dyn Observer>));

    let mut message = fresh_message();
    let run = engine.run(&mut message).await;
    assert!(
        matches!(run.status(), RunStatus::CompletedWithErrors),
        "{run:?}"
    );
    assert!(run.trace().is_none());
    assert_ran(&message);

    assert_exits_clean("no trace", &message, &run);
    assert_observed_clean("engine observer", &observer);
    let seen = observer.tasks();
    assert!(
        seen.iter()
            .any(|e| e.task_id == "sign" && e.function == "sign" && e.status == Some(200)),
        "{seen:?}"
    );
}

#[tokio::test]
async fn no_exit_carries_the_value_under_any_trace_policy() {
    install_logger();
    let engine = engine();

    let policies: Vec<(&str, TraceOptions)> = vec![
        ("default", TraceOptions::default()),
        ("timings", TraceOptions::default().timings(true)),
        ("no changes", TraceOptions::default().changes(false)),
        (
            "redacted",
            TraceOptions::default().redact(vec![path("data.dump"), path("data.sig")]),
        ),
        ("byte budget", TraceOptions::default().max_bytes(256)),
        (
            "everything on, unbounded",
            TraceOptions::default().timings(true).changes(true),
        ),
    ];

    for (policy, options) in policies {
        let observer = Recording::default();
        let mut message = fresh_message();
        let opts = RunOptions::default()
            .observer(&observer)
            .trace(options.clone());
        let run = engine.run_with(&mut message, &opts).await;
        assert_ran(&message);
        assert!(executed(&run) >= 7, "{policy}: {}", executed(&run));
        assert_exits_clean(policy, &message, &run);
        assert_observed_clean(policy, &observer);

        // Each policy did what it says, so no row checks a trace it never made.
        let trace = run.trace().expect("a traced run has a trace");
        let last = Value::from(trace.contexts().last().unwrap().as_value());
        if !options.changes {
            assert!(trace.steps.iter().all(|s| s.changes.is_empty()), "{policy}");
        }
        if !options.redact.is_empty() {
            assert_eq!(last["data"]["sig"], Value::Null, "{policy}");
            assert_eq!(last["data"]["done"], json!("12:17"), "{policy}");
        }
        if options.max_bytes.is_some() {
            assert!(trace.truncated, "{policy}");
        }
        if options.timings {
            assert!(trace.steps.iter().any(|s| s.elapsed.is_some()), "{policy}");
        }
    }
}

#[tokio::test]
async fn the_default_trace_entry_points_and_the_channel_path_are_clean() {
    // v3's three entry points are v4's `run` and `run_with`; its caller-owned
    // trace that survives a hard failure is `Run::trace` on every status,
    // pinned by the hard-failure test below.
    install_logger();
    let engine = engine();

    let mut message = fresh_message();
    let run = engine
        .run_with(
            &mut message,
            &RunOptions::default().trace(TraceOptions::default()),
        )
        .await;
    assert_ran(&message);
    assert!(run.trace().is_some_and(|t| !t.steps.is_empty()));
    assert_exits_clean("with trace", &message, &run);

    let mut message = fresh_message();
    let run = engine
        .run_with(
            &mut message,
            &RunOptions::default()
                .channel("default")
                .trace(TraceOptions::default()),
        )
        .await;
    assert_ran(&message);
    assert_eq!(
        Value::from(message.context().metadata())["channel"],
        json!("default")
    );
    assert_exits_clean("channel", &message, &run);
}

#[tokio::test]
async fn a_message_that_round_trips_through_json_stays_clean() {
    install_logger();
    let engine = engine();

    let mut message = fresh_message();
    engine.run(&mut message).await;
    let wire = serde_json::to_string(&message).unwrap();
    let back: Message = serde_json::from_str(&wire).unwrap();
    let again = serde_json::to_string(&back).unwrap();
    assert_clean("round-tripped message", &again);
    assert_eq!(again, wire);
    assert_eq!(Value::from(back.context().data())["done"], json!("12:17"));
}

#[tokio::test]
async fn the_process_log_never_carries_the_value() {
    install_logger();
    let engine = engine();

    // Both a clean run and one that produces evaluation errors, at every
    // level, with the `log` task's target enabled.
    for _ in 0..3 {
        let mut message = fresh_message();
        engine
            .run_with(
                &mut message,
                &RunOptions::default().trace(TraceOptions::default()),
            )
            .await;
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
    let engine = engine();
    let exprs = engine.program().registry().exprs();
    let ctx = json!({ "data": {}, "metadata": {}, "temp_data": {} });
    let eval = |raw: Value| -> Value {
        let expr: Expr = exprs.compile(&raw).unwrap();
        Value::from(&*exprs.eval(&expr, &dv(ctx.clone())).unwrap().value)
    };

    for path in [
        "",
        "secret",
        "secrets",
        "secrets.partner_key",
        "secret.partner_key",
    ] {
        let got = eval(json!({ "var": path }));
        assert_clean(&format!("var {path:?}"), &got.to_string());
        if !path.is_empty() {
            assert_eq!(got, Value::Null, "var {path:?}");
        }
    }
    // The whole-context read is exactly the three roots.
    assert_eq!(eval(json!({ "var": "" })), ctx);
    // And a `missing` probe agrees there is no such root.
    assert_eq!(
        eval(json!({ "missing": ["secrets", "secret"] })),
        json!(["secrets", "secret"])
    );
}

#[test]
fn no_path_can_name_the_store() {
    // v4's counterpart of v3's `ctx.get("secrets.partner_key")`: `Ctx::get`,
    // `Message::get` and every write take a `Path`, which starts at `data`,
    // `metadata` or `temp_data`, so neither a read nor a fourth root can
    // name the store.
    for named in [
        "secret",
        "secrets",
        "secret.partner_key",
        "secrets.partner_key",
        "secrets.partner.hmac",
    ] {
        let err = named
            .parse::<Path>()
            .expect_err("a path starts at a context root");
        assert_eq!(err.kind(), &ErrorKind::Config, "{named}");
        assert_clean("path refusal", &format!("{err} {err:?}"));
    }
}

#[tokio::test]
async fn context_keys_named_secret_neither_shadow_nor_expose_the_store() {
    let wf = json!({
        "id": "w", "priority": 0,
        "tasks": [ { "id": "t",
          "condition": { "==": [ { "secret": "partner_key" }, SECRET ] },
          "function": { "name": "map", "input": {
            "mappings": [
                { "path": "data.from_store_matches", "logic": true },
                { "path": "data.from_context", "logic": { "var": "data.secret.partner_key" } },
                { "path": "data.from_metadata", "logic": { "var": "metadata.secrets.partner_key" } }
            ] } } } ]
    });
    let engine = Engine::builder()
        .registry(Registry::new(exprs()))
        .workflow_json(&wf)
        .build()
        .unwrap();

    // A host that puts its own `secret` / `secrets` keys in the context gets
    // ordinary, recorded data, and the operator still reads the store.
    let mut message = Message::from_parts(
        OwnedDataValue::Null,
        dv(json!({ "secret": { "partner_key": "decoy-in-data" } })),
        dv(json!({ "secrets": { "partner_key": "decoy-in-metadata" } })),
    );
    let run = engine.run(&mut message).await;
    assert!(matches!(run.status(), RunStatus::Completed), "{run:?}");

    let data = Value::from(message.context().data());
    assert_eq!(data["from_store_matches"], json!(true));
    assert_eq!(data["from_context"], json!("decoy-in-data"));
    assert_eq!(data["from_metadata"], json!("decoy-in-metadata"));
    assert_clean(
        "message with decoy keys",
        &serde_json::to_string(&message).unwrap(),
    );

    // A message read from JSON with a fourth `secrets` root, which v3 read
    // and ran: v4 refuses the read (break 22), so the root never reaches a
    // run, and the refusal names the key, not what it holds.
    let err = serde_json::from_value::<Message>(json!({
        "id": "m", "payload": {},
        "context": { "data": {}, "metadata": {}, "temp_data": {},
                     "secrets": { "partner_key": "decoy-root" } },
        "audit_trail": [], "errors": []
    }))
    .expect_err("a context holds only its three roots");
    let shown = format!("{err} {err:?}");
    assert!(
        shown.contains("a context holds only `data`, `metadata` and `temp_data`, not `secrets`"),
        "{shown}"
    );
    assert!(!shown.contains("decoy-root"), "{shown}");
    assert_clean("smuggled-root refusal", &shown);

    // The decoy message itself reads back and runs as before.
    let wire = serde_json::to_string(&message).unwrap();
    let mut back: Message = serde_json::from_str(&wire).unwrap();
    let run = engine.run(&mut back).await;
    assert!(matches!(run.status(), RunStatus::Completed), "{run:?}");
    let data = Value::from(back.context().data());
    assert_eq!(data["from_store_matches"], json!(true));
    assert_eq!(data["from_context"], json!("decoy-in-data"));
    assert_clean(
        "read-back message with decoy keys",
        &serde_json::to_string(&back).unwrap(),
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
        let err = Secrets::new(dv(store)).expect_err("a non-object store is refused");
        assert_eq!(err.code(), "CONFIG_ERROR");
        assert_clean("non-object store error", &err.to_string());
        assert_clean("non-object store Debug", &format!("{err:?}"));
    }

    // Unknown-name and message-write refusals name keys and paths only.
    let leaky = json!({
        "id": "w", "priority": 0,
        "condition": { "secret": "nope" },
        "tasks": [ { "id": "t", "function": { "name": "map", "input": {
            "mappings": [ { "path": "data.x", "logic": { "secret": "partner_key" } } ] } } } ]
    });
    let compiler = registry().compiler(CompileOptions::default());
    let issues = compiler.check(&def(&leaky));
    let codes: Vec<IssueCode> = issues.iter().map(|i| i.code).collect();
    assert_eq!(
        codes,
        [IssueCode::UnknownSecret, IssueCode::SecretInMessageWrite]
    );
    assert_clean("issues Debug", &format!("{issues:?}"));
    for issue in &issues {
        assert_clean("issue Display", &issue.to_string());
    }
    let issues = Engine::builder()
        .registry(registry())
        .workflow_json(&leaky)
        .build()
        .expect_err("a leak does not build");
    let text = display(&issues);
    assert_clean("build error", &text);
    assert_clean("build error Debug", &format!("{issues:?}"));
    assert!(
        text.contains("nope") || text.contains("mappings[0]"),
        "{text}"
    );

    // The store's own `Debug` and names mask values.
    let exprs = exprs();
    let names: Vec<&str> = exprs.secrets().names().collect();
    // `serde_json` builds without `preserve_order`, so the store's keys are
    // sorted.
    assert_eq!(names, ["partner", "partner_key"]);
    assert_clean("Secrets::names", &format!("{names:?}"));
}

/// Every issue's `Display`, one per line.
fn display(issues: &[Issue]) -> String {
    issues
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn a_hard_failure_after_reading_a_secret_is_clean_on_both_error_channels() {
    install_logger();
    let wf = json!({
        "id": "strict", "priority": 0,
        "tasks": [ { "id": "boom", "function": {
            "name": "fail_after_reading",
            "input": { "key": { "secret": "partner_key" }, "body": "x" } } } ]
    });
    let engine = Engine::builder()
        .registry(registry())
        .config(config())
        .workflow_json(&wf)
        .build()
        .unwrap();

    let mut message = Message::from_json(&json!({}));
    let run = engine
        .run_with(
            &mut message,
            &RunOptions::default().trace(TraceOptions::default()),
        )
        .await;

    let RunStatus::Stopped(stop) = run.status() else {
        panic!("on_failure: fail must stop the run: {run:?}");
    };
    assert_eq!(stop.code(), "TASK_ERROR");
    assert!(
        run.trace()
            .is_some_and(|t| t.steps.iter().any(|s| s.result == StepResult::Failed)),
        "the trace keeps the failing step: {:?}",
        run.trace()
    );
    assert!(
        message.errors().iter().any(|e| e.code == "TASK_ERROR"),
        "{:?}",
        message.errors()
    );
    assert_exits_clean("hard failure", &message, &run);

    let err = run
        .into_result()
        .expect_err("a stopped run is an error on the result channel");
    assert_clean("Result::Err Display", &err.to_string());
    assert_clean("Result::Err Debug", &format!("{err:?}"));
}

// =============================================================================
// Execution shapes that take different engine paths
// =============================================================================

#[tokio::test]
async fn a_looping_workflow_reading_a_secret_each_sweep_is_clean() {
    install_logger();
    let wf = json!({
        "id": "loop", "priority": 0,
        "condition": { "==": [ { "secret": "partner_key" }, SECRET ] },
        "loop": { "counter": "i", "max": 4 },
        "tasks": [
            { "id": "step",
              "condition": { "!!": { "secret": "partner.hmac" } },
              "function": { "name": "map", "input": { "mappings": [
                  { "path": "data.sweeps", "logic": { "+": [ { "var": "data.sweeps" }, 1 ] } } ] } } }
        ]
    });
    let engine = Engine::builder()
        .registry(Registry::new(exprs()))
        .workflow_json(&wf)
        .build()
        .unwrap();

    let mut message = Message::from_parts(
        OwnedDataValue::Null,
        dv(json!({ "sweeps": 0 })),
        OwnedDataValue::Null,
    );
    let run = engine
        .run_with(
            &mut message,
            &RunOptions::default().trace(TraceOptions::default()),
        )
        .await;

    assert_eq!(Value::from(message.context().data())["sweeps"], json!(4));
    assert_eq!(executed(&run), 4);
    assert_exits_clean("loop", &message, &run);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_messages_on_one_engine_all_resolve_and_stay_clean() {
    let engine = engine();

    let mut handles = Vec::new();
    for _ in 0..16 {
        let engine = engine.clone();
        handles.push(tokio::spawn(async move {
            let mut message = fresh_message();
            let run = engine
                .run_with(
                    &mut message,
                    &RunOptions::default().trace(TraceOptions::default()),
                )
                .await;
            assert_ran(&message);
            assert_exits_clean("concurrent", &message, &run);
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn a_hot_reload_keeps_the_guarantee_and_still_refuses_a_leak() {
    let engine = engine();

    let compiler = engine.compiler();
    let workflows = [everything_workflow(), failing_workflow()].map(|json| {
        compiler
            .compile_one(&def(&json))
            .unwrap_or_else(|issues| panic!("the reload compiles: {issues:?}"))
            .workflow
    });
    let reloaded = engine.with_program(compiler.link(workflows));
    let mut message = fresh_message();
    let run = reloaded
        .run_with(
            &mut message,
            &RunOptions::default().trace(TraceOptions::default()),
        )
        .await;
    assert_ran(&message);
    assert_exits_clean("reloaded", &message, &run);

    // The old engine is untouched.
    let mut message = fresh_message();
    engine.run(&mut message).await;
    assert_ran(&message);

    // A reload cannot smuggle in what a build refuses.
    let leaky = json!({
        "id": "leak", "priority": 0,
        "tasks": [ { "id": "t", "function": { "name": "map", "input": {
            "mappings": [ { "path": "data.x", "logic": { "secret": "partner_key" } } ] } } } ]
    });
    let issues = compiler
        .compile_one(&def(&leaky))
        .expect_err("a leak does not compile");
    let text = display(&issues);
    assert_clean("reload refusal", &text);
    assert_clean("reload refusal Debug", &format!("{issues:?}"));
    assert!(text.contains("SECRET_IN_MESSAGE_WRITE"), "{text}");
}

#[tokio::test]
async fn a_message_processed_with_no_store_is_unchanged_in_shape() {
    // The operator exists on every engine but adds nothing to the context.
    let wf = json!({
        "id": "w", "priority": 0,
        "tasks": [ { "id": "t", "function": { "name": "map", "input": {
            "mappings": [ { "path": "data.dump", "logic": { "var": "" } } ] } } } ]
    });
    let engine = Engine::builder().workflow_json(&wf).build().unwrap();
    assert_eq!(
        engine
            .program()
            .registry()
            .exprs()
            .secrets()
            .names()
            .count(),
        0
    );
    let mut message = Message::from_json(&json!({}));
    engine.run(&mut message).await;

    let data = Value::from(message.context().data());
    let keys: Vec<&str> = data["dump"]
        .as_object()
        .expect("the dump is the whole context object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["data", "metadata", "temp_data"]);
}

// =============================================================================
// v4's own surfaces
// =============================================================================

#[test]
fn the_debug_of_everything_holding_the_store_is_clean() {
    let store = Secrets::new(dv(secrets())).unwrap();
    let masked = format!("{store:?}");
    assert_eq!(
        masked,
        r#"Secrets { partner: "******", partner_key: "******" }"#
    );

    let builder = ExprEngine::builder().secrets(store);
    assert_clean("Debug for ExprEngineBuilder", &format!("{builder:?}"));
    let exprs = builder.build().unwrap();
    assert_clean("Debug for ExprEngine", &format!("{exprs:?}"));

    let registry = registry();
    assert_clean("Debug for Registry", &format!("{registry:?}"));
    let compiler = registry.compiler(CompileOptions::default());
    assert_clean("Debug for Compiler", &format!("{compiler:?}"));

    let builder = Engine::builder()
        .registry(registry)
        .config(config())
        .workflow_json(&everything_workflow())
        .workflow_json(&failing_workflow());
    assert_clean("Debug for EngineBuilder", &format!("{builder:?}"));
    let engine = builder.build().unwrap();
    assert_clean("Debug for Engine", &format!("{engine:?}"));
    assert_clean("Debug for Program", &format!("{:?}", engine.program()));

    // An expression reading a secret shows its name, never the value, folded
    // or not.
    let secret: Expr<String> = exprs.compile(&json!({ "secret": "partner_key" })).unwrap();
    assert_clean("Debug for Expr", &format!("{secret:?}"));
}

#[tokio::test]
async fn a_function_called_alone_is_clean() {
    // `testing::call` runs one call through the real compile, `Ctx` and
    // overlay: its `Called` is a host's unit-test surface.
    let input = json!({ "key": { "secret": "partner_key" }, "body": "payload-body" });

    let called = testing::call(Sign, input.clone()).exprs(exprs()).await;
    assert!(called.issues.is_empty(), "{:?}", called.issues);
    assert_eq!(called.outcome.as_ref().unwrap(), &Outcome::Ok);
    assert_eq!(
        Value::from(called.message.context().data())["sig"],
        json!("12:17")
    );
    assert_clean("Debug for Called (sign)", &format!("{called:?}"));

    let called = testing::call(Probe, json!({})).exprs(exprs()).await;
    assert_eq!(called.outcome.as_ref().unwrap(), &Outcome::Ok);
    assert_clean("Debug for Called (probe)", &format!("{called:?}"));

    let called = testing::call(FailAfterReading, input).exprs(exprs()).await;
    let err = called.outcome.as_ref().expect_err("the call fails");
    assert_clean("Called's error", &format!("{err} {err:?}"));
    assert_clean("Debug for Called (failing)", &format!("{called:?}"));
}

#[tokio::test]
async fn a_secret_in_an_interpolated_document_is_refused_and_never_rendered() {
    // A document substitutes `var` holes only. A `{"secret": …}` node in one
    // is refused at compile (`SECRET_IN_DOCUMENT`), at the node and in a
    // hole's default, and the refusal names no value.
    let engine_for = |doc: Value| {
        Engine::builder()
            .registry(Registry::new(exprs()))
            .register("render", Render)
            .workflow_json(&json!({ "id": "w", "tasks": [
                { "id": "t", "function": { "name": "render", "input": { "doc": doc } } }
            ] }))
            .build()
    };
    for doc in [
        json!({ "t": { "secret": "partner_key" } }),
        json!({ "t": { "var": ["data.missing", { "secret": "partner.hmac" }] } }),
    ] {
        let issues = engine_for(doc.clone()).expect_err("a secret in a document is refused");
        assert!(
            issues.iter().any(|i| i.code == IssueCode::SecretInDocument),
            "{issues:?}"
        );
        assert_clean("document refusal", &display(&issues));
        assert_clean("document refusal Debug", &format!("{issues:?}"));
        let issues = exprs().interpolated(&doc).expect_err("refused alone too");
        assert_eq!(issues[0].code, IssueCode::SecretInDocument);
    }

    // What a document may hold that looks like the operator is data: an
    // escaped key and an object with a `secret` key beside another render as
    // written, and the store is never read.
    let doc = json!({
        "escaped": { "$secret": "partner_key" },
        "pair": { "secret": "partner_key", "note": { "var": "data.note" } }
    });
    let engine = engine_for(doc).expect("the literal document compiles");
    let mut message = Message::from_parts(
        OwnedDataValue::Null,
        dv(json!({ "note": "n" })),
        OwnedDataValue::Null,
    );
    let run = engine.run(&mut message).await;
    assert!(matches!(run.status(), RunStatus::Completed), "{run:?}");
    assert_eq!(
        Value::from(message.context().data())["rendered"],
        json!({
            "escaped": { "$secret": "partner_key" },
            "pair": { "secret": "partner_key", "note": "n" }
        })
    );
    assert_exits_clean("document", &message, &run);
}
