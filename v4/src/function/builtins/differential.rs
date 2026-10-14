//! One built-in task run by v3 and by v4, and what each left behind.
//!
//! v3 runs the task as a one-task workflow through its engine; v4 calls the
//! compiled call site's [`Builtin::call`] over a [`RunContext`], with no
//! engine (Task 6 builds it), so this side stands in for the recorder: the
//! status, the `TASK_STATUS_ERROR` of a 5xx and the record of an `Err`, as
//! v3's `handle_task_result` makes them.
//!
//! What is compared is serialized JSON text, so key order counts. The
//! engine's own writes, `metadata.processed_at`, `metadata.engine_version`
//! and `metadata.progress`, are dropped on both sides, and every error
//! record's timestamp is normalised.

use super::Builtin;
use crate::base::{Change, ErrorRecord};
use crate::def::{ParseOptions, parse};
use crate::function::{Outcome, Registry, SyncCtx};
use crate::program::{Callee, CompileOptions, Compiled};
use crate::state::{RunContext, Tuning};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};

/// The 216 mappings `tests/map_vs_v3.rs` compiles, shared with it.
#[path = "../../../tests/support/map_grid.rs"]
mod map_grid;

pub(crate) use map_grid::mappings as mapping_grid;

/// The run's clock reading on the v4 side; every timestamp compared is
/// normalised to [`TIMESTAMP`] first.
const NOW: &str = "2026-10-14T00:00:00+00:00";

/// What every error record's timestamp is compared as.
const TIMESTAMP: &str = "<timestamp>";

/// The engine's keys under `metadata`, dropped before comparing.
const ENGINE_KEYS: [&str; 3] = ["processed_at", "engine_version", "progress"];

/// One task's run, by v3 or by v4: the context after it (without v3's stamps
/// and progress), its changes, the errors it added, and its status (`None`
/// for a skip; the `Err` record's code for an error).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Observed {
    /// The context after the task, as JSON text.
    pub(crate) context: String,
    /// The task's audit entry's changes, as JSON text; `[]` with no entry.
    pub(crate) changes: String,
    /// The message's errors, as JSON text.
    pub(crate) errors: String,
    /// The audit entry's status; `None` for a skip.
    pub(crate) status: Option<u16>,
    /// The code of the record an `Err` left; `None` when the call did not
    /// fail.
    pub(crate) error_code: Option<String>,
}

/// The one-task workflow both sides run.
fn workflow(function: &str, input: &Value) -> Value {
    json!({"id": "w", "name": "w", "tasks": [{
        "id": "t", "name": "t", "continue_on_error": true,
        "function": {"name": function, "input": input}
    }]})
}

/// `{data, metadata, temp_data}` from `context`, a missing or non-object
/// root as `{}`, as both messages seed it.
pub(crate) fn seed(context: &Value) -> OwnedDataValue {
    let root = |key: &str| match context.get(key) {
        Some(v @ Value::Object(_)) => OwnedDataValue::from(v),
        _ => OwnedDataValue::Object(Vec::new()),
    };
    OwnedDataValue::Object(vec![
        ("data".to_string(), root("data")),
        ("metadata".to_string(), root("metadata")),
        ("temp_data".to_string(), root("temp_data")),
    ])
}

/// The context as JSON text, without the engine's keys under `metadata`.
fn context_text(mut context: OwnedDataValue) -> String {
    if let OwnedDataValue::Object(roots) = &mut context
        && let Some((_, OwnedDataValue::Object(metadata))) =
            roots.iter_mut().find(|(k, _)| k == "metadata")
    {
        metadata.retain(|(k, _)| !ENGINE_KEYS.contains(&k.as_str()));
    }
    serde_json::to_string(&context).expect("a context serializes")
}

/// A task v3's build accepted, built once to run over many messages.
pub(crate) struct V3Task {
    engine: dataflow_rs_v3::Engine,
}

/// v3's build of the one-task workflow calling `function` with `input`;
/// `None` when v3 refuses it.
pub(crate) fn v3_task(function: &str, input: &Value) -> Option<V3Task> {
    let workflow =
        dataflow_rs_v3::Workflow::from_json(&workflow(function, input).to_string()).ok()?;
    let engine = dataflow_rs_v3::Engine::builder()
        .with_workflow(workflow)
        .build()
        .ok()?;
    Some(V3Task { engine })
}

impl V3Task {
    /// Runs the task over `context` (and `payload`).
    pub(crate) async fn run(&self, payload: &Value, context: &Value, capture: bool) -> Observed {
        let mut message = dataflow_rs_v3::Message::builder()
            .payload_json(payload)
            .capture_changes(capture)
            .build();
        message.context = seed(context);
        // The task continues on error, so a failure is in the message, not
        // the result.
        let _ = self.engine.process_message(&mut message).await;
        let entry = message.audit_trail().first();
        let changes = entry.map_or(&[][..], |entry| &entry.changes);
        let mut errors = message.errors().to_vec();
        for error in &mut errors {
            if error.timestamp.is_some() {
                error.timestamp = Some(TIMESTAMP.to_owned());
            }
        }
        let error_code = errors
            .iter()
            .find(|e| e.message.starts_with("Task t error: "))
            .map(|e| e.code.clone());
        Observed {
            context: context_text(message.context.clone()),
            changes: serde_json::to_string(changes).expect("changes serialize"),
            errors: serde_json::to_string(&errors).expect("errors serialize"),
            status: entry.map(|e| u16::try_from(e.status).expect("a status fits u16")),
            error_code,
        }
    }
}

/// Run `function` with `input` over `context` (and `payload`) on v3, as a
/// one-task workflow.
pub(crate) async fn v3(
    function: &str,
    input: &Value,
    payload: &Value,
    context: &Value,
    capture: bool,
) -> Observed {
    v3_task(function, input)
        .unwrap_or_else(|| panic!("v3 builds {function} with {input}"))
        .run(payload, context, capture)
        .await
}

/// A task v4's compiler accepted.
pub(crate) struct V4Task {
    compiled: Compiled,
}

/// v4's compile of the one-task workflow, by `Registry::default()`; `None`
/// when the parse or the compile rejects it.
pub(crate) fn v4_task(function: &str, input: &Value) -> Option<V4Task> {
    let def = parse(&workflow(function, input), ParseOptions::default()).def?;
    let compiled = Registry::default()
        .compiler(CompileOptions::default())
        .compile_one(&def)
        .ok()?;
    Some(V4Task { compiled })
}

impl V4Task {
    /// The task's built-in.
    fn builtin(&self) -> &Builtin {
        match &self.compiled.workflow.calls[0].callee {
            Callee::Builtin(builtin) => builtin,
            Callee::Host(_) => panic!("the task calls a built-in"),
        }
    }

    /// One call over `rc`, with the buffers the recorder reads.
    pub(crate) fn call<'p>(
        &'p self,
        rc: &mut RunContext<'_, 'p>,
        payload: &OwnedDataValue,
        capture: bool,
        changes: &mut Vec<Change>,
        errors: &mut Vec<ErrorRecord>,
    ) -> crate::Result<Outcome> {
        let workflow = &self.compiled.workflow;
        let mut cx = SyncCtx {
            rc,
            exprs: workflow.registry.exprs(),
            payload,
            workflow_id: &workflow.id,
            task_id: &workflow.tasks[0].id,
            capture,
            changes,
            errors,
            now: NOW,
        };
        let outcome = self.builtin().call(&mut cx);
        // The recorder's record, as v3's `handle_task_result` pushes it.
        let record = match &outcome {
            Ok(Outcome::Status(s)) if *s >= 500 => Some(cx.record(
                "TASK_STATUS_ERROR",
                format!("Task {} returned status {s}", cx.task_id),
            )),
            Err(e) => Some(cx.record(e.code(), format!("Task {} error: {e}", cx.task_id))),
            Ok(_) => None,
        };
        cx.errors.extend(record);
        outcome
    }

    /// Runs the task over `context` (and `payload`).
    pub(crate) fn run(&self, payload: &Value, context: &Value, capture: bool) -> Observed {
        let payload = OwnedDataValue::from(payload);
        let mut slot = seed(context);
        let mut changes = Vec::new();
        let mut errors = Vec::new();
        let outcome = {
            let mut rc = RunContext::new(&mut slot, Tuning::default());
            let outcome = self.call(&mut rc, &payload, capture, &mut changes, &mut errors);
            rc.finish();
            outcome
        };
        let (status, error_code) = match &outcome {
            Ok(Outcome::Ok) => (Some(200), None),
            Ok(Outcome::Status(s)) => (Some(*s), None),
            Ok(Outcome::Halt) => (Some(299), None),
            Ok(Outcome::Skip) => (None, None),
            Err(e) => (Some(500), Some(e.code().to_owned())),
        };
        // A skip and an `Err` record no changes, as v3's recorder drops them.
        if !matches!(
            outcome,
            Ok(Outcome::Ok | Outcome::Status(_) | Outcome::Halt)
        ) {
            changes.clear();
        }
        for error in &mut errors {
            if error.timestamp.is_some() {
                error.timestamp = Some(TIMESTAMP.to_owned());
            }
        }
        Observed {
            context: context_text(slot),
            changes: serde_json::to_string(&changes).expect("changes serialize"),
            errors: serde_json::to_string(&errors).expect("errors serialize"),
            status,
            error_code,
        }
    }
}

/// The same call on v4, through `Builtin::call` over a `RunContext`,
/// compiled by `Registry::default()`.
pub(crate) fn v4(
    function: &str,
    input: &Value,
    payload: &Value,
    context: &Value,
    capture: bool,
) -> Observed {
    v4_task(function, input)
        .unwrap_or_else(|| panic!("v4 compiles {function} with {input}"))
        .run(payload, context, capture)
}

/// Runs `function` with `input` on both sides, with and without change
/// capture, asserts each pair agrees, and returns v4's observation (with
/// capture on) for the checks that name what it must be.
pub(crate) async fn same(
    function: &str,
    input: &Value,
    payload: &Value,
    context: &Value,
) -> Observed {
    let mut observed = None;
    for capture in [true, false] {
        let mut expected = v3(function, input, payload, context, capture).await;
        let actual = v4(function, input, payload, context, capture);
        // v3's `parse_xml` and `publish_*` return their `Change` whether or not
        // capture is on (only `map` and `parse_json` honour it). v4 records
        // changes under capture only (I67), so with capture off the one
        // difference allowed is v3's changes, and v4's must be empty.
        if !capture && ["parse_xml", "publish_json", "publish_xml"].contains(&function) {
            expected.changes = "[]".to_owned();
        }
        assert_eq!(
            actual, expected,
            "{function} {input} over {context} (payload {payload}, capture {capture})"
        );
        observed.get_or_insert(actual);
    }
    observed.expect("two runs")
}

/// The `dataflow::log` lines a run emits, on v3 and on v4.
pub(crate) mod logs {
    use super::{Observed, V3Task, V4Task, v3_task, v4_task};
    use log::{Level, LevelFilter, Log, Metadata, Record};
    use serde_json::Value;
    use std::sync::{Mutex, Once};

    /// The target the `log` built-in emits under.
    const TARGET: &str = "dataflow::log";

    /// The level captured, and the lines so far. `None` enables nothing.
    static STATE: Mutex<(Option<Level>, Vec<String>)> = Mutex::new((None, Vec::new()));
    /// The one global logger, installed once.
    static INSTALL: Once = Once::new();
    /// One capture at a time: the logger is global.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    static CAPTURE: Capture = Capture;

    struct Capture;

    impl Log for Capture {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            metadata.target() == TARGET
                && STATE
                    .lock()
                    .expect("the log state")
                    .0
                    .is_some_and(|level| metadata.level() <= level)
        }

        fn log(&self, record: &Record<'_>) {
            if self.enabled(record.metadata()) {
                STATE.lock().expect("the log state").1.push(format!(
                    "{}: {}",
                    record.level(),
                    record.args()
                ));
            }
        }

        fn flush(&self) {}
    }

    /// Runs `run` with `dataflow::log` captured up to `level`; the lines it
    /// emitted, as `LEVEL: text`.
    fn capture<R>(level: Level, run: impl FnOnce() -> R) -> (R, Vec<String>) {
        let _alone = ONE_AT_A_TIME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        INSTALL.call_once(|| {
            log::set_logger(&CAPTURE).expect("no other logger is installed");
            log::set_max_level(LevelFilter::Trace);
        });
        {
            let mut state = STATE.lock().expect("the log state");
            *state = (Some(level), Vec::new());
        }
        let result = run();
        let mut state = STATE.lock().expect("the log state");
        state.0 = None;
        (result, std::mem::take(&mut state.1))
    }

    /// One `log` task on v3 and on v4 under a logger at `level`: what each
    /// observed and logged.
    pub(crate) fn both(
        level: Level,
        input: &Value,
        context: &Value,
    ) -> ((Observed, Vec<String>), (Observed, Vec<String>)) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime");
        let v3: V3Task = v3_task("log", input).expect("v3 builds log");
        let v4: V4Task = v4_task("log", input).expect("v4 compiles log");
        let on_v3 = capture(level, || {
            runtime.block_on(v3.run(&Value::Null, context, true))
        });
        let on_v4 = capture(level, || v4.run(&Value::Null, context, true));
        (on_v3, on_v4)
    }
}
