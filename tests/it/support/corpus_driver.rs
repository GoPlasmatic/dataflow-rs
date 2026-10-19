//! The corpus driver: what the engine does with a case, and the checks every
//! case's runs must pass whatever they pin.
//!
//! A case's definitions are parsed and compiled each on its own, through a
//! registry holding `script`, the cases' one host function, and an
//! expression engine with the case's secrets, custom operators and ops
//! budget. The compiled ones are linked into one engine with the case's
//! observer and error context. Each run step runs on a fresh message, plain
//! and then traced, and the two must agree; each reload step relinks its own
//! definitions through the same compiler.
//!
//! Every run step is held to the rules no case states: the traced run's
//! message equals the plain run's; the trace's last context is the message's
//! context, through `Trace::contexts` and through the corpus's own replay;
//! the message and the trace read back byte for byte; and a `same_as` step's
//! message equals the step it names. In a case with run steps, a definition
//! refused as `UNKNOWN_FUNCTION`, a run step over an engine that compiled
//! nothing and a reload that compiles nothing all fail the case: each would
//! silently test less than the case says. A case with no run step pins
//! refusals, `UNKNOWN_FUNCTION` among them.
//!
//! `tests/it/corpus.rs` holds what a case does to its golden file;
//! `tests/ts_types.rs` reads every message, `Run`, trace and issue it makes
//! through a [`Sink`].

use dataflow_corpus::case::{AuditSpec, EngineSpec, MessageSpec, Requires, RunSpec, Step};
use dataflow_corpus::{Case, Json, normalise, replay};
use dataflow_rs::datalogic_rs::bumpalo::Bump;
use dataflow_rs::datalogic_rs::operator::EvalContext;
use dataflow_rs::datalogic_rs::{self, ArenaExt, CustomOperator, DataValue};
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::expr::{CompileCx, ExprEngine, OperatorFamily, Secrets};
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::observe::{Event, Interest, Trace, TraceOptions, WorkflowExit};
use dataflow_rs::program::{CompileOptions, CompiledWorkflow, Compiler, IssueCode};
use dataflow_rs::run::{AuditMode, EngineConfig, ErrorContext, ErrorRecord};
use dataflow_rs::{
    Ctx, Engine, Error, Expr, Function, Issue, Message, Observer, Outcome, Path, Registry, Result,
    Run, RunOptions,
};
use datavalue::OwnedDataValue;
use serde_json::Value;
use std::borrow::Cow;
use std::sync::{Arc, Mutex};

/// Where a reader of the driver's runs takes what it wants of them.
pub(crate) trait Sink {
    /// One issue `parse` or the compiler reported, labelled by its case
    /// and definition.
    fn issue(&mut self, label: &str, issue: &Issue);
    /// One run: its message after the run, and its `Run`, which carries the
    /// trace when the run was traced.
    fn run(&mut self, label: &str, message: &Message, run: &Run);
}

/// A reader that takes nothing.
impl Sink for () {
    fn issue(&mut self, _: &str, _: &Issue) {}
    fn run(&mut self, _: &str, _: &Message, _: &Run) {}
}

/// How the driver runs a case.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Options {
    /// Whether the traced run reads the clock: the wire check wants every
    /// field written, a golden file wants nothing that varies.
    pub(crate) timings: bool,
}

/// Each cargo feature a case may require, by whether this build turns it on.
const FEATURES: &[(&str, bool)] = &[
    ("datetime", cfg!(feature = "datetime")),
    ("tensor", cfg!(feature = "tensor")),
    ("budget", cfg!(feature = "budget")),
];

/// Whether this build has every feature `requires` names on, on, and every
/// one it names off, off. Read from this crate's own features, which are
/// exactly the engine's in its own tests.
pub(crate) fn satisfied(requires: &Requires) -> bool {
    let on = |name: &str| FEATURES.iter().any(|(f, on)| *f == name && *on);
    requires.on.iter().all(|f| on(f)) && requires.off.iter().all(|f| !on(f))
}

/// What the engine does with `case`, named `name`, as its golden file holds
/// it: `{definitions, steps}`. Each definition is `{issues, compiled}`, every
/// issue `parse` and the compiler report as `CODE SEVERITY path`; each run
/// step `{run: {run, message, trace, observer?}}`, the `Run`, the message
/// normalised and the traced run's steps one line each; each reload step
/// `{reload: {definitions}}`.
///
/// Panics, naming the case, on a case that breaks a rule between its keys
/// (`Case::validate`) or one this file's doc lists. The caller has checked
/// the case's `requires`.
pub(crate) async fn run_case(name: &str, case: &Case, opts: Options, sink: &mut dyn Sink) -> Json {
    if let Err(why) = case.validate() {
        panic!("{name}: {why}");
    }
    let compiler = registry(name, &case.engine).compiler(CompileOptions::default());
    let has_runs = case.steps.iter().any(|s| matches!(s, Step::Run(_)));
    let (definitions, compiled) = definitions(name, &compiler, &case.workflows, has_runs, sink);
    assert!(
        !has_runs || !compiled.is_empty(),
        "{name}: the case has run steps, but no definition compiled, so they would run on \
         an empty engine; a refusal case carries no run step"
    );
    let recorder = Arc::new(Recorder::default());
    let mut engine = Engine::new(
        compiler.link(compiled),
        config(name, &case.engine, &recorder),
    );
    let mut steps = Vec::new();
    let mut messages: Vec<Option<Json>> = Vec::new();
    for (i, step) in case.steps.iter().enumerate() {
        let label = format!("{name} step {i}");
        match step {
            Step::Run(run) => {
                let (record, message) =
                    run_step(&label, &engine, run, &case.engine, &recorder, opts, sink).await;
                if let Some(n) = run.same_as {
                    assert!(
                        messages[n].as_ref() == Some(&message),
                        "{label}: same_as {n}, but the messages differ:\n{}\n{}",
                        messages[n]
                            .as_ref()
                            .map_or_else(String::new, Json::to_pretty),
                        message.to_pretty()
                    );
                }
                messages.push(Some(message));
                steps.push(Json::Object(vec![("run".to_owned(), record)]));
            }
            Step::Reload(reload) => {
                let (definitions, compiled) =
                    self::definitions(&label, &compiler, &reload.workflows, true, sink);
                assert!(
                    !compiled.is_empty(),
                    "{label}: the reload compiled no definition"
                );
                engine = engine.with_program(compiler.link(compiled));
                messages.push(None);
                let record = Json::Object(vec![("definitions".to_owned(), definitions)]);
                steps.push(Json::Object(vec![("reload".to_owned(), record)]));
            }
        }
    }
    Json::Object(vec![
        ("definitions".to_owned(), definitions),
        ("steps".to_owned(), Json::Array(steps)),
    ])
}

/// The case's registry: `script`, over an expression engine with the case's
/// operator families, secrets, custom operators and ops budget.
fn registry(name: &str, spec: &EngineSpec) -> Registry {
    let mut exprs = ExprEngine::builder();
    for family in &spec.families {
        exprs = match family.as_str() {
            "all-operators" => exprs.all_operators(),
            "datetime" => exprs.family(OperatorFamily::Datetime),
            "ext-string" => exprs.family(OperatorFamily::ExtString),
            "ext-array" => exprs.family(OperatorFamily::ExtArray),
            "ext-math" => exprs.family(OperatorFamily::ExtMath),
            "ext-control" => exprs.family(OperatorFamily::ExtControl),
            "ext-object" => exprs.family(OperatorFamily::ExtObject),
            "error-handling" => exprs.family(OperatorFamily::ErrorHandling),
            "tensor" => exprs.family(OperatorFamily::Tensor),
            other => panic!("{name}: no operator family is named {other}"),
        };
    }
    if let Some(secrets) = &spec.secrets {
        let secrets = Secrets::new(owned(secrets))
            .unwrap_or_else(|e| panic!("{name}: the case's secrets: {e}"));
        exprs = exprs.secrets(secrets);
    }
    for operator in &spec.operators {
        exprs = match operator.as_str() {
            "shout" => exprs.operator("shout", Shout),
            other => panic!("{name}: no custom operator is named {other}"),
        };
    }
    #[cfg(feature = "budget")]
    if let Some(ops) = spec.ops_budget {
        exprs = exprs.ops_budget(ops);
    }
    let exprs = exprs
        .build()
        .unwrap_or_else(|e| panic!("{name}: the case's expression engine: {e}"));
    let mut registry = Registry::new(exprs);
    registry.register("script", Script);
    registry
}

/// The engine's config from the case: the recording observer when the case
/// asks for one, and its error context, at the default limit when the case
/// names none.
fn config(name: &str, spec: &EngineSpec, recorder: &Arc<Recorder>) -> EngineConfig {
    let mut config = EngineConfig::default();
    if spec.observer {
        config = config.observer(Arc::clone(recorder) as Arc<dyn Observer>);
    }
    if let Some(context) = &spec.error_context {
        let path: Path = context
            .path
            .parse()
            .unwrap_or_else(|e| panic!("{name}: the error context's path: {e}"));
        let limit = context.limit.unwrap_or(ErrorContext::DEFAULT_LIMIT);
        let context = ErrorContext::new(path, limit)
            .unwrap_or_else(|e| panic!("{name}: the error context: {e}"));
        config = config.error_context(context);
    }
    config
}

/// Each of `workflows` parsed and compiled on its own: its record, and the
/// ones that compiled. When the definitions are `run`, one refused as
/// `UNKNOWN_FUNCTION` fails the case: the registry is the driver's, and a
/// missing function means the runs test less than the case says.
fn definitions(
    label: &str,
    compiler: &Compiler,
    workflows: &[Json],
    run: bool,
    sink: &mut dyn Sink,
) -> (Json, Vec<CompiledWorkflow>) {
    let mut records = Vec::new();
    let mut compiled = Vec::new();
    for (i, raw) in workflows.iter().enumerate() {
        let raw: Value = serde_json::from_str(&raw.to_text()).expect("a Json is valid JSON");
        let at = format!("{label} definition {i}");
        let parsed = parse(&raw, ParseOptions::default());
        let mut issues = parsed.issues;
        let workflow = parsed.def.and_then(|def| match compiler.compile_one(&def) {
            Ok(c) => {
                issues.extend(c.issues);
                Some(c.workflow)
            }
            Err(refused) => {
                issues.extend(refused);
                None
            }
        });
        for issue in &issues {
            assert!(
                !run || issue.code != IssueCode::UnknownFunction,
                "{at}: {issue}; the driver registers every function a case runs"
            );
            sink.issue(&at, issue);
        }
        let lines = issues
            .iter()
            .map(|i| {
                Json::str(format!(
                    "{} {} {}",
                    i.code.as_str(),
                    i.severity.as_str(),
                    i.path
                ))
            })
            .collect();
        records.push(Json::Object(vec![
            ("issues".to_owned(), Json::Array(lines)),
            ("compiled".to_owned(), Json::Bool(workflow.is_some())),
        ]));
        compiled.extend(workflow);
    }
    (Json::Array(records), compiled)
}

/// One run step, plain then traced: its record, and its normalised message.
async fn run_step(
    label: &str,
    engine: &Engine,
    spec: &RunSpec,
    engine_spec: &EngineSpec,
    recorder: &Recorder,
    opts: Options,
    sink: &mut dyn Sink,
) -> (Json, Json) {
    let error_context = engine_spec.error_context.as_ref().map(|e| e.path.as_str());
    let mut message = self::message(&spec.message);
    recorder.take();
    let run = engine.run_with(&mut message, &options(spec)).await;
    let events = recorder.take();
    read_back(label, &message);
    sink.run(&format!("{label} plain"), &message, &run);
    let plain = normalised(&message, error_context);

    let mut record = Json::object();
    let mut run_wire = wire(&run);
    normalise::run(&mut run_wire);
    record.insert("run", run_wire);
    record.insert("message", plain.clone());
    if spec.trace {
        let traced_opts = options(spec).trace(TraceOptions::default().timings(opts.timings));
        let mut traced = self::message(&spec.message);
        let traced_run = engine.run_with(&mut traced, &traced_opts).await;
        recorder.take();
        sink.run(&format!("{label} traced"), &traced, &traced_run);
        assert!(
            normalised(&traced, error_context) == plain,
            "{label}: the traced run's message differs from the plain run's"
        );
        let trace = traced_run
            .trace()
            .unwrap_or_else(|| panic!("{label}: a traced run returns its trace"));
        replays(label, trace, &traced);
        record.insert(
            "trace",
            Json::Array(trace.steps.iter().map(|s| Json::str(line(s))).collect()),
        );
    }
    if engine_spec.observer {
        record.insert("observer", Json::Array(events));
    }
    (record, plain)
}

/// A trace step as one line: `workflow task result status`, `-` for no
/// task or no status.
fn line(step: &dataflow_rs::observe::TraceStep) -> String {
    let result = serde_json::to_value(step.result).expect("a step result serializes");
    format!(
        "{} {} {} {}",
        step.workflow_id,
        step.task_id.as_deref().unwrap_or("-"),
        result.as_str().expect("a step result is a string"),
        step.status
            .map_or_else(|| "-".to_owned(), |s| s.to_string()),
    )
}

/// The trace's last context is the message's, through `Trace::contexts` and
/// through the corpus's replay of the trace's wire text, key order included;
/// and the trace reads back as it was written, replaying to the same
/// contexts.
fn replays(label: &str, trace: &Trace, message: &Message) {
    let context = text(message.context());
    let contexts: Vec<String> = trace.contexts().map(|c| text(&c)).collect();
    let last = contexts
        .last()
        .cloned()
        .unwrap_or_else(|| text(&trace.initial));
    assert_eq!(last, context, "{label}: Trace::contexts ends elsewhere");

    let wire = Json::from_text(&text(trace)).expect("a trace's text parses");
    let steps = wire
        .get("steps")
        .and_then(Json::as_array)
        .expect("a trace has steps");
    let rebuilt = replay::rebuild(wire.get("initial").expect("a trace has its initial"), steps);
    let rebuilt: Vec<String> = rebuilt
        .into_iter()
        .map(|c| {
            c.unwrap_or_else(|| panic!("{label}: a step without changes"))
                .to_text()
        })
        .collect();
    let ours: Vec<String> = contexts
        .iter()
        .map(|c| Json::from_text(c).expect("a context parses").to_text())
        .collect();
    assert_eq!(
        rebuilt, ours,
        "{label}: replay::rebuild and Trace::contexts differ"
    );

    let written = text(trace);
    let back: Trace = serde_json::from_str(&written)
        .unwrap_or_else(|e| panic!("{label}: the trace does not read back: {e}"));
    assert_eq!(
        text(&back),
        written,
        "{label}: the trace reads back as other text"
    );
    let again: Vec<String> = back.contexts().map(|c| text(&c)).collect();
    assert_eq!(
        again, contexts,
        "{label}: the trace read back replays elsewhere"
    );
}

/// The message reads back as the text it was written as.
fn read_back(label: &str, message: &Message) {
    let written = serde_json::to_string(message).expect("a message serializes");
    let back: Message = serde_json::from_str(&written)
        .unwrap_or_else(|e| panic!("{label}: the message does not read back: {e}"));
    assert_eq!(
        serde_json::to_string(&back).expect("a message serializes"),
        written,
        "{label}: the message reads back as other text"
    );
}

/// A wire value's text.
fn text<T: serde::Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).expect("a wire value serializes")
}

/// The step's run options: its channel, bucket, capture and audit mode.
fn options(spec: &RunSpec) -> RunOptions<'_> {
    let mut opts = RunOptions::default();
    if let Some(channel) = &spec.channel {
        opts = opts.channel(channel);
    }
    if let Some(bucket) = spec.routing_bucket {
        opts = opts.routing_bucket(bucket);
    }
    if let Some(on) = spec.capture_changes {
        opts = opts.capture_changes(on);
    }
    if let Some(mode) = &spec.audit_mode {
        opts = opts.audit(match mode {
            AuditSpec::Full => AuditMode::Full,
            AuditSpec::Off => AuditMode::Off,
            AuditSpec::Last(n) => AuditMode::Last(*n),
        });
    }
    opts
}

/// The step's message: its id, payload, data and metadata, and `temp_data`
/// written through the context. A seed that is not an object leaves its
/// root empty, as a message's reader would refuse it.
fn message(spec: &MessageSpec) -> Message {
    let seed = |json: &Option<Json>| json.as_ref().map_or(OwnedDataValue::Null, owned);
    let mut message =
        Message::from_parts(owned(&spec.payload), seed(&spec.data), seed(&spec.metadata))
            .with_id(spec.id.clone());
    if let Some(temp_data @ Json::Object(_)) = &spec.temp_data {
        let root: Path = "temp_data".parse().expect("a root is a path");
        message
            .context_mut()
            .set(&root, owned(temp_data))
            .expect("a root takes an object");
    }
    message
}

/// A case's JSON as the engine's value, through text, so key order survives.
fn owned(json: &Json) -> OwnedDataValue {
    serde_json::from_str(&json.to_text()).expect("a Json is a valid OwnedDataValue")
}

/// A wire value as the corpus's `Json`, key order kept.
fn wire<T: serde::Serialize>(value: &T) -> Json {
    Json::from_text(&serde_json::to_string(value).expect("a wire value serializes"))
        .expect("serde_json's output parses")
}

/// The message on the wire, normalised with the case's error-context path.
fn normalised(message: &Message, error_context: Option<&str>) -> Json {
    let mut json = wire(message);
    normalise::message(&mut json, error_context);
    json
}

// ---------------------------------------------------------------------------
// `script`, `shout` and the recording observer
// ---------------------------------------------------------------------------

/// `script`, the cases' one host function: everything it does is in its
/// config. In order, a call yields `yields` times, writes its identity to
/// `identity`, applies `set` through `Ctx::set`, records `errors` through
/// `Ctx::record_error`, and evaluates and returns `outcome`. An error
/// `outcome` is a service error carrying the variant's code and text.
pub(crate) struct Script;

/// `script`'s input. An unknown key is refused, as every derived config
/// refuses one.
#[derive(FunctionConfig)]
pub(crate) struct ScriptConfig {
    /// The writes, in order.
    #[config(default_json = "[]")]
    set: Vec<Write>,
    /// The errors to add, in order.
    #[config(default_json = "[]")]
    errors: Vec<AddError>,
    /// Where to write the call's identity: workflow, task, loop counter and
    /// element index.
    identity: Option<Expr<Path>>,
    /// How many times to yield before anything else.
    yields: Option<Expr<u64>>,
    /// The outcome, evaluated last: `"success"`, `"skip"`, `"halt"`,
    /// `{"status": n}` or `{"error": {...}}`. Absent or `null` is success.
    outcome: Option<Expr>,
}

/// One write to `path`: of `value` (evaluated), of what `get` reads, or of
/// `raw` (never evaluated). Exactly one of the three is set.
#[derive(FunctionConfig)]
#[config(check = one_of)]
struct Write {
    /// Where the value is written.
    path: Expr<Path>,
    /// An expression, evaluated.
    value: Option<Expr>,
    /// A path, read; `null` when absent.
    get: Option<Expr<Path>>,
    /// The config value itself.
    raw: Option<Value>,
}

/// Refuses a write naming none, or more than one, of `value`, `get` and
/// `raw`.
fn one_of(write: &Write, cx: &mut CompileCx<'_>) {
    let named = [
        write.value.is_some(),
        write.get.is_some(),
        write.raw.is_some(),
    ];
    if named.iter().filter(|n| **n).count() != 1 {
        cx.issue(
            IssueCode::InputParse,
            "a script write needs exactly one of value, get and raw",
        );
    }
}

/// One error the call adds to the message.
#[derive(FunctionConfig)]
struct AddError {
    /// The error's code.
    code: String,
    /// The error's message.
    message: String,
}

impl Function for Script {
    type Config = ScriptConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &ScriptConfig) -> Result<Outcome> {
        if let Some(yields) = &cfg.yields {
            for _ in 0..ctx.eval(yields)? {
                tokio::task::yield_now().await;
            }
        }
        if let Some(path) = &cfg.identity {
            let path = ctx.eval(path)?;
            let identity = identity(ctx);
            ctx.set(&path, identity)?;
        }
        for write in &cfg.set {
            let path = ctx.eval(&write.path)?;
            let value = match (&write.value, &write.get, &write.raw) {
                (Some(value), _, _) => ctx.eval(value)?.into_owned(),
                (None, Some(get), _) => {
                    let at = ctx.eval(get)?;
                    ctx.get(&at).map_or(OwnedDataValue::Null, Cow::into_owned)
                }
                (None, None, Some(raw)) => OwnedDataValue::from(raw),
                (None, None, None) => unreachable!("`one_of` refuses a write with no value"),
            };
            ctx.set(&path, value)?;
        }
        for error in &cfg.errors {
            ctx.record_error(ErrorRecord::new(&error.code, &error.message));
        }
        match &cfg.outcome {
            None => Ok(Outcome::Ok),
            Some(outcome) => {
                let value = Value::from(&*ctx.eval(outcome)?);
                scripted(&value)
            }
        }
    }
}

/// The call's identity, its keys sorted.
fn identity(ctx: &Ctx<'_>) -> OwnedDataValue {
    let info = ctx.info();
    OwnedDataValue::Object(vec![
        (
            "element_index".to_owned(),
            OwnedDataValue::from(info.element_index),
        ),
        (
            "loop_counter".to_owned(),
            OwnedDataValue::from(info.loop_counter),
        ),
        ("task_id".to_owned(), OwnedDataValue::from(info.task_id)),
        (
            "workflow_id".to_owned(),
            OwnedDataValue::from(info.workflow_id),
        ),
    ])
}

/// The evaluated `outcome`.
fn scripted(v: &Value) -> Result<Outcome> {
    match v {
        Value::Null => Ok(Outcome::Ok),
        Value::String(s) if s == "success" => Ok(Outcome::Ok),
        Value::String(s) if s == "skip" => Ok(Outcome::Skip),
        Value::String(s) if s == "halt" => Ok(Outcome::Halt),
        Value::Object(o) if o.len() == 1 && o.contains_key("status") => o["status"]
            .as_u64()
            .and_then(|s| u16::try_from(s).ok())
            .map(Outcome::Status)
            .ok_or_else(|| unknown(v)),
        Value::Object(o) if o.len() == 1 && o.contains_key("error") => Err(error(&o["error"])),
        _ => Err(unknown(v)),
    }
}

/// The error `e` names: a service error with the variant's code, its text
/// and its retryability, as 3.x's error variants wrote them.
fn error(e: &Value) -> Error {
    let message = e["message"].as_str().unwrap_or("scripted failure");
    let (code, text, retryable) = match e["variant"].as_str().unwrap_or("service") {
        "service" => (
            e["code"].as_str().unwrap_or_default().to_owned(),
            message.to_owned(),
            e["retryable"].as_bool().unwrap_or(false),
        ),
        "http" => {
            let status = e["status"]
                .as_u64()
                .and_then(|s| u16::try_from(s).ok())
                .unwrap_or(500);
            (
                "HTTP_ERROR".to_owned(),
                format!("HTTP error: {status} - {message}"),
                status >= 500 || matches!(status, 429 | 408 | 0),
            )
        }
        "timeout" => variant("TIMEOUT_ERROR", "Timeout error", message, true),
        "io" => variant("IO_ERROR", "IO error", message, true),
        "unknown" => variant("UNKNOWN_ERROR", "Unknown error", message, false),
        "task" => variant("TASK_ERROR", "Task error", message, false),
        "validation" => variant("VALIDATION_ERROR", "Validation error", message, false),
        "function" => variant("FUNCTION_ERROR", "Function execution error", message, false),
        "logic" => variant("LOGIC_ERROR", "Logic evaluation error", message, false),
        other => variant(
            "VALIDATION_ERROR",
            "Validation error",
            &format!("script: unknown error variant {other}"),
            false,
        ),
    };
    Error::service(code, text).with_retryable(retryable)
}

/// A variant's code, its text (`{prefix}: {message}`) and its retryability.
fn variant(code: &str, prefix: &str, message: &str, retryable: bool) -> (String, String, bool) {
    (code.to_owned(), format!("{prefix}: {message}"), retryable)
}

/// The error for an outcome `script` cannot read.
fn unknown(v: &Value) -> Error {
    let (code, text, _) = variant(
        "VALIDATION_ERROR",
        "Validation error",
        &format!("script: unknown scripted outcome: {v}"),
        false,
    );
    Error::service(code, text)
}

/// `shout`: uppercases its first argument.
struct Shout;

impl CustomOperator for Shout {
    fn evaluate<'a>(
        &self,
        args: &[&'a DataValue<'a>],
        _ctx: &mut EvalContext<'_, 'a>,
        arena: &'a Bump,
    ) -> datalogic_rs::Result<&'a DataValue<'a>> {
        let s = args.first().and_then(|v| v.as_str()).unwrap_or_default();
        Ok(arena.string(&s.to_uppercase()))
    }
}

/// The recording observer: each workflow and task event as a line, drained
/// per run step. No clock is read.
#[derive(Debug, Default)]
struct Recorder(Mutex<Vec<Json>>);

impl Recorder {
    /// The events since the last call, in the order they fired.
    fn take(&self) -> Vec<Json> {
        std::mem::take(&mut *self.0.lock().expect("the recorder lock"))
    }
}

impl Observer for Recorder {
    fn interest(&self) -> Interest {
        Interest::WORKFLOWS | Interest::TASKS
    }

    fn on(&self, event: &Event<'_>) {
        let line = match event {
            Event::RunStarted {
                message_id,
                considered,
                ..
            } => format!("run {message_id} considers {considered}"),
            Event::WorkflowStarted { id, .. } => format!("workflow {id} started"),
            Event::TaskFinished { info, status, .. } => format!(
                "task {}/{} ({}) finished {}",
                info.workflow_id,
                info.task_id,
                info.function,
                status.map_or_else(|| "-".to_owned(), |s| s.to_string())
            ),
            Event::WorkflowFinished { record, .. } => format!(
                "workflow {} finished, {} sweeps{}",
                record.id,
                record.sweeps,
                if record.exit == Some(WorkflowExit::Halted) {
                    ", halted"
                } else {
                    ""
                }
            ),
            Event::RunFinished {
                message_id, errors, ..
            } => format!("run {message_id} finished, {errors} errors"),
            _ => return,
        };
        self.0
            .lock()
            .expect("the recorder lock")
            .push(Json::str(line));
    }
}
