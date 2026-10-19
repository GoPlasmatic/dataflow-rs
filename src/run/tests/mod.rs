//! The run module's recorder and clock tests.

use super::interp::RunEnv;
use super::policy::Flow;
use super::record::{Completed, PROGRESS, write_progress};
use crate::base::{CancelToken, Error};
use crate::def::{ParseOptions, parse};
use crate::expr::Path;
use crate::function::builtins::Builtin;
use crate::function::{Ctx, Function, Outcome, Registry};
use crate::observe::{Event, Interest, Observer, StepResult, TraceOptions};
use crate::program::{Callee, CompileOptions};
use crate::run::{EngineConfig, RunOptions};
use crate::state::{RunContext, Tuning};
use crate::{Engine, Message};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::sync::Arc;

thread_local! {
    /// How many times this thread read the recorder's clock.
    static CLOCK_READS: Cell<u32> = const { Cell::new(0) };
    /// What each recorded call completed with, while a test collects it.
    static COMPLETED: RefCell<Option<Vec<Done>>> = const { RefCell::new(None) };
    /// How much of the trace's own work this thread did.
    static TRACE_WORK: Cell<u32> = const { Cell::new(0) };
}

/// Notes one clock read on this thread.
pub(super) fn on_clock_read() {
    CLOCK_READS.with(|reads| reads.set(reads.get() + 1));
}

/// How many times this thread read [`super::record::now`].
pub(super) fn clock_reads() -> u32 {
    CLOCK_READS.with(Cell::get)
}

/// Notes one piece of trace-step work on this thread.
pub(super) fn on_trace_work() {
    TRACE_WORK.with(|work| work.set(work.get() + 1));
}

/// What one recorded call completed with, owned.
#[derive(Debug, PartialEq)]
struct Done {
    task: String,
    element: Option<usize>,
    status: Option<u16>,
    failed: bool,
    errors_before: usize,
    timed: bool,
}

/// `done`, kept while a test collects.
pub(super) fn completed(env: &RunEnv<'_, '_>, done: &Completed, status: Option<u16>, flow: &Flow) {
    COMPLETED.with_borrow_mut(|kept| {
        if let Some(kept) = kept {
            kept.push(Done {
                task: env.ids(done.task).task.to_string(),
                element: done.stamp.element_index,
                status,
                failed: matches!(flow, Flow::Fail(_) | Flow::Stop(_)),
                errors_before: done.errors_before,
                timed: done.elapsed.is_some(),
            });
        }
    });
}

fn reads() -> u32 {
    clock_reads()
}

/// Completes.
struct Answer;

impl Function for Answer {
    type Config = ();

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &()) -> crate::Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

/// Fails.
struct Down;

impl Function for Down {
    type Config = ();

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &()) -> crate::Result<Outcome> {
        Err(Error::service("PARTNER_DOWN", "down"))
    }
}

/// An observer with `interest` that keeps nothing.
struct Quiet(Interest);

impl Observer for Quiet {
    fn interest(&self) -> Interest {
        self.0
    }

    fn on(&self, _event: &Event<'_>) {}
}

/// An engine over `workflows` with `answer` and `down` registered and
/// `observer` its own.
fn engine(workflows: &[Value], observer: Option<Interest>) -> Engine {
    let mut config = EngineConfig::default();
    if let Some(interest) = observer {
        config = config.observer(Arc::new(Quiet(interest)));
    }
    workflows
        .iter()
        .fold(
            Engine::builder()
                .register("answer", Answer)
                .register("down", Down)
                .config(config),
            |b, w| b.workflow_json(w),
        )
        .build()
        .unwrap()
}

fn task(id: &str, function: &str, extra: Value) -> Value {
    let mut task = json!({"id": id, "function": {"name": function}});
    for (key, value) in extra.as_object().unwrap() {
        task[key] = value.clone();
    }
    task
}

fn map(id: &str, path: &str, mode: &str) -> Value {
    json!({"id": id, "function": {"name": "map", "input": {
        "mappings": [{"path": path, "logic": 1, "mode": mode}]}}})
}

/// What every call of one run of `e` completed with, over `data`.
async fn collect(e: &Engine, data: Value, opts: &RunOptions<'_>) -> Vec<Done> {
    COMPLETED.with_borrow_mut(|kept| *kept = Some(Vec::new()));
    let mut message = Message::from_parts(
        OwnedDataValue::Null,
        OwnedDataValue::from(&data),
        OwnedDataValue::Null,
    );
    e.run_with(&mut message, opts).await;
    COMPLETED.with_borrow_mut(Option::take).unwrap()
}

#[tokio::test]
async fn no_clock_is_read_without_an_interest_in_timings() {
    // Two sweeps of a built-in, a host call and a two-element fan-out.
    let workflows = [
        json!({"id": "w", "loop": {"counter": "i", "max": 2}, "tasks": [
            map("m", "data.a", "set"),
            task("h", "answer", json!({})),
            task("f", "answer", json!({"for_each": {"over": [1, 2], "as": "x"}})),
        ]}),
    ];
    let untimed = [
        None,
        Some(Interest::NONE),
        Some(Interest::TASKS),
        Some(Interest::WORKFLOWS | Interest::TASKS),
    ];
    for interest in untimed {
        let e = engine(&workflows, interest);
        let before = reads();
        let done = collect(&e, json!({}), &RunOptions::default()).await;
        assert_eq!(reads(), before, "{interest:?}");
        assert_eq!(done.len(), 8);
        assert!(done.iter().all(|d| !d.timed), "{interest:?}: {done:?}");
    }

    // Timed, per kind: the run's ends once each for any `TIMINGS`; the
    // workflow's start and end only for a slot holding `WORKFLOWS` and
    // `TIMINGS`; every call twice only for one holding `TASKS` and `TIMINGS`.
    // A `TIMINGS` beside another slot's interest times nothing for it.
    use Interest as I;
    for (engine_interest, run, expected, calls) in [
        (I::TASKS, I::TIMINGS, 2, false),
        (I::TASKS, I::WORKFLOWS | I::TIMINGS, 2 + 2, false),
        (I::WORKFLOWS, I::TASKS | I::TIMINGS, 2 + 8 * 2, true),
        (I::TASKS, I::ALL, 2 + 2 + 8 * 2, true),
    ] {
        let e = engine(&workflows, Some(engine_interest));
        let observer = Quiet(run);
        let before = reads();
        let opts = RunOptions::default().observer(&observer);
        let done = collect(&e, json!({}), &opts).await;
        assert_eq!(reads() - before, expected, "{engine_interest:?}, {run:?}");
        assert!(done.iter().all(|d| d.timed == calls), "{run:?}: {done:?}");
    }

    // A trace reads the clock only for its timings: two per call, and
    // neither the run's ends nor a workflow's start, which only an
    // observer's `TIMINGS` asks for.
    for observer in [None, Some(Interest::WORKFLOWS | Interest::TASKS)] {
        let e = engine(&workflows, observer);
        for (trace, expected) in [
            (TraceOptions::default(), 0),
            (TraceOptions::default().timings(true), 8 * 2),
        ] {
            let timed = trace.timings;
            let before = reads();
            let opts = RunOptions::default().trace(trace);
            let done = collect(&e, json!({}), &opts).await;
            assert_eq!(reads() - before, expected, "{observer:?}, timed {timed}");
            assert!(done.iter().all(|d| d.timed == timed), "{done:?}");
        }
    }
}

fn trace_work() -> u32 {
    TRACE_WORK.with(Cell::get)
}

#[tokio::test]
async fn an_untraced_run_does_no_trace_work() {
    // A step's records and a map's mapping ends are made only
    // under a trace, so a run without one, watched or not, does none of
    // that work: every call site that adds a record, a map of two
    // mappings, a workflow's own failure and a condition that fails.
    let workflows = [
        json!({"id": "w", "on_failure": "continue", "loop": {"counter": "i", "max": 2},
        "tasks": [
            {"id": "m", "function": {"name": "map", "input": {"mappings": [
                {"path": "data.a", "logic": 1},
                {"path": "data.l", "logic": 1, "mode": "append"}]}}},
            {"id": "v", "function": {"name": "validation", "input": {"rules": [
                {"logic": {"var": "data.missing"}, "message": "no"}]}}},
            task("h", "down", json!({"on_failure": "continue"})),
            task("f", "down", json!({"on_failure": "continue",
                "for_each": {"over": [1, 2], "as": "x"}})),
            task("stop", "down", json!({})),
        ]}),
        json!({"id": "c", "priority": 1, "condition": {"/": [1, {"var": "data.zero"}]},
            "tasks": [map("cm", "data.c", "set")]}),
        json!({"id": "o", "priority": 2, "on_failure": "continue",
            "loop": {"max": 1, "over": {"var": "data.a"}, "as": "x"},
            "tasks": [map("om", "data.o", "set")]}),
    ];
    let failures: Path = "temp_data.failures".parse().unwrap();
    let build = |observer: Option<Interest>, mirrored: bool| {
        let mut config = EngineConfig::default();
        if let Some(interest) = observer {
            config = config.observer(Arc::new(Quiet(interest)));
        }
        if mirrored {
            let ec = crate::run::ErrorContext::new(failures.clone(), 8).unwrap();
            config = config.error_context(ec);
        }
        workflows
            .iter()
            .fold(
                Engine::builder().register("down", Down).config(config),
                |b, w| b.workflow_json(w),
            )
            .build()
            .unwrap()
    };
    let data = json!({"zero": 0});
    for (observer, mirrored) in [
        (None, false),
        (Some(Interest::ALL), false),
        (None, true),
        (Some(Interest::TASKS), true),
    ] {
        let e = build(observer, mirrored);
        let before = trace_work();
        let done = collect(&e, data.clone(), &RunOptions::default()).await;
        assert_eq!(done.len(), 12, "{done:?}");
        assert_eq!(trace_work(), before, "{observer:?}, mirrored {mirrored}");
    }
    // The same runs traced do it.
    let e = build(None, true);
    let before = trace_work();
    let opts = RunOptions::default().trace(TraceOptions::default());
    collect(&e, data, &opts).await;
    assert!(trace_work() > before, "a traced run copies its records");
}

/// `(task, element, status, failed, errors_before)`.
type Site = (&'static str, Option<usize>, Option<u16>, bool, usize);

fn sites(done: &[Done]) -> Vec<Site> {
    done.iter()
        .map(|d| {
            let task = ["m", "bad", "h", "f", "e", "n", "stop", "g", "c", "fc"]
                .into_iter()
                .find(|&t| t == d.task)
                .unwrap_or_else(|| panic!("an unknown task {}", d.task));
            (task, d.element, d.status, d.failed, d.errors_before)
        })
        .collect()
}

#[tokio::test]
async fn every_call_site_completes_with_its_failure_and_its_error_count() {
    let on_continue = json!({"on_failure": "continue"});
    let e = engine(
        &[
            json!({"id": "w", "on_failure": "continue", "tasks": [
                map("m", "data.a", "set"),
                // An append to a string fails the mapping: a 500.
                {"id": "bad", "on_failure": "continue", "function": {"name": "map",
                    "input": {"mappings": [
                        {"path": "data.text", "logic": 1, "mode": "append"}]}}},
                task("h", "down", on_continue.clone()),
                task("f", "answer", json!({"for_each": {"over": [1, 2], "as": "x"}})),
                task("e", "answer", json!({"for_each": {"over": [], "as": "x"}})),
                task("n", "answer", json!({"on_failure": "continue",
                    "for_each": {"over": {"var": "data.text"}, "as": "x"}})),
                // Its failure goes to the workflow, which steps past it.
                task("stop", "down", json!({})),
            ]}),
            json!({"id": "x", "priority": 1, "tasks": [
                task("g", "down", json!({"for_each": {"over": [1, 2], "as": "x"}})),
            ]}),
        ],
        Some(Interest::ALL),
    );
    let done = collect(&e, json!({"text": "t"}), &RunOptions::default()).await;
    assert_eq!(
        sites(&done),
        [
            ("m", None, Some(200), false, 0),
            ("bad", None, Some(500), false, 0),
            ("h", None, Some(500), false, 2),
            ("f", Some(0), Some(200), false, 3),
            ("f", Some(1), Some(200), false, 3),
            ("e", None, Some(200), false, 3),
            ("n", None, Some(500), false, 3),
            ("stop", None, Some(500), true, 4),
            // The first element fails the task, and the fold stops.
            ("g", Some(0), Some(500), true, 5),
        ]
    );
    assert!(done.iter().all(|d| d.timed), "{done:?}");

    // Cut short by the run's token: a host call, and a fan-out recorded
    // once, with no element.
    let token = CancelToken::new();
    token.cancel();
    let opts = RunOptions::default().cancel(&token);
    for (workflow, expected) in [
        (
            task("c", "answer", json!({})),
            ("c", None, Some(500), true, 0),
        ),
        (
            task(
                "fc",
                "answer",
                json!({"for_each": {"over": [1, 2], "as": "x"}}),
            ),
            ("fc", None, Some(500), true, 0),
        ),
    ] {
        let e = engine(&[json!({"id": "w", "tasks": [workflow]})], None);
        let done = collect(&e, json!({}), &opts).await;
        assert_eq!(sites(&done), [expected]);
        assert!(!done[0].timed);
    }
}

fn text(v: &OwnedDataValue) -> String {
    serde_json::to_string(v).unwrap()
}

#[tokio::test]
async fn a_builtin_that_writes_then_fails_gives_its_writes_to_the_trace_alone() {
    // No shipped built-in writes and then returns `Err` today, so the test
    // swaps the call site's callee for the crate-private `Builtin::Fault`.
    // Its write stands: its `Failed` step carries it and the trace replays to
    // the message. The audit entry gets none, as v3's recorder dropped them,
    // with a trace and without one.
    let registry = Registry::default();
    let compiler = registry.compiler(CompileOptions::default());
    let def = parse(
        &json!({"id": "w", "tasks": [map("m", "data.a", "set")]}),
        ParseOptions::default(),
    )
    .def
    .unwrap();
    let mut wf = compiler.compile_one(&def).unwrap().workflow;
    wf.calls[0].callee = Callee::Builtin(Builtin::Fault);
    let e = Engine::new(compiler.link([wf]), EngineConfig::default());

    let traced = RunOptions::default().trace(TraceOptions::default());
    for opts in [RunOptions::default(), traced] {
        let mut message = Message::from_parts(
            OwnedDataValue::Null,
            OwnedDataValue::from(&json!({})),
            OwnedDataValue::Null,
        );
        let run = e.run_with(&mut message, &opts).await;
        assert!(
            matches!(run.status(), crate::run::RunStatus::Stopped(_)),
            "{run:?}"
        );
        let context = message.context();
        assert_eq!(text(&context.data()["a"]), "1", "the write stands");
        let audit = message.audit();
        assert_eq!(audit.len(), 1);
        assert!(audit[0].changes.is_empty(), "{audit:?}");
        assert_eq!(audit[0].status, 500);
        let Some(trace) = run.trace() else {
            continue;
        };
        let step = &trace.steps[0];
        assert_eq!((step.result, step.status), (StepResult::Failed, Some(500)));
        let paths: Vec<&str> = step.changes.iter().map(|c| &*c.path).collect();
        assert_eq!(paths, ["data.a", "metadata.progress"]);
        let last = trace.contexts().last().expect("a step");
        assert_eq!(
            serde_json::to_string(&last).unwrap(),
            serde_json::to_string(context).unwrap(),
            "the trace replays to the message"
        );
    }
}

/// A context with `progress` (JSON text, its key order kept) seeded.
fn context(progress: &str) -> OwnedDataValue {
    let text = format!(r#"{{"data":{{}},"metadata":{{"progress":{progress}}},"temp_data":{{}}}}"#);
    serde_json::from_str(&text).unwrap()
}

/// The buffer of the string at `metadata.progress.<key>`.
fn buffer(ctx: &OwnedDataValue, key: &str) -> *const u8 {
    match &ctx["metadata"]["progress"][key] {
        OwnedDataValue::String(s) => s.as_ptr(),
        other => panic!("{key} holds {other:?}"),
    }
}

#[test]
fn with_no_view_a_progress_of_its_three_keys_is_updated_where_it_is() {
    let mut ctx = context(r#"{"task_id":"a_long_task","status_code":1,"workflow_id":"wf"}"#);
    let before = (buffer(&ctx, "task_id"), buffer(&ctx, "workflow_id"));
    let mut rc = RunContext::new(&mut ctx, Tuning::default());
    write_progress(&mut rc, "wf", "t", 200);
    assert!(!rc.has_view(), "the write built no view");
    rc.finish();
    assert_eq!(
        text(&ctx["metadata"]["progress"]),
        r#"{"task_id":"t","status_code":200,"workflow_id":"wf"}"#,
        "the keys keep their order"
    );
    assert_eq!(
        (buffer(&ctx, "task_id"), buffer(&ctx, "workflow_id")),
        before,
        "both ids were written into the strings already there"
    );
}

/// The buffer of the progress object's pairs.
fn pairs(ctx: &OwnedDataValue) -> *const (String, OwnedDataValue) {
    match &ctx["metadata"]["progress"] {
        OwnedDataValue::Object(pairs) => pairs.as_ptr(),
        other => panic!("progress holds {other:?}"),
    }
}

#[test]
fn with_a_view_a_progress_of_its_three_keys_is_updated_where_it_is() {
    let mut ctx = context(r#"{"task_id":"a_long_task","status_code":200,"workflow_id":"wf"}"#);
    let before = (pairs(&ctx), buffer(&ctx, "workflow_id"));
    let mut rc = RunContext::new(&mut ctx, Tuning::default());
    rc.read(|_, _| ());
    write_progress(&mut rc, "wf", "t", 200);
    let stats = rc.finish();
    assert_eq!(
        text(&ctx["metadata"]["progress"]),
        r#"{"task_id":"t","status_code":200,"workflow_id":"wf"}"#,
        "the keys keep their order"
    );
    assert_eq!(stats.converted, 1, "only the task id changed: {stats:?}");
    assert_eq!(
        (pairs(&ctx), buffer(&ctx, "workflow_id")),
        before,
        "the object and the id that did not change stayed where they were"
    );

    // The same write again finds every value already there.
    let mut rc = RunContext::new(&mut ctx, Tuning::default());
    rc.read(|_, _| ());
    write_progress(&mut rc, "wf", "t", 200);
    assert!(
        matches!(rc.get(&PROGRESS), Some(Cow::Borrowed(_))),
        "nothing was marked"
    );
    assert_eq!(rc.finish().converted, 0);
}

#[test]
fn with_a_view_the_progress_write_leaves_what_it_leaves_without_one() {
    // A key that comes twice is overwritten twice, as v3's loop over the
    // pairs does, and the object is then marked as a whole.
    for seeded in [
        r#"{"workflow_id":"wf","task_id":"t0","status_code":200}"#,
        r#"{"status_code":1,"workflow_id":"w0","task_id":"t"}"#,
        r#"{"task_id":5,"workflow_id":null,"status_code":"s"}"#,
        r#"{"task_id":"t","status_code":200.5,"workflow_id":"wf"}"#,
        r#"{"task_id":"y","task_id":"z","status_code":1}"#,
        r#"{"task_id":"y","status_code":1}"#,
        r#"{"task_id":"y","status_code":1,"other":0}"#,
        r#"{"task_id":"y","status_code":1,"workflow_id":"x","extra":0}"#,
        "[1,2,3]",
        "null",
    ] {
        let run = |viewed: bool| {
            let mut ctx = context(seeded);
            let mut rc = RunContext::new(&mut ctx, Tuning::default());
            let mut seen = Vec::new();
            for (workflow, task, status) in [
                ("wf", "t", 200),
                ("wf", "a_longer_task", 404),
                ("w2", "a_longer_task", 404),
                ("w2", "a_longer_task", 404),
            ] {
                if viewed {
                    rc.read(|_, _| ());
                }
                write_progress(&mut rc, workflow, task, status);
                seen.push(text(&rc.get(&PROGRESS).expect("a progress was written")));
            }
            assert_eq!(rc.has_view(), viewed);
            rc.finish();
            seen.push(text(&ctx["metadata"]["progress"]));
            seen
        };
        assert_eq!(run(true), run(false), "{seeded}");
    }
}

#[test]
fn any_other_progress_is_replaced_in_the_fresh_order() {
    for seeded in [
        r#"{"task_id":"y","status_code":1}"#,
        r#"{"task_id":"y","status_code":1,"other":0}"#,
        r#"{"task_id":"y","status_code":1,"workflow_id":"x","extra":0}"#,
        "[1,2,3]",
    ] {
        let mut ctx = context(seeded);
        let mut rc = RunContext::new(&mut ctx, Tuning::default());
        write_progress(&mut rc, "wf", "t", 200);
        rc.finish();
        assert_eq!(
            text(&ctx["metadata"]["progress"]),
            r#"{"workflow_id":"wf","task_id":"t","status_code":200}"#,
            "{seeded}"
        );
    }
}
