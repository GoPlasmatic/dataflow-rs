// Observers end to end, through `Engine::builder()` and `run_with`: v3's
// `tests/observer.rs` ported, with `validation` where v3 reported
// `validate` and the `Run`'s status where v3 returned an `Err`,
// plus the interest rules: each observer is called with only the
// events its interest names, `elapsed` is `Some` only for one holding
// `TIMINGS`, and the interest is read once per run.
//
// v3's `observer_durations_are_populated_and_with_handlers_reaches_the_builder`
// has no twin: `with_handlers` is gone (a host function is registered
// through `Engine::builder().register`), and the durations it read are
// pinned by `each_observer_receives_only_its_interest` and
// `no_clock_is_read_without_an_interest_in_timings`.
//
// A `Timeline` records each event as v3's timeline spelt it, with what a
// task event carried and the event's `elapsed`. Two host functions stand in
// for v3's fixtures: `logger` completes (in a fan-out, after yielding fewer
// times for a later element, so the elements finish in reverse order), and
// `fail` returns a service error.

use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::observe::{Event, Interest, Observer};
use dataflow_rs::run::{EngineConfig, RunStatus, WorkflowExit};
use dataflow_rs::{Ctx, Engine, Error, Function, Message, Outcome, Result, RunOptions};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What a `TaskFinished` carried, owned.
#[derive(Clone, Debug, PartialEq)]
struct Task {
    workflow_id: String,
    task_id: String,
    function: String,
    status: Option<u16>,
    loop_counter: Option<i64>,
    element_index: Option<usize>,
}

/// One event as an observer saw it.
#[derive(Clone, Debug)]
struct Seen {
    /// The event, as v3's timeline spells it.
    line: String,
    /// What a `TaskFinished` carried.
    task: Option<Task>,
    /// The event's `elapsed`; `None` for an event that carries none.
    elapsed: Option<Duration>,
}

/// An observer with `interest`, recording every event it is given.
#[derive(Clone)]
struct Timeline {
    interest: Interest,
    /// How many times `interest` was read.
    asked: Arc<AtomicUsize>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Timeline {
    fn new(interest: Interest) -> Self {
        Self {
            interest,
            asked: Arc::default(),
            seen: Arc::default(),
        }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn lines(&self) -> Vec<String> {
        self.seen().into_iter().map(|s| s.line).collect()
    }

    fn tasks(&self) -> Vec<Task> {
        self.seen().into_iter().filter_map(|s| s.task).collect()
    }

    fn asked(&self) -> usize {
        self.asked.load(Ordering::Relaxed)
    }
}

impl Observer for Timeline {
    fn interest(&self) -> Interest {
        self.asked.fetch_add(1, Ordering::Relaxed);
        self.interest
    }

    fn on(&self, event: &Event<'_>) {
        let (line, task, elapsed) = match event {
            Event::RunStarted { considered, .. } => {
                (format!("run_started({considered})"), None, None)
            }
            Event::WorkflowStarted { id, .. } => (format!("workflow_started({id})"), None, None),
            Event::TaskFinished {
                info,
                status,
                elapsed,
                ..
            } => (
                format!("task_finished({}/{})", info.workflow_id, info.task_id),
                Some(Task {
                    workflow_id: info.workflow_id.to_owned(),
                    task_id: info.task_id.to_owned(),
                    function: info.function.to_owned(),
                    status: *status,
                    loop_counter: info.loop_counter,
                    element_index: info.element_index,
                }),
                *elapsed,
            ),
            Event::WorkflowFinished {
                record, elapsed, ..
            } => (
                format!(
                    "workflow_finished({}, sweeps={}, halted={})",
                    record.id,
                    record.sweeps,
                    record.exit == Some(WorkflowExit::Halted)
                ),
                None,
                *elapsed,
            ),
            Event::RunFinished {
                status, elapsed, ..
            } => (
                format!(
                    "run_finished(stopped_early={})",
                    matches!(
                        status,
                        RunStatus::Stopped(_) | RunStatus::TimedOut | RunStatus::Cancelled
                    )
                ),
                None,
                *elapsed,
            ),
            other => panic!("an event this test does not know: {other:?}"),
        };
        self.seen.lock().unwrap().push(Seen {
            line,
            task,
            elapsed,
        });
    }
}

/// Completes. In a fan-out it first yields `5 - index` times, so a later
/// element finishes first.
struct Logger;

impl Function for Logger {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        let yields = ctx.info().element_index.map_or(1, |i| 5 - i.min(5));
        for _ in 0..yields {
            tokio::task::yield_now().await;
        }
        Ok(Outcome::Ok)
    }
}

/// Fails with a service error.
struct Failing;

impl Function for Failing {
    type Config = ();

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        Err(Error::service("PARTNER_DOWN", "down"))
    }
}

/// A `map` task writing `1` at `data.<id>`.
fn map(id: &str) -> Value {
    json!({"id": id, "function": {"name": "map", "input": {
        "mappings": [{"path": format!("data.{id}"), "logic": 1}]}}})
}

/// A task `id` calling `function` with no input.
fn host(id: &str, function: &str) -> Value {
    json!({"id": id, "function": {"name": function}})
}

/// A workflow `id` over `tasks`, with each `(key, value)` of `extra`.
fn workflow(id: &str, tasks: Vec<Value>, extra: &[(&str, Value)]) -> Value {
    let mut w = json!({"id": id, "tasks": tasks});
    for (key, value) in extra {
        w[*key] = value.clone();
    }
    w
}

/// An engine over `workflows`, the host functions registered and
/// `observer` the engine's, if any.
fn engine(workflows: &[Value], observer: Option<&Timeline>) -> Engine {
    let mut config = EngineConfig::default();
    if let Some(observer) = observer {
        config = config.observer(Arc::new(observer.clone()));
    }
    workflows
        .iter()
        .fold(
            Engine::builder()
                .register("logger", Logger)
                .register("fail", Failing)
                .config(config),
            |b, w| b.workflow_json(w),
        )
        .build()
        .unwrap_or_else(|issues| panic!("the engine builds: {issues:?}"))
}

/// A message whose `data` is `data`.
fn message(data: Value) -> Message {
    Message::from_parts(
        OwnedDataValue::Null,
        OwnedDataValue::from(&data),
        OwnedDataValue::Null,
    )
    .with_id("m")
}

/// The timeline of one run of `workflows` over an empty message, under
/// `opts`, observed by the engine.
async fn timeline_for(workflows: &[Value], opts: &RunOptions<'_>) -> Vec<String> {
    let timeline = Timeline::new(Interest::WORKFLOWS | Interest::TASKS);
    let e = engine(workflows, Some(&timeline));
    e.run_with(&mut message(json!({})), opts).await;
    timeline.lines()
}

fn lines(expected: &[&str]) -> Vec<String> {
    expected.iter().map(|&s| s.to_owned()).collect()
}

#[tokio::test]
async fn every_recorded_call_reports_sync_builtins_included() {
    // v3's reason for the observer: the built-ins never reached its
    // registry, so a host could not wrap them. Both call sites report.
    let timeline = Timeline::new(Interest::TASKS);
    let e = engine(
        &[workflow(
            "w",
            vec![
                map("m"),
                json!({"id": "v", "function": {"name": "validation", "input": {"rules": []}}}),
                json!({"id": "l", "function": {"name": "log", "input": {"message": "hi"}}}),
                host("custom", "logger"),
            ],
            &[],
        )],
        Some(&timeline),
    );
    let run = e.run(&mut message(json!({}))).await;
    assert!(matches!(run.status(), RunStatus::Completed), "{run:?}");

    let tasks = timeline.tasks();
    let ids: Vec<_> = tasks.iter().map(|t| t.task_id.as_str()).collect();
    assert_eq!(ids, ["m", "v", "l", "custom"], "every call reports");
    // The registered names: `validation`, the one spelling.
    let functions: Vec<_> = tasks.iter().map(|t| t.function.as_str()).collect();
    assert_eq!(functions, ["map", "validation", "log", "logger"]);
    assert!(tasks.iter().all(|t| t.status == Some(200)), "{tasks:?}");
    assert!(tasks.iter().all(|t| t.workflow_id == "w"), "{tasks:?}");
}

#[tokio::test]
async fn the_sync_and_async_paths_emit_identical_sequences() {
    // v3's hazard, in v4's form: a built-in and a host call build their
    // record at two sites (`interp::call`, `call_host`), and an observer
    // must not tell them apart. The same shape, a two-sweep loop of one
    // task, over `map` and over a host function.
    let looped = |task: Value| {
        workflow(
            "w",
            vec![task],
            &[("loop", json!({"counter": "i", "max": 2}))],
        )
    };
    let shape = |timeline: &Timeline| {
        let tasks: Vec<_> = timeline
            .tasks()
            .into_iter()
            .map(|t| {
                (
                    t.workflow_id,
                    t.task_id,
                    t.status,
                    t.loop_counter,
                    t.element_index,
                )
            })
            .collect();
        (timeline.lines(), tasks)
    };
    let sync = Timeline::new(Interest::WORKFLOWS | Interest::TASKS);
    let e = engine(&[looped(map("a"))], Some(&sync));
    e.run(&mut message(json!({}))).await;
    let hosted = Timeline::new(Interest::WORKFLOWS | Interest::TASKS);
    let e = engine(&[looped(host("a", "logger"))], Some(&hosted));
    e.run(&mut message(json!({}))).await;

    assert_eq!(
        sync.lines(),
        lines(&[
            "run_started(1)",
            "workflow_started(w)",
            "task_finished(w/a)",
            "task_finished(w/a)",
            "workflow_finished(w, sweeps=2, halted=false)",
            "run_finished(stopped_early=false)",
        ])
    );
    assert_eq!(
        shape(&sync),
        shape(&hosted),
        "the two call sites must be indistinguishable to an observer"
    );
}

#[tokio::test]
async fn a_failing_task_is_reported_before_the_run_stops() {
    let timeline = Timeline::new(Interest::TASKS);
    let e = engine(
        &[workflow("w", vec![map("ok"), host("boom", "fail")], &[])],
        Some(&timeline),
    );
    let run = e.run(&mut message(json!({}))).await;
    assert!(matches!(run.status(), RunStatus::Stopped(_)), "{run:?}");

    let tasks = timeline.tasks();
    assert_eq!(tasks.len(), 2, "the failing task is reported: {tasks:?}");
    assert_eq!(tasks[1].task_id, "boom");
    assert_eq!(tasks[1].status, Some(500), "an `Err` reports 500");
}

#[tokio::test]
async fn a_refused_condition_is_not_reported_but_a_skip_outcome_is() {
    let timeline = Timeline::new(Interest::TASKS);
    let mut never = map("never");
    never["condition"] = json!({"==": [{"var": "data.x"}, 2]});
    let e = engine(
        &[workflow(
            "w",
            vec![
                never,
                json!({"id": "gate", "function": {"name": "filter", "input": {
                    "condition": false, "on_reject": "skip"}}}),
            ],
            &[],
        )],
        Some(&timeline),
    );
    let run = e.run(&mut message(json!({"x": 1}))).await;
    assert!(matches!(run.status(), RunStatus::Completed), "{run:?}");

    let tasks = timeline.tasks();
    // The task its condition refused never ran; a `Skip` ran and records no
    // status.
    assert_eq!(tasks.len(), 1, "only the call reports: {tasks:?}");
    assert_eq!(tasks[0].task_id, "gate");
    assert_eq!(tasks[0].status, None, "a `Skip` reports no status");
}

#[tokio::test]
async fn the_observer_survives_a_reload() {
    // A reload that dropped the observer would stop a host's metrics at the
    // first reload, silently.
    let timeline = Timeline::new(Interest::TASKS);
    let e = engine(&[workflow("w", vec![map("a")], &[])], Some(&timeline));
    e.run(&mut message(json!({}))).await;
    assert_eq!(timeline.tasks().len(), 1);

    let compiler = e.compiler();
    let def = parse(
        &workflow("w2", vec![map("a"), map("b")], &[]),
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
    assert_eq!(
        config(&reloaded),
        "config: EngineConfig { observer: present, error_context: None, .. } }"
    );
    reloaded.run(&mut message(json!({}))).await;
    assert_eq!(
        timeline.tasks().len(),
        3,
        "the reloaded engine keeps reporting"
    );
}

#[tokio::test]
async fn a_two_workflow_run_reports_nested_boundaries_in_order() {
    let events = timeline_for(
        &[
            workflow("first", vec![map("a")], &[]),
            workflow("second", vec![map("b")], &[]),
        ],
        &RunOptions::default(),
    )
    .await;
    assert_eq!(
        events,
        lines(&[
            "run_started(2)",
            "workflow_started(first)",
            "task_finished(first/a)",
            "workflow_finished(first, sweeps=1, halted=false)",
            "workflow_started(second)",
            "task_finished(second/b)",
            "workflow_finished(second, sweeps=1, halted=false)",
            "run_finished(stopped_early=false)",
        ]),
        "a task's event nests inside its workflow's pair"
    );
}

#[tokio::test]
async fn a_workflow_refused_by_its_condition_or_its_rollout_emits_no_workflow_events() {
    let events = timeline_for(
        &[
            workflow("skipped", vec![map("a")], &[("condition", json!(false))]),
            workflow("ran", vec![map("b")], &[]),
        ],
        &RunOptions::default(),
    )
    .await;
    assert!(
        !events.iter().any(|e| e.contains("skipped")),
        "a workflow its condition refused never started: {events:?}"
    );
    assert!(events.contains(&"workflow_started(ran)".to_owned()));
    // The run still considered both.
    assert_eq!(events[0], "run_started(2)");

    let rollout = json!({"bucket_start": 0, "bucket_end": 10});
    let events = timeline_for(
        &[workflow("canary", vec![map("a")], &[("rollout", rollout)])],
        &RunOptions::default().routing_bucket(50),
    )
    .await;
    assert_eq!(
        events,
        lines(&["run_started(1)", "run_finished(stopped_early=false)"]),
        "the rollout refuses before the workflow starts"
    );
}

#[tokio::test]
async fn a_looping_workflow_reports_one_pair_with_the_sweep_count() {
    let timeline = Timeline::new(Interest::WORKFLOWS | Interest::TASKS);
    let e = engine(
        &[workflow(
            "sweeper",
            vec![map("a")],
            &[(
                "loop",
                json!({"counter": "i", "init": 0, "max": 3, "increment": 1}),
            )],
        )],
        Some(&timeline),
    );
    e.run(&mut message(json!({}))).await;
    let events = timeline.lines();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.starts_with("workflow_started"))
            .count(),
        1,
        "one pair for the whole loop, not one per sweep: {events:?}"
    );
    assert!(
        events.contains(&"workflow_finished(sweeper, sweeps=3, halted=false)".to_owned()),
        "the sweep count carries the cardinality: {events:?}"
    );
    // The body reports per sweep, each with its sweep's counter.
    let counters: Vec<_> = timeline.tasks().iter().map(|t| t.loop_counter).collect();
    assert_eq!(counters, [Some(0), Some(1), Some(2)]);
}

#[tokio::test]
async fn a_halted_workflow_says_so_on_its_record() {
    let events = timeline_for(
        &[workflow(
            "guarded",
            vec![
                map("a"),
                json!({"id": "gate", "function": {"name": "filter", "input": {
                    "condition": false, "on_reject": "halt"}}}),
                map("never"),
            ],
            &[],
        )],
        &RunOptions::default(),
    )
    .await;
    assert!(
        events.contains(&"workflow_finished(guarded, sweeps=1, halted=true)".to_owned()),
        "{events:?}"
    );
    assert!(!events.iter().any(|e| e.contains("/never")), "{events:?}");
}

#[tokio::test]
async fn a_run_that_stops_early_still_reports_run_finished() {
    let timeline = Timeline::new(Interest::WORKFLOWS | Interest::TASKS);
    let e = engine(
        &[workflow("w", vec![host("boom", "fail")], &[])],
        Some(&timeline),
    );
    let run = e.run(&mut message(json!({}))).await;
    assert!(run.partial(), "the fixture stops the run: {run:?}");

    let events = timeline.lines();
    assert_eq!(
        events.last().map(String::as_str),
        Some("run_finished(stopped_early=true)"),
        "a stopped run is the one most worth measuring: {events:?}"
    );
    assert!(
        events.contains(&"workflow_finished(w, sweeps=1, halted=false)".to_owned()),
        "and the failing workflow's record closes: {events:?}"
    );
}

#[tokio::test]
async fn a_fan_out_reports_each_folded_element_and_an_empty_over_once() {
    let timeline = Timeline::new(Interest::TASKS);
    let mut fan = host("t", "logger");
    fan["for_each"] = json!({"over": {"var": "data.items"}, "as": "item", "max_concurrency": 3});
    let e = engine(&[workflow("w", vec![fan], &[])], Some(&timeline));

    // The calls finish in reverse order; the fold, and so the events, go in
    // element order.
    e.run(&mut message(json!({"items": [1, 2, 3]}))).await;
    let elements: Vec<_> = timeline.tasks().iter().map(|t| t.element_index).collect();
    assert_eq!(elements, [Some(0), Some(1), Some(2)]);
    assert!(timeline.tasks().iter().all(|t| t.status == Some(200)));

    // An empty `over` records the task once, with no element.
    e.run(&mut message(json!({"items": []}))).await;
    let tasks = timeline.tasks();
    assert_eq!(tasks.len(), 4, "{tasks:?}");
    assert_eq!((tasks[3].element_index, tasks[3].status), (None, Some(200)));

    // So does an `over` that is not an array, as the task's failure.
    e.run(&mut message(json!({"items": "no"}))).await;
    let tasks = timeline.tasks();
    assert_eq!(tasks.len(), 5, "{tasks:?}");
    assert_eq!((tasks[4].element_index, tasks[4].status), (None, Some(500)));
}

/// Keeps each run event's message id, and the error count of each end.
#[derive(Default)]
struct Ends(Mutex<Vec<(&'static str, String, Option<usize>)>>);

impl Observer for Ends {
    fn interest(&self) -> Interest {
        Interest::NONE
    }

    fn on(&self, event: &Event<'_>) {
        let seen = match event {
            Event::RunStarted { message_id, .. } => ("started", message_id.to_string(), None),
            Event::RunFinished {
                message_id, errors, ..
            } => ("finished", message_id.to_string(), Some(*errors)),
            _ => return,
        };
        self.0.lock().unwrap().push(seen);
    }
}

#[tokio::test]
async fn each_run_finished_names_its_own_message_when_runs_overlap() {
    // Review B I-1: v3's `MessageFinished` carried the message's id, so one
    // engine observer can tie each end to its start.
    let ends = Arc::new(Ends::default());
    let mut fails = host("boom", "fail");
    fails["condition"] = json!({"var": "data.fail"});
    fails["on_failure"] = json!("continue");
    let e = Engine::builder()
        .register("logger", Logger)
        .register("fail", Failing)
        .config(EngineConfig::default().observer(Arc::clone(&ends) as _))
        .workflow_json(&workflow("w", vec![host("wait", "logger"), fails], &[]))
        .build()
        .unwrap();
    let mut a = message(json!({"fail": true})).with_id("a");
    let mut b = message(json!({})).with_id("b");
    tokio::join!(e.run(&mut a), e.run(&mut b));
    let seen = ends.0.lock().unwrap().clone();
    let kinds: Vec<_> = seen.iter().map(|(kind, ..)| *kind).collect();
    assert_eq!(
        kinds,
        ["started", "started", "finished", "finished"],
        "the runs overlap: {seen:?}"
    );
    let mut finished: Vec<_> = seen
        .iter()
        .filter_map(|(_, id, errors)| errors.map(|n| (id.as_str(), n)))
        .collect();
    finished.sort_unstable();
    assert_eq!(finished, [("a", 1), ("b", 0)]);
}

#[tokio::test]
async fn each_observer_receives_only_its_interest() {
    let tasks_only = Timeline::new(Interest::TASKS);
    let timed_workflows = Timeline::new(Interest::WORKFLOWS | Interest::TIMINGS);
    let e = engine(&[workflow("w", vec![map("a")], &[])], Some(&tasks_only));
    let opts = RunOptions::default().observer(&timed_workflows);
    e.run_with(&mut message(json!({})), &opts).await;

    // Both see the run's events; the first the task's, untimed.
    assert_eq!(
        tasks_only.lines(),
        lines(&[
            "run_started(1)",
            "task_finished(w/a)",
            "run_finished(stopped_early=false)",
        ])
    );
    assert!(
        tasks_only.seen().iter().all(|s| s.elapsed.is_none()),
        "no `TIMINGS`, no elapsed: {:?}",
        tasks_only.seen()
    );
    // The second the workflow's, timed.
    assert_eq!(
        timed_workflows.lines(),
        lines(&[
            "run_started(1)",
            "workflow_started(w)",
            "workflow_finished(w, sweeps=1, halted=false)",
            "run_finished(stopped_early=false)",
        ])
    );
    let timed: Vec<_> = timed_workflows
        .seen()
        .iter()
        .map(|s| s.elapsed.is_some())
        .collect();
    assert_eq!(
        timed,
        [false, false, true, true],
        "the finished events carry it"
    );
    assert_eq!((tasks_only.asked(), timed_workflows.asked()), (1, 1));

    // An observer asking for nothing still sees the run's events; each
    // interest is read once per run.
    let nothing = Timeline::new(Interest::NONE);
    let opts = RunOptions::default().observer(&nothing);
    e.run_with(&mut message(json!({})), &opts).await;
    assert_eq!(
        nothing.lines(),
        lines(&["run_started(1)", "run_finished(stopped_early=false)"])
    );
    assert_eq!((tasks_only.asked(), nothing.asked()), (2, 1));
}

#[tokio::test]
async fn no_clock_is_read_without_an_interest_in_timings() {
    // The clock reads themselves are counted by the recorder's unit test
    // (`run::record`); here, what an observer and the audit trail see.
    let mut fan = host("f", "logger");
    fan["for_each"] = json!({"over": [1, 2], "as": "item"});
    let workflows = [workflow(
        "w",
        vec![map("a"), host("h", "logger"), fan, map("b")],
        &[("loop", json!({"counter": "i", "max": 2}))],
    )];
    for (interest, timed) in [
        (Interest::WORKFLOWS | Interest::TASKS, false),
        (Interest::ALL, true),
        (Interest::TASKS | Interest::TIMINGS, true),
    ] {
        let timeline = Timeline::new(interest);
        let e = engine(&workflows, Some(&timeline));
        let mut m = message(json!({}));
        e.run(&mut m).await;
        let finished: Vec<_> = timeline
            .seen()
            .into_iter()
            .filter(|s| !s.line.contains("_started"))
            .collect();
        assert!(finished.len() > 10, "{finished:?}");
        assert!(
            finished.iter().all(|s| s.elapsed.is_some() == timed),
            "{interest:?}: {finished:?}"
        );
        // The run's one reading stamps every audit entry, timed or not.
        let stamps: Vec<_> = m.audit().iter().map(|a| a.timestamp).collect();
        assert_eq!(stamps.len(), 10, "{interest:?}");
        assert!(stamps.windows(2).all(|w| w[0] == w[1]), "{interest:?}");
    }
}

#[tokio::test]
async fn an_engine_with_no_observer_runs_the_same_workflows() {
    // Observing changes nothing on the message.
    let workflows = [workflow(
        "w",
        vec![map("a"), host("h", "logger"), host("boom", "fail")],
        &[("on_failure", json!("continue"))],
    )];
    let stripped = |m: &Message| {
        let mut text = m.to_json();
        text["context"]["metadata"]["processed_at"] = Value::Null;
        for entry in text["audit_trail"].as_array_mut().unwrap() {
            entry["timestamp"] = Value::Null;
        }
        for record in text["errors"].as_array_mut().unwrap() {
            record["timestamp"] = Value::Null;
        }
        text
    };
    assert_eq!(
        format!("{:?}", EngineConfig::default()),
        "EngineConfig { observer: absent, error_context: None, .. }"
    );
    let mut unobserved = message(json!({}));
    engine(&workflows, None).run(&mut unobserved).await;
    assert_eq!(
        unobserved.context().data(),
        &OwnedDataValue::from(&json!({"a": 1}))
    );

    let timeline = Timeline::new(Interest::ALL);
    let mut observed = message(json!({}));
    let opts = RunOptions::default().observer(&timeline);
    engine(&workflows, Some(&timeline))
        .run_with(&mut observed, &opts)
        .await;
    assert_eq!(stripped(&observed), stripped(&unobserved));
    assert_eq!(timeline.tasks().len(), 6, "both slots heard each call");
}

#[test]
fn interest_is_a_set() {
    let both = Interest::WORKFLOWS | Interest::TASKS;
    assert!(Interest::NONE.is_empty());
    assert!(!Interest::TIMINGS.is_empty());
    assert_eq!(Interest::ALL, both | Interest::TIMINGS);
    assert!(both.contains(Interest::TASKS) && both.contains(Interest::WORKFLOWS));
    assert!(!both.contains(Interest::TIMINGS));
    assert!(!Interest::TASKS.contains(both));
    assert!(Interest::ALL.contains(Interest::WORKFLOWS | Interest::TIMINGS));
    assert!(Interest::NONE.contains(Interest::NONE) && both.contains(Interest::NONE));
    let mut grown = Interest::NONE;
    grown |= Interest::TASKS;
    grown |= Interest::TASKS;
    assert_eq!(grown, Interest::TASKS);
    // `union` composes in a `const` (review B M-3).
    const MINE: Interest = Interest::TASKS.union(Interest::TIMINGS);
    assert_eq!(MINE, Interest::TASKS | Interest::TIMINGS);
    assert_eq!(Interest::WORKFLOWS.union(MINE), Interest::ALL);

    assert_eq!(format!("{:?}", Interest::NONE), "Interest(NONE)");
    assert_eq!(format!("{both:?}"), "Interest(WORKFLOWS | TASKS)");
    assert_eq!(
        format!("{:?}", Interest::ALL),
        "Interest(WORKFLOWS | TASKS | TIMINGS)"
    );
    assert_eq!(
        format!("{:?}", Interest::TASKS | Interest::TIMINGS),
        "Interest(TASKS | TIMINGS)"
    );

    // An observer that names none asks for both kinds of event, untimed.
    struct Quiet;
    impl Observer for Quiet {
        fn on(&self, _event: &Event<'_>) {}
    }
    assert_eq!(Quiet.interest(), both);
}
