//! One built-in task, compiled alone and called once, and what it left
//! behind; and the golden files that hold what it leaves.
//!
//! The task is compiled as a one-task workflow by `Registry::default()` and
//! called through [`Builtin::call`] over a [`RunContext`], with no engine, so
//! this side stands in for the recorder: the status, the
//! `TASK_STATUS_ERROR` of a 5xx and the record of an `Err` come from the
//! engine's own [`decide`] and [`task_record`], so the two cannot drift.
//!
//! What is observed is serialized JSON text, so key order counts. The
//! engine's own writes, `metadata.processed_at`, `metadata.engine_version`
//! and `metadata.progress`, are dropped, and every error record's timestamp
//! is normalised.
//!
//! A [`Golden`] holds a test's observations to the ones checked in under
//! `tests/golden/lib/`, case by case and byte for byte, key order included;
//! `DATAFLOW_GOLDEN_WRITE=1` rewrites them. Under serde_json's
//! `preserve_order` a `json!` literal keeps its source order, where the
//! default build sorts it, so a seed and a golden call's input and payload
//! are given sorted keys: every build calls the task over the same trees.

use super::Builtin;
use crate::base::{Change, ErrorRecord};
use crate::def::{ParseOptions, parse};
use crate::function::{Registry, SyncCtx, task_record};
use crate::program::{Callee, CompileOptions, Compiled};
use crate::run::policy::decide;
use crate::state::{RunContext, Tuning};
use datavalue::OwnedDataValue;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// The run's clock reading; every timestamp observed is normalised to
/// [`TIMESTAMP`] first.
const NOW: &str = "2026-10-14T00:00:00+00:00";

/// What every error record's timestamp is observed as.
const TIMESTAMP: &str = "<timestamp>";

/// The engine's keys under `metadata`, dropped before observing.
const ENGINE_KEYS: [&str; 3] = ["processed_at", "engine_version", "progress"];

/// One task's call: the context after it (without the engine's keys), its
/// changes, the errors it added, and its status (`None` for a skip; the
/// `Err` record's code for an error).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Observed {
    /// The context after the task, as JSON text.
    pub(crate) context: String,
    /// The task's changes, as JSON text; `[]` for a skip or an `Err`.
    pub(crate) changes: String,
    /// The errors the call added, as JSON text.
    pub(crate) errors: String,
    /// The recorded status; `None` for a skip.
    pub(crate) status: Option<u16>,
    /// The code of the record an `Err` left; `None` when the call did not
    /// fail.
    pub(crate) error_code: Option<String>,
}

impl Observed {
    /// The observation as one golden value, its texts parsed in their order.
    pub(crate) fn golden(&self) -> OwnedDataValue {
        let parsed = |text: &str| OwnedDataValue::from_json(text).expect("observed JSON");
        OwnedDataValue::Object(vec![
            ("context".to_owned(), parsed(&self.context)),
            ("changes".to_owned(), parsed(&self.changes)),
            ("errors".to_owned(), parsed(&self.errors)),
            (
                "status".to_owned(),
                self.status
                    .map_or(OwnedDataValue::Null, |s| OwnedDataValue::from(&json!(s))),
            ),
            (
                "error_code".to_owned(),
                self.error_code
                    .as_ref()
                    .map_or(OwnedDataValue::Null, |c| OwnedDataValue::String(c.clone())),
            ),
        ])
    }
}

/// The one-task workflow every call compiles.
fn workflow(function: &str, input: &Value) -> Value {
    json!({"id": "w", "name": "w", "tasks": [{
        "id": "t", "name": "t", "continue_on_error": true,
        "function": {"name": function, "input": input}
    }]})
}

/// `{data, metadata, temp_data}` from `context`, a missing or non-object
/// root as `{}`, every object's keys sorted whatever the build.
pub(crate) fn seed(context: &Value) -> OwnedDataValue {
    let root = |key: &str| match context.get(key) {
        Some(v @ Value::Object(_)) => OwnedDataValue::from(&sorted(v)),
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

/// The text of `context` seeded and left as it is: what a call that writes
/// nothing leaves.
pub(crate) fn untouched(context: &Value) -> String {
    context_text(seed(context))
}

/// The context text after one call over `context` as it is, so a tree whose
/// keys are not sorted (a `serde_json::Value` sorts them unless serde_json's
/// `preserve_order` is on) can be used.
pub(crate) fn context_text_after(function: &str, input: &Value, context: OwnedDataValue) -> String {
    let task = task(function, input).unwrap_or_else(|| panic!("{function} compiles"));
    let mut slot = context;
    {
        let mut rc = RunContext::new(&mut slot, Tuning::default());
        let _ = task.call(
            &mut rc,
            &OwnedDataValue::Null,
            false,
            &mut Vec::new(),
            &mut Vec::new(),
        );
        rc.finish();
    }
    context_text(slot)
}

/// A task the compiler accepted.
pub(crate) struct Task {
    compiled: Compiled,
}

/// The compile of the one-task workflow, by `Registry::default()`; `None`
/// when the parse or the compile rejects it.
pub(crate) fn task(function: &str, input: &Value) -> Option<Task> {
    let def = parse(&workflow(function, input), ParseOptions::default()).def?;
    let compiled = Registry::default()
        .compiler(CompileOptions::default())
        .compile_one(&def)
        .ok()?;
    Some(Task { compiled })
}

impl Task {
    /// The task's built-in.
    fn builtin(&self) -> &Builtin {
        match &self.compiled.workflow.calls[0].callee {
            Callee::Builtin(builtin) => builtin,
            Callee::Host(_) => panic!("the task calls a built-in"),
        }
    }

    /// One call over `rc`, with the buffers the recorder reads, and the
    /// record the recorder adds. Its status, `None` for a skip, and the code
    /// of an `Err`.
    pub(crate) fn call<'p>(
        &'p self,
        rc: &mut RunContext<'_, 'p>,
        payload: &OwnedDataValue,
        capture: bool,
        changes: &mut Vec<Change>,
        errors: &mut Vec<ErrorRecord>,
    ) -> (Option<u16>, Option<String>) {
        let workflow = &self.compiled.workflow;
        let workflow_id: Arc<str> = Arc::from(workflow.id.as_str());
        let task_id: Arc<str> = Arc::from(workflow.tasks[0].id.as_str());
        let now: Arc<str> = Arc::from(NOW);
        let outcome = self.builtin().call(&mut SyncCtx {
            rc,
            exprs: workflow.registry.exprs(),
            payload,
            workflow_id: &workflow_id,
            task_id: &task_id,
            capture,
            traced: false,
            changes,
            marked: false,
            mapping_ends: None,
            errors: &mut *errors,
            now: &now,
        });
        let error_code = outcome.as_ref().err().map(|e| e.code().to_owned());
        let decision = decide(workflow.calls[0].policy, &task_id, outcome);
        if let Some((code, message)) = decision.record {
            errors.push(task_record(code, message, &workflow_id, &task_id, &now));
        }
        (decision.status, error_code)
    }

    /// Runs the task over `context` (and `payload`).
    pub(crate) fn run(&self, payload: &Value, context: &Value, capture: bool) -> Observed {
        let payload = OwnedDataValue::from(&sorted(payload));
        let mut slot = seed(context);
        let mut changes = Vec::new();
        let mut errors = Vec::new();
        let (status, error_code) = {
            let mut rc = RunContext::new(&mut slot, Tuning::default());
            let called = self.call(&mut rc, &payload, capture, &mut changes, &mut errors);
            rc.finish();
            called
        };
        // A skip and an `Err` record no changes: the recorder drops them.
        if status.is_none() || error_code.is_some() {
            changes.clear();
        }
        for error in &mut errors {
            if error.timestamp.is_some() {
                error.timestamp = Some(Arc::from(TIMESTAMP));
            }
            error.message = dataflow_corpus::normalise::dependency_text(&error.message);
        }
        Observed {
            context: context_text(slot),
            changes: serde_json::to_string(&changes).expect("changes serialize"),
            errors: serde_json::to_string(&errors).expect("errors serialize"),
            status,
            error_code,
        }
    }

    /// [`Task::run`] with capture on and off: asserts the two differ in
    /// nothing but the changes, which capture off never records, and
    /// returns the capture-on run.
    pub(crate) fn run_both(&self, payload: &Value, context: &Value) -> Observed {
        let on = self.run(payload, context, true);
        let off = self.run(payload, context, false);
        assert_eq!(
            (&off.context, &off.errors, off.status, &off.error_code),
            (&on.context, &on.errors, on.status, &on.error_code),
            "capture off over {context} (payload {payload})"
        );
        assert_eq!(off.changes, "[]", "capture off records no change");
        on
    }
}

/// One call of `function` with `input` over `context` (and `payload`), with
/// capture on or off.
pub(crate) fn run_once(
    function: &str,
    input: &Value,
    payload: &Value,
    context: &Value,
    capture: bool,
) -> Observed {
    task(function, input)
        .unwrap_or_else(|| panic!("{function} compiles with {input}"))
        .run(payload, context, capture)
}

/// [`Task::run_both`] of `function` with `input`, its keys sorted.
pub(crate) fn observe(function: &str, input: &Value, payload: &Value, context: &Value) -> Observed {
    task(function, &sorted(input))
        .unwrap_or_else(|| panic!("{function} compiles with {input}"))
        .run_both(payload, context)
}

/// `value` with every object's keys sorted, whatever serde_json's map type
/// is.
pub(crate) fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for key in keys {
                out.insert(key.clone(), sorted(&map[key]));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// `value`'s text, with every object's keys sorted.
fn sorted_text(value: &Value) -> String {
    sorted(value).to_string()
}

/// Are the goldens being rewritten?
fn writing() -> bool {
    std::env::var_os("DATAFLOW_GOLDEN_WRITE").is_some_and(|v| v == "1")
}

/// One golden file's path, from its name below `tests/golden/lib/` without
/// `.json`. Outside `src/`, so the goldens never ship in the published crate.
fn golden_path(file: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/lib")
        .join(format!("{file}.json"))
}

/// One golden file: each test's cases, in the order the test made them.
type Cases = BTreeMap<String, Vec<(String, OwnedDataValue)>>;

/// Reads a golden file; empty when it does not exist yet.
fn read_golden(file: &str) -> Cases {
    let Ok(text) = std::fs::read_to_string(golden_path(file)) else {
        return Cases::new();
    };
    let OwnedDataValue::Object(tests) = OwnedDataValue::from_json(&text).expect("a golden parses")
    else {
        panic!("{file}.json is an object of tests");
    };
    tests
        .into_iter()
        .map(|(test, cases)| {
            let OwnedDataValue::Array(cases) = cases else {
                panic!("{file}.json: {test} is a list of cases");
            };
            let cases = cases
                .into_iter()
                .map(|case| match case {
                    OwnedDataValue::Array(mut pair) if pair.len() == 2 => {
                        let value = pair.pop().expect("two");
                        let Some(OwnedDataValue::String(name)) = pair.pop() else {
                            panic!("{file}.json: {test}: a case is named");
                        };
                        (name, value)
                    }
                    other => panic!("{file}.json: {test}: a case is [name, value]: {other:?}"),
                })
                .collect();
            (test, cases)
        })
        .collect()
}

/// Writes a golden file: one line per case, so a change reviews as the
/// cases it moved.
fn write_golden(file: &str, tests: &Cases) {
    let mut text = String::from("{");
    for (i, (test, cases)) in tests.iter().enumerate() {
        text.push_str(if i == 0 { "\n" } else { ",\n" });
        text.push_str(&format!("  {}: [", Value::from(test.as_str())));
        for (j, (name, value)) in cases.iter().enumerate() {
            text.push_str(if j == 0 { "\n" } else { ",\n" });
            let value = serde_json::to_string(value).expect("a golden serializes");
            text.push_str(&format!("    [{}, {value}]", Value::from(name.as_str())));
        }
        text.push_str("\n  ]");
    }
    text.push_str("\n}\n");
    std::fs::create_dir_all(golden_path(file).parent().expect("a directory"))
        .expect("the golden directory");
    std::fs::write(golden_path(file), text).expect("the golden is written");
}

/// One writer at a time: tests sharing a file rewrite it in turn.
static WRITE: Mutex<()> = Mutex::new(());

/// One test's observations, held on drop to the test's cases in a golden
/// file, in order and byte for byte (key order included), or written there
/// under `DATAFLOW_GOLDEN_WRITE=1`.
pub(crate) struct Golden {
    file: String,
    test: &'static str,
    cases: Vec<(String, OwnedDataValue)>,
}

impl Golden {
    /// The cases of `test` in `tests/golden/lib/builtins/<file>.json`.
    pub(crate) fn new(file: &str, test: &'static str) -> Self {
        Self::at(&format!("builtins/{file}"), test)
    }

    /// The cases of `test` in `tests/golden/lib/<file>.json`.
    pub(crate) fn at(file: &str, test: &'static str) -> Self {
        Self {
            file: file.to_owned(),
            test,
            cases: Vec::new(),
        }
    }

    /// Adds a case: `name` and the value it is to hold.
    pub(crate) fn case(&mut self, name: String, value: OwnedDataValue) {
        self.cases.push((name, value));
    }

    /// [`observe`]s one call and adds it as a case named for its inputs; the
    /// capture-on run.
    pub(crate) fn observe(
        &mut self,
        function: &str,
        input: &Value,
        payload: &Value,
        context: &Value,
    ) -> Observed {
        let observed = observe(function, input, payload, context);
        let name = format!(
            "{function} {} payload {} over {}",
            sorted_text(input),
            sorted_text(payload),
            sorted_text(context)
        );
        self.case(name, observed.golden());
        observed
    }

    /// The text a case is compared as.
    fn text(value: &OwnedDataValue) -> String {
        serde_json::to_string(value).expect("a golden serializes")
    }
}

impl Drop for Golden {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let (file, test) = (self.file.as_str(), self.test);
        if writing() {
            let _alone = WRITE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut tests = read_golden(file);
            tests.insert(test.to_owned(), std::mem::take(&mut self.cases));
            write_golden(file, &tests);
            return;
        }
        let mut tests = read_golden(file);
        let want = tests.remove(test).unwrap_or_else(|| {
            panic!("{file}.json has no {test}: run with DATAFLOW_GOLDEN_WRITE=1")
        });
        for (i, ((got_name, got), (want_name, want))) in self.cases.iter().zip(&want).enumerate() {
            assert_eq!(got_name, want_name, "{file}.json: {test}: case {i}'s name");
            assert_eq!(
                Self::text(got),
                Self::text(want),
                "{file}.json: {test}: {got_name}"
            );
        }
        assert_eq!(
            self.cases.len(),
            want.len(),
            "{file}.json: {test}: the number of cases"
        );
    }
}

/// The `dataflow::log` lines a call emits.
pub(crate) mod logs {
    use super::{Observed, task};
    use log::{Level, LevelFilter, Log, Metadata, Record};
    use serde_json::Value;
    use std::sync::{Mutex, Once};

    /// The target the `log` built-in emits under.
    const TARGET: &str = "dataflow::log";

    /// What the built-in logs, on its own target, when an expression fails.
    const FAILED: &str = "Log: Failed to evaluate expression";

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
            let level = STATE.lock().expect("the log state").0;
            // The `dataflow::log` lines up to the level, and any `error`: the
            // built-in reports a failing expression on its own module's
            // target, which `log` below keeps only for that one line.
            level.is_some_and(|level| {
                if metadata.target() == TARGET {
                    metadata.level() <= level
                } else {
                    metadata.level() == Level::Error
                }
            })
        }

        fn log(&self, record: &Record<'_>) {
            if !self.enabled(record.metadata()) {
                return;
            }
            let line = if record.target() == TARGET {
                format!("{}: {}", record.level(), record.args())
            } else if record.args().to_string().starts_with(FAILED) {
                // The line is kept without the error's debug text: that one
                // fired is what counts.
                format!("{}: {FAILED}", record.level())
            } else {
                return;
            };
            STATE.lock().expect("the log state").1.push(line);
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

    /// One `log` task under a logger at `level`: what it observed and
    /// logged.
    pub(crate) fn logged(level: Level, input: &Value, context: &Value) -> (Observed, Vec<String>) {
        let task = task("log", input).expect("log compiles");
        capture(level, || task.run(&Value::Null, context, true))
    }
}
