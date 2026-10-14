//! Phase 4's runs: what v4 does with a case's steps, shaped as v3's
//! recording so that `compare::diff` holds one against the other (I69).
//!
//! A case builds as v3's driver built it. Each definition is parsed and
//! compiled on its own, through the case's [`compiler`]. One v4 refuses (its
//! `parse` or its compile rejects it) is left out when v3's `parse` refused
//! it too, as v3's driver left that one out; the definitions test holds the
//! two refusals together, wherever v4 makes its own. Any other refusal leaves
//! the case with no engine, as a v3 build failure did. Each run step then runs on a fresh message,
//! with the step's channel, bucket, capture and audit mode, on an engine
//! holding the linked program and the default config.

use crate::{compiler, definitions, logs};
use dataflow_corpus::case::{AuditSpec, MessageSpec, Requires, RunSpec, Step};
use dataflow_corpus::{Case, Json, normalise};
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::def::{BUILTIN_FUNCTIONS, ParseOptions, StepDef, WorkflowDef, parse};
use dataflow_rs::expr::ExprEngine;
use dataflow_rs::program::{CompiledWorkflow, Compiler};
use dataflow_rs::run::{AuditMode, Engine, EngineConfig, Message, RunOptions, RunStatus};
use dataflow_rs::{Issue, Path};
use std::collections::BTreeSet;

/// What v4 makes of one definition.
enum Verdict {
    /// Parsed and compiled.
    Compiled(Box<(WorkflowDef, CompiledWorkflow)>),
    /// Refused, by `parse` or by the compiler: every Rejected issue, as
    /// `CODE at path`.
    Refused(Vec<String>),
}

/// Each of the case's definitions, parsed and compiled on its own, in case
/// order.
fn verdicts(compiler: &Compiler, case: &Case) -> Vec<Verdict> {
    let rejected = |issues: Vec<Issue>| {
        issues
            .into_iter()
            .filter(|i| i.severity.as_str() == "REJECTED")
            .map(|i| format!("{} at {}", i.code.as_str(), i.path))
            .collect()
    };
    definitions(case)
        .iter()
        .map(|raw| {
            let parsed = parse(raw, ParseOptions::default());
            match parsed.def {
                None => Verdict::Refused(rejected(parsed.issues)),
                Some(def) => match compiler.compile_one(&def) {
                    Ok(c) => Verdict::Compiled(Box::new((def, c.workflow))),
                    Err(issues) => Verdict::Refused(rejected(issues)),
                },
            }
        })
        .collect()
}

/// Why a case's steps wait for a later phase, read from the case; `None`
/// when phase 4 runs them. Only the definitions v4 compiles are read: on a
/// case that builds, those are the definitions left in.
///
/// - `"phase 5"`: a task calls a function that is not a built-in, or carries
///   `for_each`.
/// - `"phase 6"`: the engine has an observer or an error context, or a step
///   is a reload.
pub fn waits_for(case: &Case) -> Option<&'static str> {
    let host_or_fan_out = verdicts(&compiler(case), case).iter().any(|v| {
        let Verdict::Compiled(compiled) = v else {
            return false;
        };
        compiled.0.walk().any(|(_, _, step)| match step {
            StepDef::Task(task) => {
                task.for_each.is_some() || !BUILTIN_FUNCTIONS.contains(&task.function.name.as_str())
            }
            StepDef::Group(_) => false,
        })
    });
    let reloads = case.steps.iter().any(|s| matches!(s, Step::Reload(_)));
    if host_or_fan_out {
        Some("phase 5")
    } else if case.engine.observer || case.engine.error_context.is_some() || reloads {
        Some("phase 6")
    } else {
        None
    }
}

/// Whether v3's `parse` refused definition `i`, by its `recording`.
fn v3_refused(recording: &Json, i: usize) -> bool {
    let at = format!("/definitions/{i}/parse");
    !matches!(recording.pointer(&at), Some(Json::String(ok)) if ok == "ok")
}

/// Each operator family, and an operator only it adds.
const PROBES: &[(&str, &str)] = &[
    ("datetime", "datetime"),
    ("ext-string", "starts_with"),
    ("ext-array", "sort"),
    ("ext-math", "abs"),
    ("ext-control", "exists"),
    ("ext-object", "keys"),
    ("error-handling", "try"),
    ("tensor", "shape"),
];

/// What `all-operators` turns on: every family but `tensor`.
const ALL_OPERATORS: &[&str] = &[
    "datetime",
    "ext-string",
    "ext-array",
    "ext-math",
    "ext-control",
    "ext-object",
    "error-handling",
];

/// The features this build has on: each family whose probe operator v4's
/// expression engine evaluates, and `budget` when this crate's own feature
/// forwards it. Families are read from the engine, as v3's driver reads
/// them, since datalogic's features unify across the workspace.
fn features_on() -> BTreeSet<&'static str> {
    let exprs = ExprEngine::builder()
        .build()
        .expect("an empty expression engine builds");
    let names: BTreeSet<&str> = exprs.operators().iter().map(|op| &*op.name).collect();
    let mut on: BTreeSet<&'static str> = PROBES
        .iter()
        .filter(|(_, probe)| names.contains(probe))
        .map(|(family, _)| *family)
        .collect();
    if cfg!(feature = "budget") {
        on.insert("budget");
    }
    on
}

/// Whether this build has the features `requires` names on, and those it
/// names off, off.
pub fn satisfies(requires: &Requires) -> bool {
    let on = features_on();
    let expand = |names: &[String]| -> Vec<String> {
        names
            .iter()
            .flat_map(|name| match name.as_str() {
                "all-operators" => ALL_OPERATORS.iter().map(|f| (*f).to_owned()).collect(),
                other => vec![other.to_owned()],
            })
            .collect()
    };
    expand(&requires.on).iter().all(|f| on.contains(f.as_str()))
        && expand(&requires.off)
            .iter()
            .all(|f| !on.contains(f.as_str()))
}

/// What v4 does with a case, shaped as v3's recording: `{build, steps}`,
/// each run step `{run: {outcome, message, logs?}}`, normalised as the
/// recording is. `recording` is v3's, read only for which definitions v3's
/// `parse` refused.
///
/// `build` is `"ok"`, or, when v4 refuses a definition v3's `parse`
/// accepted, an error naming every Rejected issue of each, and then no step
/// runs. No step runs either when the case [`waits_for`] a later phase or
/// this build does not [`satisfies`] its `requires`: `steps` is then empty.
pub async fn run_case(case: &Case, recording: &Json) -> Json {
    let compiler = compiler(case);
    let mut compiled = Vec::new();
    let mut rejected = Vec::new();
    for (i, verdict) in verdicts(&compiler, case).into_iter().enumerate() {
        match verdict {
            Verdict::Compiled(c) => compiled.push(c.1),
            Verdict::Refused(_) if v3_refused(recording, i) => {}
            Verdict::Refused(issues) => rejected.extend(issues),
        }
    }
    let mut out = Json::object();
    if !rejected.is_empty() {
        let error = Json::Object(vec![
            ("code".to_owned(), Json::str("REJECTED")),
            ("message".to_owned(), Json::str(rejected.join("; "))),
        ]);
        out.insert("build", Json::Object(vec![("error".to_owned(), error)]));
        out.insert("steps", Json::Array(Vec::new()));
        return out;
    }
    out.insert("build", Json::str("ok"));
    let mut steps = Vec::new();
    if waits_for(case).is_none() && satisfies(&case.requires) {
        let engine = Engine::new(compiler.link(compiled), EngineConfig::default());
        for step in &case.steps {
            let Step::Run(run) = step else {
                unreachable!("a case with a reload waits for phase 6")
            };
            steps.push(run_step(&engine, run, case.engine.logs.as_deref()).await);
        }
    }
    out.insert("steps", Json::Array(steps));
    out
}

/// One run step: `{run: {outcome, message, logs?}}`.
async fn run_step(engine: &Engine, run: &RunSpec, log_level: Option<&str>) -> Json {
    let mut message = message(&run.message);
    let mut opts = RunOptions::default();
    if let Some(channel) = &run.channel {
        opts = opts.channel(channel);
    }
    if let Some(bucket) = run.routing_bucket {
        opts = opts.routing_bucket(bucket);
    }
    if let Some(on) = run.capture_changes {
        opts = opts.capture_changes(on);
    }
    if let Some(mode) = &run.audit_mode {
        opts = opts.audit(match mode {
            AuditSpec::Full => AuditMode::Full,
            AuditSpec::Off => AuditMode::Off,
            AuditSpec::Last(n) => AuditMode::Last(*n),
        });
    }
    logs::begin(log_level);
    let result = engine.run_with(&mut message, &opts).await;
    let lines = logs::end();

    let mut record = Json::object();
    record.insert("outcome", outcome(result.status()));
    record.insert("message", normalised(&message));
    if log_level.is_some() {
        record.insert("logs", Json::Array(lines));
    }
    Json::Object(vec![("run".to_owned(), record)])
}

/// The step's message: its id, payload, data and metadata, and `temp_data`
/// written through the context. A seed that is not an object is ignored, as
/// v3's builder ignores one.
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

/// A case's JSON as v4's value, through text, so key order survives.
fn owned(json: &Json) -> OwnedDataValue {
    serde_json::from_str(&json.to_text()).expect("a Json is a valid OwnedDataValue")
}

/// The message on the wire, in wire order, normalised as a recording is.
fn normalised(message: &Message) -> Json {
    let text = serde_json::to_string(message).expect("a message serializes");
    let mut json = Json::from_text(&text).expect("serde_json's output parses");
    normalise::message(&mut json, None);
    json
}

/// `"ok"` for a run that completed, with or without error records;
/// otherwise `{"error": {code, message, retryable}}`.
fn outcome(status: &RunStatus) -> Json {
    match status {
        RunStatus::Completed | RunStatus::CompletedWithErrors => Json::str("ok"),
        RunStatus::Stopped(e) => Json::Object(vec![(
            "error".to_owned(),
            Json::Object(vec![
                ("code".to_owned(), Json::str(e.code())),
                ("message".to_owned(), Json::str(e.message())),
                ("retryable".to_owned(), Json::Bool(e.is_retryable())),
            ]),
        )]),
        other => panic!("phase 4 has no deadline or cancellation, yet a run ended {other:?}"),
    }
}
