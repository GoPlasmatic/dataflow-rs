// The error context end to end (I108): after every recorded call that is not
// a `Skip`, each error record the call added, the recorder's own and the
// call's, is mirrored at the configured path as `{workflow_id, task_id,
// code, status}`, appended and trimmed to the newest `limit`; a non-array at
// the path, or a non-object above it, is replaced. Every rule v3 can run is
// held to v3 through `support/vs_v3.rs`, with v3's `with_error_context_path`
// and `with_error_context_limit` on its builder, comparing the normalised
// wire text. One rule is v4's own and is checked literally: the path is
// reset at run start (break 20), where v3 appends to what the message
// brought.
//
// One host function, `act`, has a twin on both sides. It evaluates its
// `outcome` and runs it as a step or a list of steps: `"record"` records a
// `HOST_WARNING` and goes on; `"ok"`, `"skip"`, `"halt"`, a status, or
// `"err"` (a `PARTNER_DOWN` service error) is the result. In a fan-out its
// `outcome` reads the element.

#[path = "support/vs_v3.rs"]
mod vs_v3;

use dataflow_corpus::json::Json;
use dataflow_corpus::{path as json_path, replay};
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::observe::{StepResult, Trace, TraceOptions};
use dataflow_rs::run::{EngineConfig, ErrorContext, ErrorKind, ErrorRecord};
use dataflow_rs::{Ctx, Engine, Error, Expr, Function, Message, Outcome, Path, Result, RunOptions};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use vs_v3::{Run, data, on_v3, on_v4, v3_engine, v3_engine_with, v4_engine, v4_engine_with};

/// `act`'s config.
#[derive(FunctionConfig)]
struct ActConfig {
    /// A step, or a list of steps, run in order.
    outcome: Expr,
}

/// A step's status: a number, never fractional.
fn status(step: &Value) -> u16 {
    let n = step.as_f64().expect("a step is a word or a status");
    u16::try_from(n as u64).unwrap()
}

/// Runs its steps.
struct Act;

impl Function for Act {
    type Config = ActConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &ActConfig) -> Result<Outcome> {
        tokio::task::yield_now().await;
        let steps = match Value::from(&*ctx.eval(&cfg.outcome)?) {
            Value::Array(steps) => steps,
            step => vec![step],
        };
        let mut result = Ok(Outcome::Ok);
        for step in steps {
            result = match step.as_str() {
                Some("record") => {
                    ctx.record_error(ErrorRecord::new("HOST_WARNING", "recorded by the handler"));
                    continue;
                }
                Some("ok") => Ok(Outcome::Ok),
                Some("skip") => Ok(Outcome::Skip),
                Some("halt") => Ok(Outcome::Halt),
                Some("err") => Err(Error::service("PARTNER_DOWN", "down")),
                _ => Ok(Outcome::Status(status(&step))),
            };
        }
        result
    }
}

/// v3's `act`, through `async_trait`.
mod v3_twin {
    use super::status;
    use async_trait::async_trait;
    use dataflow_rs_v3::{
        AsyncFunctionHandler, DataflowError, ErrorInfo, Result, TaskContext, TaskOutcome, Template,
        TemplateCompiler,
    };
    use serde::Deserialize;
    use serde_json::Value;

    #[derive(Deserialize)]
    pub struct ActInput {
        outcome: Template,
    }

    pub struct Act;

    #[async_trait]
    impl AsyncFunctionHandler for Act {
        type Input = ActInput;

        fn compile_input(input: &mut ActInput, c: &TemplateCompiler) -> Result<()> {
            input.outcome.compile(c, "outcome")
        }

        async fn execute(
            &self,
            ctx: &mut TaskContext<'_>,
            input: &ActInput,
        ) -> Result<TaskOutcome> {
            tokio::task::yield_now().await;
            let steps = match Value::from(&input.outcome.resolve(ctx)?) {
                Value::Array(steps) => steps,
                step => vec![step],
            };
            let mut result = Ok(TaskOutcome::Success);
            for step in steps {
                result = match step.as_str() {
                    Some("record") => {
                        ctx.add_error(
                            ErrorInfo::builder("HOST_WARNING", "recorded by the handler").build(),
                        );
                        continue;
                    }
                    Some("ok") => Ok(TaskOutcome::Success),
                    Some("skip") => Ok(TaskOutcome::Skip),
                    Some("halt") => Ok(TaskOutcome::Halt),
                    Some("err") => Err(DataflowError::service("PARTNER_DOWN", "down").build()),
                    _ => Ok(TaskOutcome::Status(status(&step))),
                };
            }
            result
        }
    }
}

/// The error context at `path`, keeping `limit`.
fn error_context(path: &str, limit: usize) -> ErrorContext {
    ErrorContext::new(path.parse().unwrap(), limit).unwrap()
}

/// v3's build of `workflows`, with `act` registered and its error context at
/// `path`, keeping `limit`.
fn v3(workflows: &[Value], path: &str, limit: usize) -> dataflow_rs_v3::Engine {
    v3_engine_with(workflows, |b| {
        b.register("act", v3_twin::Act)
            .with_error_context_path(path)
            .with_error_context_limit(limit)
    })
    .unwrap_or_else(|e| panic!("v3 builds: {e}"))
}

/// v4's twin of [`v3`].
fn v4(workflows: &[Value], path: &str, limit: usize) -> Engine {
    v4_engine_with(workflows, |b| {
        b.register("act", Act)
            .config(EngineConfig::default().error_context(error_context(path, limit)))
    })
    .unwrap_or_else(|e| panic!("v4 builds: {e}"))
}

/// Runs `run` on both builds of `workflows` and holds v4 to v3: the
/// normalised wire text and the outcome's code. Returns v4's text.
async fn compare(workflows: &[Value], path: &str, limit: usize, run: &Run) -> String {
    let expected = on_v3(&v3(workflows, path, limit), run).await;
    let (text, code, _) = on_v4(&v4(workflows, path, limit), run).await;
    assert_eq!(
        (&text, &code),
        (&expected.0, &expected.1),
        "workflows {workflows:?}, the error context at {path}, run over data {}",
        run.data
    );
    text
}

/// The text of the value at `path` in a normalised message's context, its
/// key order kept; `None` when nothing is there.
fn slot(message: &str, path: &str) -> Option<String> {
    let message = Json::from_text(message).unwrap();
    json_path::get(message.get("context").unwrap(), path).map(Json::to_text)
}

/// The records `(workflow_id, task_id, code, status)` as v3 writes them.
fn records(records: &[(&str, &str, &str, u16)]) -> String {
    let items: Vec<String> = records
        .iter()
        .map(|(w, t, c, s)| {
            format!(r#"{{"workflow_id":"{w}","task_id":"{t}","code":"{c}","status":{s}}}"#)
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// An `act` task running `outcome`.
fn act(id: &str, outcome: Value) -> Value {
    json!({"id": id, "function": {"name": "act", "input": {"outcome": outcome}}})
}

/// A `map` task writing `logic` at `path`.
fn map(id: &str, path: &str, logic: Value) -> Value {
    json!({"id": id, "function": {"name": "map", "input": {
        "mappings": [{"path": path, "logic": logic}]}}})
}

/// A `map` task that returns 500: an append to `data.text`, which every run
/// seeds with a string.
fn fails(id: &str) -> Value {
    json!({"id": id, "function": {"name": "map", "input": {
        "mappings": [{"path": "data.text", "logic": "x", "mode": "append"}]}}})
}

/// A `validation` task whose two rules fail: two records, and 400.
fn rules(id: &str) -> Value {
    json!({"id": id, "function": {"name": "validation", "input": {"rules": [
        {"logic": {"==": [{"var": "data.ok"}, true]}, "message": "not ok"},
        {"logic": {"!!": [{"var": "data.name"}]}, "message": "no name"}]}}})
}

/// A `parse_xml` task over a missing source: an `Err`.
fn bad_xml(id: &str) -> Value {
    json!({"id": id, "function": {"name": "parse_xml", "input": {
        "source": "data.xml", "target": "doc"}}})
}

/// `node` with each `(key, value)` of `extra` set on it.
fn with(mut node: Value, extra: &[(&str, Value)]) -> Value {
    for (key, value) in extra {
        node[*key] = value.clone();
    }
    node
}

/// A task that steps past its own failure.
fn goes_on(task: Value) -> Value {
    with(task, &[("continue_on_error", json!(true))])
}

/// A workflow `id` over `tasks`.
fn workflow(id: &str, tasks: Vec<Value>) -> Value {
    json!({"id": id, "tasks": tasks})
}

/// The run every test makes unless it says otherwise.
fn seeded() -> Run {
    data(json!({"text": "a string"}))
}

/// A message whose `data` is `data` and `metadata` is `metadata`.
fn message(data: Value, metadata: Value) -> Message {
    Message::from_parts(
        OwnedDataValue::Null,
        OwnedDataValue::from(&data),
        OwnedDataValue::from(&metadata),
    )
    .with_id("m")
}

fn text<T: serde::Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).unwrap()
}

/// The value at `path` in `m`'s context, as text, its key order kept.
fn at(m: &Message, path: &str) -> Option<String> {
    m.context().get(&path.parse().unwrap()).map(text)
}

#[tokio::test]
async fn a_failing_task_mirrors_its_records_at_the_path() {
    let workflows = [workflow(
        "w",
        vec![
            // A 5xx: the recorder's `TASK_STATUS_ERROR`.
            goes_on(act("five", json!(503))),
            // An `Err`: its record, at 500.
            goes_on(act("err", json!("err"))),
            // A 4xx records nothing, so it mirrors nothing.
            act("four", json!(404)),
            // `validation`'s two records, at its 400.
            rules("rules"),
            // A built-in's 5xx and a built-in's `Err`.
            goes_on(fails("map")),
            goes_on(bad_xml("xml")),
        ],
    )];
    let expected = records(&[
        ("w", "five", "TASK_STATUS_ERROR", 503),
        ("w", "err", "PARTNER_DOWN", 500),
        ("w", "rules", "VALIDATION_ERROR", 400),
        ("w", "rules", "VALIDATION_ERROR", 400),
        ("w", "map", "TASK_STATUS_ERROR", 500),
        ("w", "xml", "VALIDATION_ERROR", 500),
    ]);
    // Under each root, and created through missing objects.
    for path in ["metadata.errors", "data.failures.list", "temp_data.errs"] {
        let message = compare(&workflows, path, 32, &seeded()).await;
        assert_eq!(slot(&message, path), Some(expected.clone()), "{path}");
    }

    // A run whose only failure is a 4xx that records nothing writes no key.
    let workflows = [workflow("w", vec![act("four", json!(404))])];
    let message = compare(&workflows, "metadata.errors", 32, &seeded()).await;
    assert_eq!(slot(&message, "metadata.errors"), None);
}

#[tokio::test]
async fn a_handlers_own_records_are_mirrored_too() {
    let workflows = [workflow(
        "w",
        vec![
            // Recorded under `Ok`: mirrored at the call's 200.
            act("ok", json!(["record", "ok"])),
            // Recorded under a 4xx, which adds none of its own.
            act("four", json!(["record", 422])),
            // Recorded under `Err`: the handler's, then the call's.
            goes_on(act("err", json!(["record", "record", "err"]))),
            // Recorded under a halt.
            act("halt", json!(["record", "halt"])),
        ],
    )];
    let message = compare(&workflows, "metadata.errors", 32, &seeded()).await;
    assert_eq!(
        slot(&message, "metadata.errors"),
        Some(records(&[
            ("w", "ok", "HOST_WARNING", 200),
            ("w", "four", "HOST_WARNING", 422),
            ("w", "err", "HOST_WARNING", 500),
            ("w", "err", "HOST_WARNING", 500),
            ("w", "err", "PARTNER_DOWN", 500),
            ("w", "halt", "HOST_WARNING", 299),
        ]))
    );
}

#[tokio::test]
async fn the_limit_keeps_the_newest() {
    // Four sweeps, each failing at 500 plus its counter, so the records tell
    // the sweeps apart.
    let workflows = [
        json!({"id": "w", "loop": {"counter": "i", "max": 4}, "tasks": [
            goes_on(act("bad", json!({"+": [500, {"var": "temp_data.i"}]}))),
        ]}),
    ];
    let message = compare(&workflows, "metadata.errors", 2, &seeded()).await;
    assert_eq!(
        slot(&message, "metadata.errors"),
        Some(records(&[
            ("w", "bad", "TASK_STATUS_ERROR", 502),
            ("w", "bad", "TASK_STATUS_ERROR", 503),
        ]))
    );
    // Trimmed within one call too: three records, two kept.
    let workflows = [workflow(
        "w",
        vec![goes_on(act("many", json!(["record", "record", 500])))],
    )];
    let message = compare(&workflows, "metadata.errors", 2, &seeded()).await;
    assert_eq!(
        slot(&message, "metadata.errors"),
        Some(records(&[
            ("w", "many", "HOST_WARNING", 500),
            ("w", "many", "TASK_STATUS_ERROR", 500),
        ]))
    );
}

#[tokio::test]
async fn a_skip_mirrors_nothing_and_a_clean_run_leaves_the_key_absent() {
    // A `Skip` keeps the record its call made, on the message's errors, and
    // mirrors nothing.
    let skipping = [workflow(
        "w",
        vec![
            act("skips", json!(["record", "skip"])),
            map("m", "data.a", json!(1)),
        ],
    )];
    let clean = [workflow(
        "w",
        vec![map("m", "data.a", json!(1)), act("ok", json!("ok"))],
    )];
    for workflows in [&skipping[..], &clean[..]] {
        // The wire text is v3's, byte for byte once normalised: no key, not
        // an empty array.
        let message = compare(workflows, "metadata.errors", 32, &seeded()).await;
        assert_eq!(slot(&message, "metadata.errors"), None, "{workflows:?}");
    }
    let e = v4(&skipping, "metadata.errors", 32);
    let mut m = message(json!({}), json!({}));
    e.run(&mut m).await;
    assert_eq!(m.errors().len(), 1, "the skip's record is kept");
    assert_eq!(at(&m, "metadata.errors"), None);
}

#[tokio::test]
async fn a_fan_out_mirrors_per_element() {
    let workflows = [workflow(
        "w",
        vec![goes_on(with(
            act("each", json!({"var": "temp_data.x"})),
            &[(
                "for_each",
                json!({"over": {"var": "data.items"}, "as": "x"}),
            )],
        ))],
    )];
    let run = data(json!({"text": "a string", "items": [
        "ok",
        503,
        ["record", "ok"],
        "err",
        ["record", 404],
        ["record", "skip"],
    ]}));
    let message = compare(&workflows, "metadata.errors", 32, &run).await;
    // Each failed element's records, at its own status; the skipped
    // element's record is on the errors and nowhere else.
    assert_eq!(
        slot(&message, "metadata.errors"),
        Some(records(&[
            ("w", "each", "TASK_STATUS_ERROR", 503),
            ("w", "each", "HOST_WARNING", 200),
            ("w", "each", "PARTNER_DOWN", 500),
            ("w", "each", "HOST_WARNING", 404),
        ]))
    );
    // Run concurrently, the fold is the same.
    let concurrent = [workflow(
        "w",
        vec![goes_on(with(
            act("each", json!({"var": "temp_data.x"})),
            &[(
                "for_each",
                json!({"over": {"var": "data.items"}, "as": "x", "max_concurrency": 4}),
            )],
        ))],
    )];
    let again = compare(&concurrent, "metadata.errors", 32, &run).await;
    assert_eq!(
        slot(&again, "metadata.errors"),
        slot(&message, "metadata.errors")
    );
}

#[tokio::test]
async fn a_non_array_at_the_path_is_replaced() {
    // What a `map` writes over the path or above it, then two failures: the
    // first replaces it, and the second appends.
    let one = ("w", "bad", "TASK_STATUS_ERROR", 500);
    let two = ("w", "bad2", "TASK_STATUS_ERROR", 501);
    let both = records(&[one, two]);
    for (path, written_at, value, at_path, expected) in [
        // A string at the path.
        (
            "metadata.errors",
            "metadata.errors",
            json!("not a list"),
            "metadata.errors",
            both.clone(),
        ),
        // An object at the path.
        (
            "metadata.errors",
            "metadata.errors",
            json!({"a": 1}),
            "metadata.errors",
            both.clone(),
        ),
        // A number above it.
        (
            "data.fail.list",
            "data.fail",
            json!(5),
            "data.fail",
            format!(r#"{{"list":{both}}}"#),
        ),
        // An array above it, under a key.
        (
            "data.items.list",
            "data.items",
            json!([1, 2]),
            "data.items",
            format!(r#"{{"list":{both}}}"#),
        ),
        // An array above it, under an index: v3 reads every segment of the
        // path as a key.
        (
            "data.items.0",
            "data.items",
            json!([1, 2]),
            "data.items",
            format!(r#"{{"0":{both}}}"#),
        ),
        // A `null` above it.
        (
            "data.none.list",
            "data.none",
            json!(null),
            "data.none",
            format!(r#"{{"list":{both}}}"#),
        ),
        // An object above it keeps its keys.
        (
            "data.obj.list",
            "data.obj",
            json!({"keep": 1}),
            "data.obj",
            format!(r#"{{"keep":1,"list":{both}}}"#),
        ),
    ] {
        let workflows = [workflow(
            "w",
            vec![
                map("m", written_at, value.clone()),
                goes_on(act("bad", json!(500))),
                goes_on(act("bad2", json!(501))),
            ],
        )];
        let message = compare(&workflows, path, 32, &seeded()).await;
        assert_eq!(
            slot(&message, at_path),
            Some(expected),
            "{path} over {value} at {written_at}"
        );
    }
}

#[tokio::test]
async fn the_path_is_reset_at_run_start() {
    // Break 20: a message that arrives with records at the path loses them.
    // v3 appended to them.
    let workflows = [workflow("w", vec![goes_on(act("bad", json!(500)))])];
    let arriving = json!({"errors": [{"old": true}], "keep": 1});
    let run = Run {
        data: json!({"text": "a string"}),
        metadata: arriving.clone(),
        ..Run::default()
    };
    let fresh = records(&[("w", "bad", "TASK_STATUS_ERROR", 500)]);
    let (v4_text, _, _) = on_v4(&v4(&workflows, "metadata.errors", 32), &run).await;
    assert_eq!(slot(&v4_text, "metadata.errors"), Some(fresh));
    assert_eq!(slot(&v4_text, "metadata.keep"), Some("1".to_owned()));
    let (v3_text, _) = on_v3(&v3(&workflows, "metadata.errors", 32), &run).await;
    assert_eq!(
        slot(&v3_text, "metadata.errors"),
        Some(format!(
            r#"[{{"old":true}},{}]"#,
            records(&[("w", "bad", "TASK_STATUS_ERROR", 500)])
                .trim_start_matches('[')
                .trim_end_matches(']')
        )),
        "v3 appends"
    );

    // A clean run removes the key, and the trace's `initial` never holds it.
    let clean = [workflow("w", vec![map("m", "data.a", json!(1))])];
    let e = v4(&clean, "metadata.errors", 32);
    let mut m = message(json!({}), arriving);
    let run = e
        .run_with(
            &mut m,
            &RunOptions::default().trace(TraceOptions::default()),
        )
        .await;
    assert_eq!(at(&m, "metadata.errors"), None);
    assert_eq!(at(&m, "metadata.keep"), Some("1".to_owned()));
    let trace = run.trace().unwrap();
    assert_eq!(trace.initial.get(&"metadata.errors".parse().unwrap()), None);
    assert!(
        trace
            .initial
            .get(&"metadata.keep".parse().unwrap())
            .is_some()
    );

    // The reset goes through the path's segments: under `data` too.
    let e = v4(&workflows, "data.audit.errors", 32);
    let mut m = message(
        json!({"text": "a string", "audit": {"errors": [1, 2], "other": 0}}),
        json!({}),
    );
    e.run(&mut m).await;
    assert_eq!(
        at(&m, "data.audit"),
        Some(format!(
            r#"{{"other":0,"errors":{}}}"#,
            records(&[("w", "bad", "TASK_STATUS_ERROR", 500)])
        ))
    );
}

#[test]
fn new_refuses_a_root_metadata_progress_and_a_zero_limit() {
    for (path, limit) in [
        ("data", 32),
        ("metadata", 32),
        ("temp_data", 1),
        ("metadata.progress", 32),
        // `#` names the same key, so it is the same slot.
        ("metadata.#progress", 32),
        ("metadata.errors", 0),
        // At or below any slot the engine owns (review B M-8): v3 accepted
        // these, and the next progress write or the stamps wrote over them.
        ("metadata.progress.notes", 32),
        ("metadata.processed_at", 32),
        ("metadata.engine_version.x", 32),
        ("metadata.channel", 32),
    ] {
        let e = ErrorContext::new(path.parse().unwrap(), limit).unwrap_err();
        assert_eq!(*e.kind(), ErrorKind::Config, "{path}, {limit}: {e}");
    }
    // v3 refuses the same, but for `#progress`, whose check read the raw
    // segment.
    for (path, limit) in [
        ("data", 32),
        ("metadata", 32),
        ("temp_data", 1),
        ("metadata.progress", 32),
        ("metadata.errors", 0),
    ] {
        let refused = v3_engine_with(&[], |b| {
            b.with_error_context_path(path)
                .with_error_context_limit(limit)
        });
        assert!(refused.is_err(), "v3 refuses {path}, {limit}");
    }

    let ec = ErrorContext::new(
        "metadata.errors".parse().unwrap(),
        ErrorContext::DEFAULT_LIMIT,
    )
    .unwrap();
    assert_eq!(ec.path().to_string(), "metadata.errors");
    assert_eq!(ec.limit(), 32);
    assert_eq!(ErrorContext::DEFAULT_LIMIT, 32);
    // What v3 accepts beside it.
    for path in [
        "data.errors",
        "data.progress",
        "temp_data.a.b",
        "metadata.#errors",
        "metadata.errors.progress",
    ] {
        let ec = ErrorContext::new(path.parse().unwrap(), 1).unwrap();
        assert_eq!(ec.limit(), 1);
    }
    // A root outside the three cannot be spelled at all.
    assert!("payload.errors".parse::<Path>().is_err());
}

/// One traced run of `e` over `data`: the message and its trace.
async fn traced(e: &Engine, data: Value) -> (Message, Trace) {
    let mut m = message(data, json!({}));
    let run = e
        .run_with(
            &mut m,
            &RunOptions::default().trace(TraceOptions::default()),
        )
        .await;
    let trace = run.trace().cloned().unwrap();
    (m, trace)
}

/// The replay after the last step equals the message, key order included,
/// through `Trace::contexts` and through the corpus's replay.
fn assert_replays(trace: &Trace, m: &Message) {
    let last = trace.contexts().last().expect("the trace has a step");
    assert_eq!(text(&last), text(m.context()), "Trace::contexts");
    let wire = Json::from_text(&text(trace)).unwrap();
    let steps = wire.get("steps").and_then(Json::as_array).unwrap();
    let rebuilt = replay::rebuild(wire.get("initial").unwrap(), steps);
    let rebuilt = rebuilt.last().cloned().flatten().unwrap().to_text();
    let message = Json::from_text(&text(m.context())).unwrap().to_text();
    assert_eq!(rebuilt, message, "replay::rebuild");
}

#[tokio::test]
async fn the_trace_carries_the_record_as_a_change() {
    let workflows = [workflow(
        "w",
        vec![
            goes_on(act("first", json!(500))),
            map("m", "data.a", json!(1)),
            goes_on(act("second", json!(["record", "err"]))),
        ],
    )];
    let e = v4(&workflows, "metadata.errors", 32);
    let (m, trace) = traced(&e, json!({})).await;
    let step = |task: &str| {
        trace
            .steps
            .iter()
            .find(|s| s.task_id.as_deref() == Some(task))
            .unwrap_or_else(|| panic!("a step for {task}"))
    };
    let paths = |task: &str| -> Vec<String> {
        step(task)
            .changes
            .iter()
            .map(|c| c.path.to_string())
            .collect()
    };
    // After the progress write, in every step that mirrors; nowhere else.
    assert_eq!(paths("first"), ["metadata.progress", "metadata.errors"]);
    assert_eq!(paths("m"), ["data.a", "metadata.progress"]);
    assert_eq!(paths("second"), ["metadata.progress", "metadata.errors"]);
    assert_eq!(step("first").result, StepResult::Executed);
    let first = records(&[("w", "first", "TASK_STATUS_ERROR", 500)]);
    let change = &step("first").changes[1];
    assert_eq!(text(&change.old_value), "null");
    assert_eq!(text(&change.new_value), first);
    let change = &step("second").changes[1];
    assert_eq!(text(&change.old_value), first);
    assert_eq!(
        text(&change.new_value),
        records(&[
            ("w", "first", "TASK_STATUS_ERROR", 500),
            ("w", "second", "HOST_WARNING", 500),
            ("w", "second", "PARTNER_DOWN", 500),
        ])
    );
    assert_replays(&trace, &m);
    // The engine's write never enters an audit entry.
    for entry in m.audit() {
        assert!(
            entry.changes.iter().all(|c| &*c.path != "metadata.errors"),
            "{entry:?}"
        );
    }

    // A non-array at the path: the change's old value is what it replaced.
    let workflows = [workflow(
        "w",
        vec![
            map("m", "metadata.errors", json!("not a list")),
            goes_on(act("bad", json!(500))),
        ],
    )];
    let (m, trace) = traced(&v4(&workflows, "metadata.errors", 32), json!({})).await;
    let change = trace.steps.last().unwrap().changes.last().unwrap();
    assert_eq!(&*change.path, "metadata.errors");
    assert_eq!(text(&change.old_value), r#""not a list""#);
    assert_replays(&trace, &m);

    // A non-object above the path: the change is where the engine wrote, so
    // the replay still rebuilds the message.
    let workflows = [workflow(
        "w",
        vec![
            map("m", "data.fail", json!(5)),
            goes_on(act("bad", json!(500))),
        ],
    )];
    let (m, trace) = traced(&v4(&workflows, "data.fail.list", 32), json!({})).await;
    let change = trace.steps.last().unwrap().changes.last().unwrap();
    assert_eq!(&*change.path, "data.fail");
    assert_eq!(text(&change.old_value), "5");
    assert_eq!(
        text(&change.new_value),
        format!(
            r#"{{"list":{}}}"#,
            records(&[("w", "bad", "TASK_STATUS_ERROR", 500)])
        )
    );
    assert_replays(&trace, &m);

    // A fan-out's elements each carry their own.
    let workflows = [workflow(
        "w",
        vec![goes_on(with(
            act("each", json!({"var": "temp_data.x"})),
            &[("for_each", json!({"over": [500, "ok", "err"], "as": "x"}))],
        ))],
    )];
    let (m, trace) = traced(&v4(&workflows, "metadata.errors", 32), json!({})).await;
    let mirrored: Vec<usize> = trace
        .steps
        .iter()
        .filter(|s| s.changes.iter().any(|c| &*c.path == "metadata.errors"))
        .filter_map(|s| s.element_index)
        .collect();
    assert_eq!(mirrored, [0, 2]);
    assert_replays(&trace, &m);
}

#[tokio::test]
async fn a_reload_keeps_the_error_context() {
    let e = v4(
        &[workflow("w", vec![goes_on(act("t", json!(500)))])],
        "metadata.errors",
        2,
    );
    let compiler = e.compiler();
    let def = parse(
        &vs_v3::named(workflow(
            "w2",
            vec![goes_on(act("u", json!(503))), goes_on(act("v", json!(504)))],
        )),
        ParseOptions::default(),
    )
    .def
    .unwrap();
    let program = compiler.link([compiler.compile_one(&def).unwrap().workflow]);
    let reloaded = e.with_program(program);
    let config = |e: &Engine| {
        let debug = format!("{e:?}");
        debug[debug.find("config:").expect("the config is shown")..].to_owned()
    };
    assert_eq!(config(&reloaded), config(&e));
    // A path shows as its dotted text (review B M-9).
    assert!(
        config(&reloaded)
            .contains(r#"error_context: Some(ErrorContext { path: Path("metadata.errors")"#),
        "{}",
        config(&reloaded)
    );
    assert!(config(&reloaded).contains("limit: 2"));

    let mut m = message(json!({}), json!({}));
    reloaded.run(&mut m).await;
    assert_eq!(
        at(&m, "metadata.errors"),
        Some(records(&[
            ("w2", "u", "TASK_STATUS_ERROR", 503),
            ("w2", "v", "TASK_STATUS_ERROR", 504),
        ]))
    );
    // The old engine keeps its program, and the same error context.
    let mut m = message(json!({}), json!({}));
    e.run(&mut m).await;
    assert_eq!(
        at(&m, "metadata.errors"),
        Some(records(&[("w", "t", "TASK_STATUS_ERROR", 500)]))
    );
    // With none configured, neither engine writes a key.
    let workflows = [workflow("w", vec![goes_on(fails("map"))])];
    let expected = on_v3(&v3_engine(&workflows).unwrap(), &seeded()).await;
    let plain = v4_engine(&workflows).unwrap();
    let (text, code, _) = on_v4(&plain, &seeded()).await;
    assert_eq!((&text, &code), (&expected.0, &expected.1));
    assert_eq!(slot(&text, "metadata.errors"), None);
    assert!(format!("{plain:?}").contains("error_context: None"));
}
