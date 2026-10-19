// The trace end to end, through `Engine::builder()` and `run_with`: the
// initial context after the stamps, then one step per recorded
// call, skipped step and engine write, each carrying its changes, so a
// replay from `initial` rebuilds the context after every step. The rules
// covered: redaction, the byte budget, timings, the failing step, and the
// engine's writes kept out of the audit.
//
// Four host functions drive the calls: `put` writes `writes` in key order
// through `Ctx::set`, then returns what `outcome` names; `peek` keeps the
// whole context as it sees it; `slow` sleeps on tokio's clock; `note`
// records an error of its own, then fails. `wrap` wraps its fan-out element
// for the audit-bytes test.
//
// Two tests hold a whole message's normalised wire text to a golden file
// under `tests/golden/trace/`; `DATAFLOW_GOLDEN_WRITE=1 cargo test --test
// trace` rewrites them.

use crate::support::{Put, golden, map, message, with, workflow};
use dataflow_corpus::json::Json;
use dataflow_corpus::{normalise, replay};
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::observe::{Event, Interest, Observer, StepResult, Trace, TraceOptions, TraceStep};
use dataflow_rs::run::{AuditMode, ErrorRecord, Instant, RunStatus, Selection};
use dataflow_rs::{
    Ctx, Engine, Error, Expr, Function, Message, Outcome, Path, Result, Run, RunOptions,
};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Keeps the whole context, its three roots, as each call sees it.
#[derive(Clone, Default)]
struct Peek(Arc<Mutex<Vec<OwnedDataValue>>>);

impl Function for Peek {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        let mut roots = Vec::new();
        for root in ["data", "metadata", "temp_data"] {
            let value = ctx
                .get(&root.parse()?)
                .map_or(OwnedDataValue::Null, |v| v.into_owned());
            roots.push((root.to_owned(), value));
        }
        self.0.lock().unwrap().push(OwnedDataValue::Object(roots));
        Ok(Outcome::Ok)
    }
}

/// `slow`'s config.
#[derive(FunctionConfig)]
struct SlowConfig {
    /// How long the call sleeps, in milliseconds.
    ms: Expr<u64>,
}

/// Writes `data.began`, sleeps on tokio's clock, then writes `data.slept`.
struct Slow;

impl Function for Slow {
    type Config = SlowConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &SlowConfig) -> Result<Outcome> {
        let ms = ctx.eval(&cfg.ms)?;
        ctx.set(&"data.began".parse()?, OwnedDataValue::from(&json!(true)))?;
        tokio::time::sleep(Duration::from_millis(ms)).await;
        ctx.set(&"data.slept".parse()?, OwnedDataValue::from(&json!(ms)))?;
        Ok(Outcome::Ok)
    }
}

/// Records a `NOTED` error of its own, then fails with `PARTNER_DOWN`.
struct Note;

impl Function for Note {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        ctx.record_error(ErrorRecord::new("NOTED", "noted"));
        Err(Error::service("PARTNER_DOWN", "down"))
    }
}

/// Writes `temp_data.r = {"v": <temp_data.x>}`: a fan-out element wrapped.
struct Wrap;

impl Function for Wrap {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        let x = ctx
            .get(&"temp_data.x".parse()?)
            .map_or(OwnedDataValue::Null, |v| v.into_owned());
        let wrapped = OwnedDataValue::Object(vec![("v".to_owned(), x)]);
        ctx.set(&"temp_data.r".parse()?, wrapped)?;
        Ok(Outcome::Ok)
    }
}

/// An engine over `workflows` with `put`, `peek` (over `peeked`), `slow`,
/// `note` and `wrap` registered.
fn engine_with(workflows: &[Value], peeked: &Peek) -> Engine {
    crate::support::engine_with(workflows, |b| {
        b.register("put", Put)
            .register("peek", peeked.clone())
            .register("slow", Slow)
            .register("note", Note)
            .register("wrap", Wrap)
    })
}

fn engine(workflows: &[Value]) -> Engine {
    engine_with(workflows, &Peek::default())
}

/// A `put` task writing `writes` and returning `outcome`.
fn put(id: &str, writes: Value, outcome: Value) -> Value {
    json!({"id": id, "function": {"name": "put", "input": {
        "writes": writes, "outcome": outcome}}})
}

/// One run of `e` over `m` under `opts`: the report and its trace.
async fn traced(e: &Engine, m: &mut Message, opts: &RunOptions<'_>) -> (Run, Trace) {
    let run = e.run_with(m, opts).await;
    let trace = run
        .trace()
        .cloned()
        .expect("a traced run returns its trace");
    (run, trace)
}

/// [`traced`] with `trace` the run's only option.
async fn trace_of(e: &Engine, m: &mut Message, trace: TraceOptions) -> (Run, Trace) {
    traced(e, m, &RunOptions::default().trace(trace)).await
}

/// `(workflow_id, task_id, result, status)` of each step.
type Line = (String, Option<String>, StepResult, Option<u16>);

fn lines(trace: &Trace) -> Vec<Line> {
    trace
        .steps
        .iter()
        .map(|s| {
            (
                s.workflow_id.to_string(),
                s.task_id.as_deref().map(str::to_owned),
                s.result,
                s.status,
            )
        })
        .collect()
}

fn line(workflow: &str, task: Option<&str>, result: StepResult, status: Option<u16>) -> Line {
    (workflow.to_owned(), task.map(str::to_owned), result, status)
}

fn text<T: serde::Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).unwrap()
}

/// The paths of a step's changes.
fn paths(step: &TraceStep) -> Vec<&str> {
    step.changes.iter().map(|c| &*c.path).collect()
}

fn dv(v: Value) -> OwnedDataValue {
    OwnedDataValue::from(&v)
}

/// What the corpus's replay rebuilds after the last step, from the trace's
/// wire text: its context's text.
fn corpus_rebuild(trace: &Trace) -> String {
    let wire = Json::from_text(&text(trace)).unwrap();
    let steps = wire.get("steps").and_then(Json::as_array).unwrap();
    let rebuilt = replay::rebuild(wire.get("initial").unwrap(), steps);
    rebuilt
        .last()
        .cloned()
        .flatten()
        .expect("every step carries `changes`")
        .to_text()
}

/// The message's context as the corpus's `Json` writes it.
fn context_text(m: &Message) -> String {
    Json::from_text(&text(m.context())).unwrap().to_text()
}

/// The replay after the last step equals the message, key order included,
/// both through `Trace::contexts` and through the corpus's replay.
fn assert_replays(trace: &Trace, m: &Message) {
    let last = trace.contexts().last().expect("the trace has a step");
    assert_eq!(text(&last), text(m.context()), "Trace::contexts");
    assert_eq!(corpus_rebuild(trace), context_text(m), "replay::rebuild");
}

#[tokio::test]
async fn a_trace_is_the_initial_context_plus_each_steps_changes() {
    let e = engine(&[
        workflow(
            "w1",
            vec![
                map("m", "data.a", json!(1)),
                put(
                    "h",
                    json!({"data.h": {"var": "data.a"}, "temp_data.t": [1, 2]}),
                    json!("ok"),
                ),
                // A `Skip`'s writes stand, so its step carries them.
                put("s", json!({"data.s": 2}), json!("skip")),
            ],
            &[],
        ),
        workflow(
            "w2",
            vec![map("b", "data.b", json!({"var": "data.h"}))],
            &[("priority", json!(1))],
        ),
    ]);
    let mut m = message(json!({"seed": "x"}));
    let (run, trace) = trace_of(&e, &mut m, TraceOptions::default()).await;
    assert!(matches!(run.status(), RunStatus::Completed), "{run:?}");

    // `initial` is the context after the stamps, before any workflow.
    assert_eq!(trace.initial.data(), &dv(json!({"seed": "x"})));
    let meta = trace.initial.metadata();
    assert_eq!(meta["processed_at"], m.context().metadata()["processed_at"]);
    assert_eq!(meta["engine_version"], dv(json!(dataflow_rs::VERSION)));
    assert_eq!(trace.initial.temp_data(), &dv(json!({})));
    assert!(!trace.truncated);

    use StepResult::Executed;
    assert_eq!(
        lines(&trace),
        [
            line("w1", Some("m"), Executed, Some(200)),
            line("w1", Some("h"), Executed, Some(200)),
            line("w1", Some("s"), Executed, None),
            line("w2", Some("b"), Executed, Some(200)),
        ]
    );
    let ix: Vec<_> = trace
        .steps
        .iter()
        .map(|s| (s.workflow.index(), s.task.map(|t| t.index())))
        .collect();
    assert_eq!(ix, [(0, Some(0)), (0, Some(1)), (0, Some(2)), (1, Some(3))]);
    // Each call's changes, then the engine's progress write; a `Skip` makes
    // none.
    assert_eq!(paths(&trace.steps[0]), ["data.a", "metadata.progress"]);
    assert_eq!(
        paths(&trace.steps[1]),
        ["data.h", "temp_data.t", "metadata.progress"]
    );
    assert_eq!(paths(&trace.steps[2]), ["data.s"]);
    assert_eq!(paths(&trace.steps[3]), ["data.b", "metadata.progress"]);
    let progress = &trace.steps[1].changes[2];
    assert_eq!(
        text(&progress.old_value),
        r#"{"workflow_id":"w1","task_id":"m","status_code":200}"#
    );
    assert_eq!(
        text(&progress.new_value),
        r#"{"workflow_id":"w1","task_id":"h","status_code":200}"#
    );

    assert_eq!(trace.contexts().count(), trace.steps.len());
    assert_replays(&trace, &m);
    // The replay after the first step is the initial context plus `data.a`.
    let first = trace.contexts().next().unwrap();
    assert_eq!(first.data(), &dv(json!({"seed": "x", "a": 1})));
}

#[tokio::test]
async fn the_failing_step_is_recorded_with_its_error() {
    // The step whose failure stopped the run is the trace's last.
    let e = engine(&[workflow(
        "w",
        vec![
            map("m", "data.a", json!(1)),
            put("boom", json!({"data.lost": 1}), json!("err")),
        ],
        &[],
    )]);
    let mut m = message(json!({}));
    let (mut run, trace) = trace_of(&e, &mut m, TraceOptions::default()).await;
    assert!(matches!(run.status(), RunStatus::Stopped(_)), "{run:?}");
    // The trace taken out of the report survives `into_result`'s `Err`
    // (review B M-4).
    let taken = run.take_trace().expect("the report holds the trace");
    assert_eq!(text(&taken), text(&trace));
    assert!(run.trace().is_none() && run.take_trace().is_none());
    assert!(run.into_result().is_err());

    let last = trace.steps.last().unwrap();
    assert_eq!(last.task_id.as_deref(), Some("boom"));
    assert_eq!(last.result, StepResult::Failed);
    assert_eq!(last.status, Some(500));
    // The recorder's record, as the message keeps it.
    assert_eq!(
        last.errors,
        m.errors(),
        "the failing step carries its error"
    );
    assert_eq!(last.errors[0].code, "PARTNER_DOWN");
    // An `Err` drops its writes; the progress write stands.
    assert_eq!(paths(last), ["metadata.progress"]);
    assert_replays(&trace, &m);
}

#[tokio::test]
async fn a_builtin_failure_that_stops_the_run_records_its_failed_step() {
    // Two built-ins, the second failing on a source that is not a string:
    // the trace keeps the steps before it and the failing step, with its
    // error.
    let e = engine(&[workflow(
        "wf",
        vec![
            map("step_ok", "data.n", json!(7)),
            json!({"id": "step_boom", "function": {"name": "parse_xml",
                "input": {"source": "data.n", "target": "parsed"}}}),
        ],
        &[],
    )]);
    let mut m = message(json!({}));
    let (run, trace) = trace_of(&e, &mut m, TraceOptions::default()).await;
    assert!(matches!(run.status(), RunStatus::Stopped(_)), "{run:?}");
    use StepResult::{Executed, Failed};
    assert_eq!(
        lines(&trace),
        [
            line("wf", Some("step_ok"), Executed, Some(200)),
            line("wf", Some("step_boom"), Failed, Some(500)),
        ]
    );
    assert_eq!(
        trace.steps[1].errors,
        m.errors(),
        "the failing step carries its error"
    );
    assert_replays(&trace, &m);
}

#[tokio::test]
async fn a_failing_run_is_stamped() {
    // A run that fails is still stamped: the stamps are in `initial` and on
    // the message.
    let e = engine(&[workflow(
        "wf",
        vec![put("step_boom", json!({}), json!("err"))],
        &[],
    )]);
    let mut m = message(json!({}));
    let (run, trace) = trace_of(&e, &mut m, TraceOptions::default()).await;
    assert!(matches!(run.status(), RunStatus::Stopped(_)), "{run:?}");
    let stamped = m.context().metadata();
    assert_eq!(
        trace.initial.metadata()["processed_at"],
        stamped["processed_at"]
    );
    assert!(matches!(stamped["processed_at"], OwnedDataValue::String(_)));
    assert_eq!(
        trace.initial.metadata()["engine_version"],
        dv(json!(dataflow_rs::VERSION))
    );
    assert_replays(&trace, &m);
}

#[tokio::test]
async fn the_channel_is_stamped_in_initial_and_an_unknown_channel_traces_no_step() {
    // The channel is stamped and the steps kept, through a failure; an
    // unknown channel traces nothing.
    let e = engine(&[with(
        workflow(
            "wf_ch",
            vec![
                map("step_ok", "data.a", json!(1)),
                put("step_boom", json!({}), json!("err")),
            ],
            &[],
        ),
        &[("channel", json!("payments"))],
    )]);
    let mut m = message(json!({}));
    let opts = RunOptions::default()
        .channel("payments")
        .trace(TraceOptions::default());
    let (run, trace) = traced(&e, &mut m, &opts).await;
    assert!(matches!(run.status(), RunStatus::Stopped(_)), "{run:?}");
    assert_eq!(trace.initial.metadata()["channel"], dv(json!("payments")));
    use StepResult::{Executed, Failed};
    assert_eq!(
        lines(&trace),
        [
            line("wf_ch", Some("step_ok"), Executed, Some(200)),
            line("wf_ch", Some("step_boom"), Failed, Some(500)),
        ]
    );
    assert_replays(&trace, &m);

    // A channel no workflow is on runs nothing, and traces no step.
    let mut m = message(json!({}));
    let opts = RunOptions::default()
        .channel("nope")
        .trace(TraceOptions::default());
    let (run, trace) = traced(&e, &mut m, &opts).await;
    assert!(matches!(run.status(), RunStatus::Completed), "{run:?}");
    assert!(trace.steps.is_empty(), "{:?}", trace.steps);
    assert_eq!(text(&trace.initial), text(m.context()));
}

#[tokio::test]
async fn a_filter_halt_records_the_halting_step_and_nothing_after() {
    // A filter's halt keeps the whole trace up to the halting step.
    let e = engine(&[workflow(
        "wf",
        vec![
            map("step_ok", "data.a", json!(1)),
            json!({"id": "gate", "function": {"name": "filter",
                "input": {"condition": false}}}),
            map("never", "data.never", json!(1)),
        ],
        &[],
    )]);
    let mut m = message(json!({}));
    let (run, trace) = trace_of(&e, &mut m, TraceOptions::default()).await;
    assert!(matches!(run.status(), RunStatus::Completed), "{run:?}");
    let steps: Vec<_> = trace
        .steps
        .iter()
        .map(|s| (s.task_id.as_deref(), s.result))
        .collect();
    assert_eq!(
        steps,
        [
            (Some("step_ok"), StepResult::Executed),
            (Some("gate"), StepResult::Executed),
        ],
        "the halting task is recorded; the task after it never runs"
    );
    assert_eq!(trace.steps[1].status, m.audit()[1].status.try_into().ok());
    assert!(m.context().data().get("never").is_none());
    assert_replays(&trace, &m);
}

#[tokio::test]
async fn a_failure_stepped_past_is_executed_with_its_error() {
    use StepResult::{Executed, Failed};
    // The task's own policy steps past it: `continue` (in a workflow that
    // fails, and in one that continues) and `halt`. A failure the task's
    // policy sends to the workflow's is the `Failed` step, even when the
    // workflow steps past it.
    let continuing = |id: &str| {
        with(
            put(id, json!({}), json!("err")),
            &[("on_failure", json!("continue"))],
        )
    };
    for (workflow_policy, task, expected) in [
        (None, continuing("t"), Executed),
        (Some("continue"), continuing("t"), Executed),
        (
            None,
            with(
                put("t", json!({}), json!("err")),
                &[("on_failure", json!("halt"))],
            ),
            Executed,
        ),
        (Some("continue"), put("t", json!({}), json!("err")), Failed),
    ] {
        let extra: Vec<(&str, Value)> = workflow_policy
            .map(|p| ("on_failure", json!(p)))
            .into_iter()
            .collect();
        let e = engine(&[workflow(
            "w",
            vec![task.clone(), map("after", "data.after", json!(1))],
            &extra,
        )]);
        let mut m = message(json!({}));
        let (run, trace) = trace_of(&e, &mut m, TraceOptions::default()).await;
        assert!(
            matches!(run.status(), RunStatus::CompletedWithErrors),
            "{task}: {run:?}"
        );
        let step = &trace.steps[0];
        assert_eq!(
            (step.task_id.as_deref(), step.result, step.status),
            (Some("t"), expected, Some(500)),
            "{task} in a workflow {workflow_policy:?}"
        );
        assert_eq!(step.errors, m.errors(), "the step carries its error");
        assert_eq!(step.errors[0].code, "PARTNER_DOWN");
        assert_replays(&trace, &m);
    }
}

#[tokio::test]
async fn skipped_steps_are_recorded_for_tasks_groups_and_workflows() {
    use StepResult::{Executed, Skipped};
    let broken = json!({"/": [1, {"var": "data.zero"}]});
    let e = engine(&[
        workflow(
            "w1",
            vec![
                with(
                    map("t1", "data.t1", json!(1)),
                    &[("condition", json!(false))],
                ),
                json!({"id": "g", "condition": {"==": [{"var": "data.zero"}, 1]}, "tasks": [
                    map("g1", "data.g1", json!(1)),
                    {"id": "inner", "tasks": [map("g2", "data.g2", json!(1))]},
                ]}),
                map("t2", "data.t2", json!(1)),
            ],
            &[],
        ),
        workflow(
            "w2",
            vec![map("never", "data.never", json!(1))],
            &[("priority", json!(1)), ("condition", json!(false))],
        ),
        workflow(
            "w3",
            vec![map("broken", "data.broken", json!(1))],
            &[("priority", json!(2)), ("condition", broken)],
        ),
        workflow(
            "w4",
            vec![map("rolled", "data.rolled", json!(1))],
            &[
                ("priority", json!(3)),
                ("rollout", json!({"bucket_start": 0, "bucket_end": 50})),
            ],
        ),
    ]);
    let mut m = message(json!({"zero": 0}));
    let opts = RunOptions::default()
        .routing_bucket(70)
        .trace(TraceOptions::default());
    let (run, trace) = traced(&e, &mut m, &opts).await;
    assert_eq!(
        lines(&trace),
        [
            line("w1", Some("t1"), Skipped, None),
            line("w1", Some("g1"), Skipped, None),
            line("w1", Some("g2"), Skipped, None),
            line("w1", Some("t2"), Executed, Some(200)),
            line("w2", None, Skipped, None),
            line("w3", None, Skipped, None),
            line("w4", None, Skipped, None),
        ]
    );
    let tasks: Vec<_> = trace
        .steps
        .iter()
        .map(|s| s.task.map(|t| t.index()))
        .collect();
    assert_eq!(
        tasks,
        [Some(0), Some(1), Some(2), Some(3), None, None, None]
    );
    for step in &trace.steps[..3] {
        assert!(
            step.changes.is_empty() && step.errors.is_empty(),
            "{step:?}"
        );
    }
    // A condition that fails to evaluate is a skip with its error.
    let Selection::ConditionFailed(e) = run.workflows().nth(2).unwrap().selection else {
        panic!("w3's condition fails: {run:?}");
    };
    let [error] = &trace.steps[5].errors[..] else {
        panic!("the failure is kept: {:?}", trace.steps[5]);
    };
    assert_eq!(
        (error.code.as_str(), error.message.as_str()),
        (e.code(), e.message())
    );
    assert_eq!(error.workflow_id.as_deref(), Some("w3"));
    assert!(trace.steps[4].errors.is_empty() && trace.steps[6].errors.is_empty());
    assert_replays(&trace, &m);
}

#[tokio::test]
async fn a_loop_records_a_sweep_step_before_each_sweep_and_one_at_its_exit() {
    let peeked = Peek::default();
    let e = engine_with(
        &[workflow(
            "w",
            vec![
                map("t", "data.last", json!({"var": "temp_data.x"})),
                map("acc", "temp_data.acc.seen", json!({"var": "temp_data.i"})),
                json!({"id": "p", "function": {"name": "peek"}}),
            ],
            &[(
                "loop",
                json!({"counter": "i", "max": 2, "over": [10, 20, 30], "as": "x",
                       "scratch": "acc", "setup": [map("s", "data.setup", json!(1))]}),
            )],
        )],
        &peeked,
    );
    let mut m = message(json!({}));
    let (_, trace) = trace_of(&e, &mut m, TraceOptions::default()).await;

    use StepResult::Executed;
    let sweep = |counter: i64| (None, Executed, None, Some(counter));
    let call =
        |task: &'static str, counter: Option<i64>| (Some(task), Executed, Some(200), counter);
    let seen: Vec<_> = trace
        .steps
        .iter()
        .map(|s| (s.task_id.as_deref(), s.result, s.status, s.loop_counter))
        .collect();
    assert_eq!(
        seen,
        [
            // Setup's steps carry no counter.
            call("s", None),
            sweep(0),
            call("t", Some(0)),
            call("acc", Some(0)),
            call("p", Some(0)),
            sweep(1),
            call("t", Some(1)),
            call("acc", Some(1)),
            call("p", Some(1)),
            // The exit: the counter past the bounds, alone.
            sweep(2),
        ]
    );
    let changes = |i: usize| -> Vec<(String, String, String)> {
        trace.steps[i]
            .changes
            .iter()
            .map(|c| (c.path.to_string(), text(&c.old_value), text(&c.new_value)))
            .collect()
    };
    let change =
        |path: &str, old: &str, new: &str| (path.to_owned(), old.to_owned(), new.to_owned());
    assert_eq!(
        changes(1),
        [
            change("temp_data.i", "null", "0"),
            change("temp_data.x", "null", "10"),
            change("temp_data.acc", "null", "{}"),
        ]
    );
    assert_eq!(
        changes(5),
        [
            change("temp_data.i", "0", "1"),
            change("temp_data.x", "10", "20"),
            change("temp_data.acc", r#"{"seen":0}"#, "{}"),
        ]
    );
    assert_eq!(changes(9), [change("temp_data.i", "1", "2")]);

    // The replay rebuilds every sweep's context: what `peek` saw is the
    // context after the step before its own.
    let contexts: Vec<_> = trace.contexts().collect();
    let peeks = peeked.0.lock().unwrap().clone();
    assert_eq!(peeks.len(), 2);
    for (at, seen) in [(3, &peeks[0]), (7, &peeks[1])] {
        assert_eq!(
            contexts[at].as_value(),
            seen,
            "the context before step {}",
            at + 1
        );
    }
    assert_replays(&trace, &m);
}

#[tokio::test]
async fn a_fan_out_records_the_prefill_then_each_element() {
    let e = engine(&[workflow(
        "w",
        vec![with(
            put(
                "f",
                json!({"data.last": {"var": "temp_data.x"},
                       "temp_data.r": {"*": [{"var": "temp_data.x"}, 10]}}),
                json!("ok"),
            ),
            &[(
                "for_each",
                json!({"over": [1, 2, 3], "as": "x", "collect": "temp_data.r", "into": "data.out"}),
            )],
        )],
        &[],
    )]);
    let mut m = message(json!({}));
    let (_, trace) = trace_of(&e, &mut m, TraceOptions::default()).await;

    let elements: Vec<_> = trace
        .steps
        .iter()
        .map(|s| (s.task_id.as_deref(), s.result, s.status, s.element_index))
        .collect();
    let element = |i| (Some("f"), StepResult::Executed, Some(200), Some(i));
    assert_eq!(elements, [element(0), element(1), element(2)]);

    // The pre-fill is the first change of the first element's step, then the
    // element's writes, its `into` slot, and the progress write.
    let first = &trace.steps[0];
    assert_eq!(
        paths(first),
        [
            "data.out",
            "data.last",
            "temp_data.r",
            "data.out.0",
            "metadata.progress"
        ]
    );
    assert_eq!(first.changes[0].old_value, OwnedDataValue::Null);
    assert_eq!(text(&first.changes[0].new_value), "[null,null,null]");
    for (i, step) in trace.steps.iter().enumerate().skip(1) {
        let slot = format!("data.out.{i}");
        assert_eq!(
            paths(step),
            [
                "data.last",
                "temp_data.r",
                slot.as_str(),
                "metadata.progress"
            ]
        );
    }
    // The pre-fill is the engine's: the audit entry has the slot alone.
    let audited: Vec<_> = m.audit()[0].changes.iter().map(|c| &*c.path).collect();
    assert_eq!(audited, ["data.last", "temp_data.r", "data.out.0"]);
    assert_replays(&trace, &m);
    assert_eq!(m.context().data()["out"], dv(json!([10, 20, 30])));
}

#[tokio::test]
async fn the_trace_records_changes_when_capture_is_off_and_the_audit_keeps_nothing() {
    let workflows = [workflow(
        "w",
        vec![
            map("m", "data.a", json!(1)),
            put("h", json!({"data.h": {"var": "data.a"}}), json!("ok")),
        ],
        &[],
    )];
    let e = engine(&workflows);
    let full = TraceOptions::default();

    // Capture off: the audit's entries carry no changes; the trace does.
    let mut m = message(json!({}));
    let opts = RunOptions::default()
        .capture_changes(false)
        .trace(full.clone());
    let (_, trace) = traced(&e, &mut m, &opts).await;
    assert_eq!(m.audit().len(), 2);
    assert!(
        m.audit().iter().all(|a| a.changes.is_empty()),
        "{:?}",
        m.audit()
    );
    assert_eq!(paths(&trace.steps[0]), ["data.a", "metadata.progress"]);
    assert_eq!(paths(&trace.steps[1]), ["data.h", "metadata.progress"]);
    assert_replays(&trace, &m);

    // An audit that keeps nothing.
    let mut m = message(json!({}));
    let opts = RunOptions::default().audit(AuditMode::Off).trace(full);
    let (_, trace) = traced(&e, &mut m, &opts).await;
    assert!(m.audit().is_empty());
    assert_eq!(paths(&trace.steps[1]), ["data.h", "metadata.progress"]);
    assert_replays(&trace, &m);

    // A trace without changes: statuses for profiling, and a replay that
    // stays at `initial`.
    let mut m = message(json!({}));
    let (_, trace) = trace_of(&e, &mut m, TraceOptions::default().changes(false)).await;
    assert_eq!(trace.steps.len(), 2);
    assert!(trace.steps.iter().all(|s| s.changes.is_empty()));
    assert!(trace.steps.iter().all(|s| s.status == Some(200)));
    assert!(!trace.truncated);
    for context in trace.contexts() {
        assert_eq!(text(&context), text(&trace.initial));
    }
    // The audit is as without a trace.
    assert_eq!(paths_of(&m), [vec!["data.a"], vec!["data.h"]]);
}

/// The paths of each audit entry's changes.
fn paths_of(m: &Message) -> Vec<Vec<&str>> {
    m.audit()
        .iter()
        .map(|a| a.changes.iter().map(|c| &*c.path).collect())
        .collect()
}

/// The message's wire text, normalised as the corpus normalises it: the
/// run's stamps and the audit and error timestamps blanked.
fn normalised(m: &Message) -> Json {
    let mut json = Json::from_text(&text(m)).expect("a message parses");
    normalise::message(&mut json, None);
    json
}

/// `e` run over a message whose `data` is `seed`, under `opts` without a
/// trace and then with one: the message's normalised wire text, which the
/// trace leaves byte for byte as it was. The traced run replays to its
/// message.
async fn wire(e: &Engine, seed: &Value, opts: &RunOptions<'_>) -> Json {
    let mut plain = message(seed.clone());
    let report = e.run_with(&mut plain, opts).await;
    assert!(
        !matches!(report.status(), RunStatus::Stopped(_)),
        "{report:?}"
    );
    let mut m = message(seed.clone());
    let opts = opts.clone().trace(TraceOptions::default());
    let (report, trace) = traced(e, &mut m, &opts).await;
    assert!(
        !matches!(report.status(), RunStatus::Stopped(_)),
        "{report:?}"
    );
    assert_replays(&trace, &m);
    let untraced = normalised(&plain);
    assert_eq!(
        normalised(&m).to_text(),
        untraced.to_text(),
        "a trace changes nothing in the message"
    );
    untraced
}

/// `runs`, each `(name, normalised message)`, held to the golden file
/// `tests/golden/trace/<name>.json`, or written there under
/// `DATAFLOW_GOLDEN_WRITE=1`.
fn golden(name: &str, runs: Vec<(&str, Json)>) {
    let mut actual = Json::object();
    for (run, message) in runs {
        actual.insert(run, message);
    }
    golden::hold(
        &golden::file(golden::module_dir(module_path!()), name),
        &(actual.to_pretty() + "\n"),
    );
}

#[tokio::test]
async fn the_engines_writes_never_enter_an_audit_entry() {
    // The progress write, a loop's counter, `as` and `scratch`, and a
    // fan-out's `into` pre-fill are all in the trace; none of them is in an
    // audit entry, whose bytes are the same with a trace or without one.
    let workflows = [
        workflow(
            "w",
            vec![
                map("m", "data.last", json!({"var": "temp_data.item.k"})),
                json!({"id": "f", "function": {"name": "wrap", "input": {}},
                       "for_each": {"over": [1, 2], "as": "x", "collect": "temp_data.r",
                                    "into": "data.out"}}),
            ],
            &[(
                "loop",
                json!({"counter": "i", "max": 2, "over": [{"k": 1}, {"k": 2}], "as": "item",
                       "scratch": "acc"}),
            )],
        ),
        workflow(
            "w2",
            vec![map("after", "data.after", json!(1))],
            &[("priority", json!(1))],
        ),
    ];
    let e = engine(&workflows);
    let engines = [
        "metadata.progress",
        "temp_data.i",
        "temp_data.item",
        "temp_data.acc",
        "data.out",
    ];
    let mut runs = Vec::new();
    for (name, opts) in [
        ("default", RunOptions::default()),
        ("capture_off", RunOptions::default().capture_changes(false)),
        (
            "audit_last_3",
            RunOptions::default().audit(AuditMode::Last(3)),
        ),
    ] {
        let mut m = message(json!({}));
        e.run_with(&mut m, &opts).await;
        for entry in m.audit() {
            for change in &entry.changes {
                assert!(
                    !engines.contains(&&*change.path),
                    "{name}: the engine's write {} is audited",
                    change.path
                );
            }
        }
        runs.push((name, wire(&e, &json!({}), &opts).await));
    }

    // And without a host function.
    let plain = engine(&[workflow(
        "p",
        vec![map("m", "data.n", json!({"var": "temp_data.i"}))],
        &[("loop", json!({"counter": "i", "max": 3}))],
    )]);
    let mut m = message(json!({}));
    plain.run(&mut m).await;
    assert_eq!(
        paths_of(&m),
        [vec!["data.n"], vec!["data.n"], vec!["data.n"]]
    );
    runs.push((
        "plain_loop",
        wire(&plain, &json!({}), &RunOptions::default()).await,
    ));
    golden("engines_writes", runs);
}

#[tokio::test]
async fn redaction_nulls_the_path_in_initial_and_in_every_change_but_not_the_message() {
    let redact: Vec<Path> = [
        // A leaf.
        "data.secret",
        // A subtree, written below.
        "data.card",
        // Inside a host's write of its parent.
        "data.user.password",
        // Inside `map`'s merge into the root.
        "data.token",
        // One that resolves nothing, then is written below.
        "data.missing.deep",
        // Removed, then written below.
        "data.vault",
    ]
    .iter()
    .map(|p| p.parse().unwrap())
    .collect();
    let e = engine(&[workflow(
        "w",
        vec![
            map("leaf", "data.secret", json!("s3cret")),
            put("card", json!({"data.card.number": "5555"}), json!("ok")),
            put(
                "user",
                json!({"data.user": {"name": "m", "password": "pw0rd"}}),
                json!("ok"),
            ),
            map("merge", "data", json!({"token": "tok3n", "x": 1})),
            map("copy", "data.copied", json!({"var": "data.secret"})),
            // Three shapes, each written below a redact path: its
            // parent absent from `initial`, present, and removed first.
            put(
                "deep",
                json!({"data.missing.deep.hidden_a": "x1"}),
                json!("ok"),
            ),
            put("below", json!({"data.card.hidden_b": "x2"}), json!("ok")),
            json!({"id": "unset", "function": {"name": "map", "input": {"mappings": [
                {"path": "data.vault", "unset": true}]}}}),
            put("vault", json!({"data.vault.hidden_c": "x3"}), json!("ok")),
        ],
        &[],
    )]);
    let mut m = message(json!({
        "card": {"number": "4111", "exp": "12/30"},
        "secret": "s0",
        "user": {"name": "n"},
        "vault": {"key": "v0"},
    }));
    let (_, trace) = trace_of(&e, &mut m, TraceOptions::default().redact(redact)).await;

    // `initial`: each redacted path that resolves reads `null`; one that does
    // not creates nothing. (The seed spells its keys in the order read here.)
    assert_eq!(
        text(trace.initial.data()),
        r#"{"card":null,"secret":null,"user":{"name":"n"},"vault":null}"#
    );

    // No change carries a redacted value, `copy`'s excepted: a value derived
    // from a redacted path is not redacted.
    let wire = text(&trace.steps[..4]);
    for secret in ["s0", "s3cret", "4111", "12/30", "5555", "pw0rd", "tok3n"] {
        assert!(!wire.contains(secret), "{secret} in {wire}");
    }
    assert_eq!(
        text(&trace.steps[2].changes[0]),
        r#"{"path":"data.user","old_value":{"name":"n"},"new_value":{"name":"m","password":null}}"#
    );
    let merge = &trace.steps[3].changes[0];
    assert_eq!(&*merge.path, "data");
    assert_eq!(text(&merge.new_value), r#"{"token":null,"x":1}"#);
    assert_eq!(
        text(&merge.old_value),
        r#"{"card":null,"secret":null,"user":{"name":"m","password":null},"vault":null}"#
    );
    // A write below a redact path, or a removal at one, is a write of `null`
    // at the redact path, so no key below it reaches the trace.
    for (step, path) in [
        (5, "data.missing.deep"),
        (6, "data.card"),
        (7, "data.vault"),
        (8, "data.vault"),
    ] {
        assert_eq!(
            text(&trace.steps[step].changes[0]),
            format!(r#"{{"path":"{path}","old_value":null,"new_value":null}}"#),
            "step {step}"
        );
    }
    let wire = text(&trace);
    for hidden in ["hidden_a", "hidden_b", "hidden_c", "x1", "x2", "x3", "v0"] {
        assert!(!wire.contains(hidden), "{hidden} in {wire}");
    }

    // Every replayed context reads `null` at each redacted path.
    for (i, context) in trace.contexts().enumerate() {
        let data = context.data();
        assert_eq!(data["secret"], OwnedDataValue::Null, "step {i}");
        assert_eq!(data["card"], OwnedDataValue::Null, "step {i}");
        assert_eq!(data["vault"], OwnedDataValue::Null, "step {i}");
        if i < 5 {
            assert!(data.get("missing").is_none(), "step {i}");
        } else {
            assert_eq!(data["missing"]["deep"], OwnedDataValue::Null, "step {i}");
        }
        if i >= 2 {
            assert_eq!(data["user"]["password"], OwnedDataValue::Null, "step {i}");
        }
        if i >= 3 {
            assert_eq!(data["token"], OwnedDataValue::Null, "step {i}");
        }
    }

    // The message is untouched, and a later task read the real value.
    let data = m.context().data();
    assert_eq!(data["secret"], dv(json!("s3cret")));
    assert_eq!(data["copied"], dv(json!("s3cret")));
    assert_eq!(data["card"]["number"], dv(json!("5555")));
    assert_eq!(data["user"]["password"], dv(json!("pw0rd")));
    assert_eq!(data["token"], dv(json!("tok3n")));
    assert_eq!(data["missing"], dv(json!({"deep": {"hidden_a": "x1"}})));
    assert_eq!(data["card"]["hidden_b"], dv(json!("x2")));
    assert_eq!(data["vault"], dv(json!({"hidden_c": "x3"})));
}

#[tokio::test]
async fn a_byte_budget_truncates_from_the_step_that_crosses_it_and_stays_truncated() {
    let big = "x".repeat(2000);
    let e = engine(&[workflow(
        "w",
        vec![
            map("a", "data.a", json!(big)),
            map("b", "data.b", json!(big)),
            map("c", "data.c", json!(big)),
            // Small enough to fit what is left: it is truncated all the same.
            map("d", "data.d", json!(1)),
        ],
        &[],
    )]);
    let kept =
        |trace: &Trace| -> Vec<usize> { trace.steps.iter().map(|s| s.changes.len()).collect() };

    let (_, unbounded) = trace_of(&e, &mut message(json!({})), TraceOptions::default()).await;
    assert!(!unbounded.truncated);
    assert_eq!(kept(&unbounded), [2, 2, 2, 2]);

    // About 150 bytes of `initial`, then about 2,170 per big step: two fit
    // under 5,500, and the third crosses it.
    let mut m = message(json!({}));
    let (_, trace) = trace_of(&e, &mut m, TraceOptions::default().max_bytes(5500)).await;
    assert!(trace.truncated);
    assert_eq!(kept(&trace), [2, 2, 0, 0]);
    // A truncated step keeps everything but its changes.
    let statuses: Vec<_> = trace
        .steps
        .iter()
        .map(|s| (s.task_id.as_deref(), s.status))
        .collect();
    assert_eq!(
        statuses,
        [
            (Some("a"), Some(200)),
            (Some("b"), Some(200)),
            (Some("c"), Some(200)),
            (Some("d"), Some(200))
        ]
    );
    // The replay is whole up to the step that crossed.
    let second = trace.contexts().nth(1).unwrap();
    assert_eq!(second.data()["b"], dv(json!(big)));

    // A budget below `initial` truncates from the start.
    let (_, trace) = trace_of(
        &e,
        &mut message(json!({})),
        TraceOptions::default().max_bytes(10),
    )
    .await;
    assert!(trace.truncated);
    assert_eq!(kept(&trace), [0, 0, 0, 0]);
    assert_eq!(
        trace.initial.metadata()["engine_version"],
        dv(json!(dataflow_rs::VERSION))
    );
}

/// An observer asking for `TIMINGS`, keeping nothing.
struct Timed;

impl Observer for Timed {
    fn interest(&self) -> Interest {
        Interest::TASKS | Interest::TIMINGS
    }

    fn on(&self, _event: &Event<'_>) {}
}

#[tokio::test]
async fn timings_are_read_only_when_asked() {
    let e = engine(&[workflow(
        "w",
        vec![
            map("m", "data.a", json!(1)),
            put("h", json!({}), json!("ok")),
            with(
                map("never", "data.n", json!(1)),
                &[("condition", json!(false))],
            ),
        ],
        &[("loop", json!({"counter": "i", "max": 1}))],
    )]);
    let timed =
        |trace: &Trace| -> Vec<bool> { trace.steps.iter().map(|s| s.elapsed.is_some()).collect() };

    let (_, trace) = trace_of(&e, &mut message(json!({})), TraceOptions::default()).await;
    assert_eq!(timed(&trace), [false; 5]);
    assert!(!text(&trace).contains("elapsed_us"));

    // An observer's `TIMINGS` times the calls for it, not for the trace.
    let observer = Timed;
    let opts = RunOptions::default()
        .observer(&observer)
        .trace(TraceOptions::default());
    let (_, trace) = traced(&e, &mut message(json!({})), &opts).await;
    assert_eq!(timed(&trace), [false; 5]);

    // The sweep entered, the two calls, the skipped task, the exit: only the
    // calls are timed.
    let (_, trace) = trace_of(
        &e,
        &mut message(json!({})),
        TraceOptions::default().timings(true),
    )
    .await;
    assert_eq!(timed(&trace), [false, true, true, false, false]);
    let wire = Json::from_text(&text(&trace.steps[1])).unwrap();
    assert!(
        wire.get("elapsed_us")
            .is_some_and(|v| matches!(v, Json::Number(_))),
        "{wire:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_run_cut_short_returns_its_trace() {
    let e = engine(&[workflow(
        "w",
        vec![
            map("m", "data.a", json!(1)),
            json!({"id": "t", "function": {"name": "slow", "input": {"ms": 10_000}}}),
            map("never", "data.never", json!(1)),
        ],
        &[],
    )]);
    let deadline: Instant = (tokio::time::Instant::now() + Duration::from_millis(1000)).into_std();
    let mut m = message(json!({}));
    let opts = RunOptions::default()
        .deadline(deadline)
        .trace(TraceOptions::default());
    let (run, trace) = traced(&e, &mut m, &opts).await;
    assert!(matches!(run.status(), RunStatus::TimedOut), "{run:?}");

    assert_eq!(trace.steps.len(), 2);
    let cut = &trace.steps[1];
    assert_eq!(
        (cut.task_id.as_deref(), cut.result, cut.status),
        (Some("t"), StepResult::Failed, Some(500))
    );
    assert_eq!(cut.errors, m.errors(), "the cut call carries its error");
    assert_eq!(cut.errors[0].code, "TIMEOUT_ERROR");
    // The cut call's writes are dropped.
    assert_eq!(paths(cut), ["metadata.progress"]);
    assert_replays(&trace, &m);
}

/// Cancels the run's token, then completes.
struct Cancel;

impl Function for Cancel {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        ctx.cancel().expect("the run has a token").cancel();
        Ok(Outcome::Ok)
    }
}

#[tokio::test]
async fn a_run_stopped_as_a_sweep_starts_records_the_counter_it_wrote() {
    // The next sweep's counter is written before the limits are
    // checked, so the stop's engine step carries that write and the trace
    // replays to the message. The token is cancelled from inside the call,
    // which completes in the same poll: no clock.
    let e = Engine::builder()
        .register("cancel", Cancel)
        .workflow_json(&workflow(
            "w",
            vec![json!({"id": "c", "function": {"name": "cancel"}})],
            &[("loop", json!({"counter": "i", "max": 3}))],
        ))
        .build()
        .unwrap();
    let token = dataflow_rs::run::CancelToken::new();
    let mut m = message(json!({}));
    let opts = RunOptions::default()
        .cancel(&token)
        .trace(TraceOptions::default());
    let (run, trace) = traced(&e, &mut m, &opts).await;
    assert!(matches!(run.status(), RunStatus::Cancelled), "{run:?}");
    assert_eq!(m.context().temp_data()["i"], dv(json!(1)));

    use StepResult::Executed;
    assert_eq!(
        lines(&trace),
        [
            line("w", None, Executed, None),
            line("w", Some("c"), Executed, Some(200)),
            line("w", None, Executed, None),
        ]
    );
    let stop = &trace.steps[2];
    assert_eq!(stop.loop_counter, Some(1));
    assert_eq!(
        text(&stop.changes),
        r#"[{"path":"temp_data.i","old_value":0,"new_value":1}]"#
    );
    assert_replays(&trace, &m);
}

#[tokio::test]
async fn a_root_write_from_a_host_says_replaced_and_map_at_a_root_does_not() {
    let e = engine(&[workflow(
        "w",
        vec![
            map("merge", "data", json!({"m": 1})),
            put("replace", json!({"data": {"h": 2}}), json!("ok")),
        ],
        &[],
    )]);
    let mut m = message(json!({"keep": 0}));
    let (_, trace) = trace_of(&e, &mut m, TraceOptions::default()).await;

    let merge = &trace.steps[0].changes[0];
    assert_eq!((&*merge.path, merge.replaced), ("data", false));
    assert!(!text(merge).contains("replaced"), "{}", text(merge));
    let replace = &trace.steps[1].changes[0];
    assert_eq!((&*replace.path, replace.replaced), ("data", true));
    assert_eq!(
        text(replace),
        r#"{"path":"data","old_value":{"keep":0,"m":1},"new_value":{"h":2},"replaced":true}"#
    );

    let contexts: Vec<_> = trace.contexts().collect();
    assert_eq!(text(contexts[0].data()), r#"{"keep":0,"m":1}"#);
    assert_eq!(text(contexts[1].data()), r#"{"h":2}"#);
    assert_eq!(text(m.context().data()), r#"{"h":2}"#);
    assert_replays(&trace, &m);

    // The audit entry's copy has no `replaced`: it is the trace's alone
    // (review B I-2), with a trace and without one.
    let audited = r#"[{"path":"data","old_value":{"keep":0,"m":1},"new_value":{"h":2}}]"#;
    assert_eq!(text(&m.audit()[1].changes), audited);
    let mut plain = message(json!({"keep": 0}));
    e.run(&mut plain).await;
    assert_eq!(text(&plain.audit()[1].changes), audited);
}

#[tokio::test]
async fn an_append_that_creates_its_target_gives_the_trace_the_array_and_the_audit_its_elements() {
    // Review A I-1: an append or extend onto a missing or `null` target
    // makes it an array. The audit entry records the elements alone; a
    // replay of those alone writes nothing through a `null`, and an
    // empty `extend` records none, so the trace records the array's
    // creation first, `null` to `[]`, for itself alone.
    let mapping =
        |path: &str, logic: Value, mode: &str| json!({"path": path, "logic": logic, "mode": mode});
    let workflows = [workflow(
        "w",
        vec![
            json!({"id": "m", "function": {"name": "map", "input": {"mappings": [
                mapping("data.list", json!(1), "append"),
                mapping("data.ext", json!([1, 2]), "extend"),
                mapping("data.empty", json!([]), "extend"),
                mapping("data.nulled", json!([]), "extend"),
                // A later mapping of the same call, over the array it created.
                {"path": "data.list.1", "logic": 5},
            ]}}}),
        ],
        &[],
    )];
    // Keys in sorted order, so the golden file holds with serde_json's
    // `preserve_order` and without it.
    let seed = json!({"ext": null, "list": null, "nulled": null});
    let mut m = message(seed.clone());
    let (_, trace) = trace_of(&engine(&workflows), &mut m, TraceOptions::default()).await;
    let written = m.context().data();
    assert_eq!(written["list"], dv(json!([1, 5])));
    assert_eq!(written["ext"], dv(json!([1, 2])));
    assert_eq!(written["empty"], dv(json!([])));
    assert_eq!(written["nulled"], dv(json!([])));
    assert_eq!(
        paths(&trace.steps[0]),
        [
            "data.list",
            "data.list.0",
            "data.ext",
            "data.ext.0",
            "data.ext.1",
            "data.empty",
            "data.nulled",
            "data.list.1",
            "metadata.progress",
        ]
    );
    assert_eq!(
        text(&trace.steps[0].changes[0]),
        r#"{"path":"data.list","old_value":null,"new_value":[]}"#
    );
    assert_replays(&trace, &m);
    // The audit entry holds the elements alone, as without a trace.
    assert_eq!(
        paths_of(&m),
        [vec![
            "data.list.0",
            "data.ext.0",
            "data.ext.1",
            "data.list.1"
        ]]
    );

    // The audit entry's bytes: each element at its index, old `null`.
    assert_eq!(
        text(&m.audit()[0].changes),
        concat!(
            r#"[{"path":"data.list.0","old_value":null,"new_value":1},"#,
            r#"{"path":"data.ext.0","old_value":null,"new_value":1},"#,
            r#"{"path":"data.ext.1","old_value":null,"new_value":2},"#,
            r#"{"path":"data.list.1","old_value":null,"new_value":5}]"#,
        )
    );
    // The whole message, the same with a trace and without one.
    let e = engine(&workflows);
    golden(
        "append_creates_target",
        vec![("default", wire(&e, &seed, &RunOptions::default()).await)],
    );
}

#[tokio::test]
async fn the_serialized_shape() {
    let e = engine(&[
        workflow("w", vec![map("m", "data.a", json!(1))], &[]),
        workflow(
            "w2",
            vec![map("never", "data.n", json!(1))],
            &[("priority", json!(1)), ("condition", json!(false))],
        ),
        workflow(
            "w3",
            vec![
                with(
                    put("f", json!({}), json!("err")),
                    &[
                        ("on_failure", json!("continue")),
                        ("for_each", json!({"over": [1], "as": "x"})),
                    ],
                ),
                // A write, then an append to a number, which fails the
                // mapping: a 500, stepped past.
                json!({"id": "mf", "on_failure": "continue", "function": {"name": "map",
                    "input": {"mappings": [
                        {"path": "data.z", "logic": 1},
                        {"path": "data.a", "logic": 2, "mode": "append"}]}}}),
            ],
            &[
                ("priority", json!(2)),
                ("loop", json!({"counter": "i", "max": 1})),
            ],
        ),
    ]);
    let (_, trace) = trace_of(&e, &mut message(json!({})), TraceOptions::default()).await;
    assert_eq!(
        text(&trace.steps[0]),
        concat!(
            r#"{"workflow":0,"workflow_id":"w","task":0,"task_id":"m","result":"executed","#,
            r#""status":200,"changes":[{"path":"data.a","old_value":null,"new_value":1},"#,
            r#"{"path":"metadata.progress","old_value":null,"#,
            r#""new_value":{"workflow_id":"w","task_id":"m","status_code":200}}],"#,
            r#""mapping_ends":[1]}"#,
        )
    );
    assert_eq!(
        text(&trace.steps[1]),
        r#"{"workflow":1,"workflow_id":"w2","task":null,"task_id":null,"result":"skipped","status":null,"changes":[]}"#
    );
    // The sweep entered: the engine's step.
    assert_eq!(
        text(&trace.steps[2]),
        concat!(
            r#"{"workflow":2,"workflow_id":"w3","task":null,"task_id":null,"result":"executed","#,
            r#""status":null,"changes":[{"path":"temp_data.i","old_value":null,"new_value":0}],"#,
            r#""loop_counter":0}"#,
        )
    );

    // Every key, in order, on a timed step.
    let (_, trace) = trace_of(
        &e,
        &mut message(json!({})),
        TraceOptions::default().timings(true),
    )
    .await;
    let keys_of = |step: &TraceStep| -> Vec<String> {
        let wire = Json::from_text(&text(step)).unwrap();
        wire.as_object()
            .unwrap()
            .iter()
            .map(|(k, _)| k.clone())
            .collect()
    };
    assert_eq!(
        keys_of(&trace.steps[3]),
        [
            "workflow",
            "workflow_id",
            "task",
            "task_id",
            "result",
            "status",
            "changes",
            "errors",
            "elapsed_us",
            "loop_counter",
            "element_index",
        ]
    );
    let element = Json::from_text(&text(&trace.steps[3])).unwrap();
    assert_eq!(element.get("result"), Some(&Json::str("executed")));
    // A `map` step: each mapping's end, after the changes and before the
    // records.
    assert_eq!(trace.steps[4].task_id.as_deref(), Some("mf"));
    assert_eq!(
        keys_of(&trace.steps[4]),
        [
            "workflow",
            "workflow_id",
            "task",
            "task_id",
            "result",
            "status",
            "changes",
            "mapping_ends",
            "errors",
            "elapsed_us",
            "loop_counter",
        ]
    );
    let mapped = Json::from_text(&text(&trace.steps[4])).unwrap();
    assert_eq!(mapped.get("mapping_ends").unwrap().to_text(), "[1,1]");
    // The failed mapping's record, then the recorder's.
    assert_eq!(
        mapped.pointer("/errors/0/code"),
        Some(&Json::str("MAPPING_ERROR"))
    );
    assert_eq!(
        mapped.pointer("/errors/1/code"),
        Some(&Json::str("TASK_STATUS_ERROR"))
    );

    let whole = Json::from_text(&text(&trace)).unwrap();
    let keys: Vec<_> = whole
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, _)| k.as_str())
        .collect();
    assert_eq!(keys, ["initial", "steps", "truncated"]);
}

/// A `validation` task failing each of `rules` (`{"var": <path>}` over
/// paths that hold nothing), under `on_failure: continue`.
fn failing_validation(id: &str, rules: &[&str]) -> Value {
    let rules: Vec<Value> = rules
        .iter()
        .map(|r| json!({"logic": {"var": format!("data.{r}")}, "message": format!("needs {r}")}))
        .collect();
    json!({"id": id, "on_failure": "continue",
        "function": {"name": "validation", "input": {"rules": rules}}})
}

/// Each record's code.
fn codes(records: &[ErrorRecord]) -> Vec<&str> {
    records.iter().map(|r| r.code.as_str()).collect()
}

#[tokio::test]
async fn a_steps_errors_are_every_record_it_added() {
    // Every record the step added to the message, in order: the
    // call's own, then the recorder's.
    let e = engine(&[
        workflow(
            "w",
            vec![
                // Two rules fail: two records of its own, and a 400, for
                // which the recorder adds none.
                failing_validation("v", &["a", "b"]),
                with(
                    json!({"id": "n", "function": {"name": "note"}}),
                    &[("on_failure", json!("continue"))],
                ),
                map("ok", "data.ok", json!(1)),
            ],
            &[],
        ),
        // The same validation fails in each of two sweeps.
        workflow(
            "l",
            vec![failing_validation("lv", &["c"])],
            &[
                ("priority", json!(1)),
                ("loop", json!({"counter": "i", "max": 2})),
            ],
        ),
    ]);
    let mut m = message(json!({}));
    let (run, trace) = trace_of(&e, &mut m, TraceOptions::default()).await;
    assert!(
        matches!(run.status(), RunStatus::CompletedWithErrors),
        "{run:?}"
    );
    let calls: Vec<_> = trace
        .steps
        .iter()
        .filter(|s| s.task_id.is_some())
        .map(|s| {
            (
                s.task_id.as_deref().unwrap(),
                s.loop_counter,
                codes(&s.errors),
            )
        })
        .collect();
    assert_eq!(
        calls,
        [
            ("v", None, vec!["VALIDATION_ERROR", "VALIDATION_ERROR"]),
            ("n", None, vec!["NOTED", "PARTNER_DOWN"]),
            ("ok", None, vec![]),
            ("lv", Some(0), vec!["VALIDATION_ERROR"]),
            ("lv", Some(1), vec!["VALIDATION_ERROR"]),
        ]
    );
    let messages: Vec<_> = trace.steps[0]
        .errors
        .iter()
        .map(|r| r.message.as_str())
        .collect();
    assert_eq!(messages, ["needs a", "needs b"]);
    // Each record is on the step that added it, and on no other: the steps'
    // records, in step order, are the message's, as it keeps them.
    let all: Vec<ErrorRecord> = trace
        .steps
        .iter()
        .flat_map(|s| s.errors.iter().cloned())
        .collect();
    assert_eq!(all, m.errors());
    assert_replays(&trace, &m);
}

#[tokio::test]
async fn a_condition_that_failed_to_evaluate_shows_its_record() {
    // A workflow whose condition fails to evaluate is skipped. Its
    // record reaches no message, so its step shows it: the only record on
    // a step that the message does not keep.
    let e = engine(&[workflow(
        "w",
        vec![map("never", "data.never", json!(1))],
        &[("condition", json!({"/": [1, {"var": "data.zero"}]}))],
    )]);
    let mut m = message(json!({"zero": 0}));
    let (run, trace) = trace_of(&e, &mut m, TraceOptions::default()).await;
    let Selection::ConditionFailed(e) = &run.workflows().next().unwrap().selection else {
        panic!("w's condition fails: {run:?}");
    };
    assert!(m.errors().is_empty(), "{:?}", m.errors());
    assert_eq!(lines(&trace), [line("w", None, StepResult::Skipped, None)]);
    let [record] = &trace.steps[0].errors[..] else {
        panic!("the failure is kept: {:?}", trace.steps[0]);
    };
    assert_eq!(
        (record.code.as_str(), record.message.as_str()),
        (e.code(), e.message())
    );
    assert_eq!(record.workflow_id.as_deref(), Some("w"));
    assert_eq!(record.task_id, None);
    let wire = Json::from_text(&text(&trace.steps[0])).unwrap();
    assert_eq!(wire.pointer("/errors/0/code"), Some(&Json::str(e.code())));
}

#[tokio::test]
async fn a_step_with_no_records_writes_no_errors_key() {
    let e = engine(&[workflow(
        "w",
        vec![
            map("m", "data.a", json!(1)),
            with(
                put("boom", json!({}), json!("err")),
                &[("on_failure", json!("continue"))],
            ),
        ],
        &[],
    )]);
    let (_, trace) = trace_of(&e, &mut message(json!({})), TraceOptions::default()).await;
    let quiet = text(&trace.steps[0]);
    assert!(trace.steps[0].errors.is_empty());
    assert!(!quiet.contains("\"errors\""), "{quiet}");
    let failed = text(&trace.steps[1]);
    assert!(
        failed.contains(r#""errors":[{"code":"PARTNER_DOWN""#),
        "{failed}"
    );
    // Both read back as written: a missing key is no records.
    for wire in [quiet, failed] {
        let back: TraceStep = serde_json::from_str(&wire).unwrap();
        assert_eq!(text(&back), wire);
    }
}

/// `context` with the two stamps that differ between two runs, and
/// `metadata.progress`, which no mapping writes, taken out.
fn unstamped(mut context: Value) -> Value {
    if let Some(metadata) = context.get_mut("metadata").and_then(Value::as_object_mut) {
        for key in ["processed_at", "engine_version", "progress"] {
            metadata.remove(key);
        }
    }
    context
}

#[tokio::test]
async fn mapping_ends_split_a_maps_changes_by_mapping() {
    let workflows = [workflow(
        "w",
        vec![
            json!({"id": "m", "function": {"name": "map", "input": {"mappings": [
                // A plain write: one change.
                {"path": "data.a", "logic": 1},
                // A `null` result, skipped: none.
                {"path": "data.b", "logic": {"var": "data.missing"}},
                // An append creating its array: the trace's `[]`, then the
                // element.
                {"path": "data.list", "logic": 7, "mode": "append"},
                // A root, merged with two keys: one change, at the root.
                {"path": "data", "logic": {"x": 1, "y": 2}},
            ]}}}),
        ],
        &[],
    )];
    let mut m = message(json!({"seed": true}));
    let (_, trace) = trace_of(&engine(&workflows), &mut m, TraceOptions::default()).await;
    let step = &trace.steps[0];
    assert_eq!(step.mapping_ends, [1, 1, 3, 4]);
    assert_eq!(
        paths(step),
        [
            "data.a",
            "data.list",
            "data.list.0",
            "data",
            "metadata.progress"
        ]
    );
    // The changes after the last mapping's end are the engine's.
    assert_eq!(
        paths(step)[step.mapping_ends[3] as usize..],
        ["metadata.progress"]
    );

    // The context before each mapping, and after the last; the `null`
    // result leaves the second mapping's context as the first left it.
    let context = |data: Value| json!({"data": data, "metadata": {}, "temp_data": {}});
    let before = [
        context(json!({"seed": true})),
        context(json!({"seed": true, "a": 1})),
        context(json!({"seed": true, "a": 1})),
        context(json!({"seed": true, "a": 1, "list": [7]})),
    ];
    let after = context(json!({"seed": true, "a": 1, "list": [7], "x": 1, "y": 2}));

    // The context replayed from `initial` through `changes[..end]`.
    let replayed = |end: usize| -> Value {
        let mut prefix = trace.clone();
        prefix.steps.truncate(1);
        prefix.steps[0].changes.truncate(end);
        let context = prefix.contexts().last().expect("one step");
        unstamped(serde_json::from_str(&text(&context)).unwrap())
    };
    let ends: Vec<usize> = step.mapping_ends.iter().map(|&e| e as usize).collect();
    // Before mapping `i` is after mapping `i - 1`: `ends[i - 1]`, with
    // `ends[-1]` 0.
    for (i, expected) in before.iter().enumerate() {
        let end = if i == 0 { 0 } else { ends[i - 1] };
        assert_eq!(&replayed(end), expected, "before mapping {i}");
    }
    assert_eq!(replayed(ends[3]), after, "after the last mapping");
    // `Trace::mapping_contexts` is the context after each mapping.
    let mapped: Vec<Value> = trace
        .mapping_contexts(0)
        .expect("a map step has its ends")
        .iter()
        .map(|c| unstamped(serde_json::from_str(&text(c)).unwrap()))
        .collect();
    assert_eq!(mapped[..3], before[1..]);
    assert_eq!(mapped[3], after);
    assert_replays(&trace, &m);

    // The audit entry holds the mappings' rows, without the trace's `[]`.
    assert_eq!(
        paths_of(&m),
        [vec!["data.a", "data.list.0", "data"]],
        "the audit entry"
    );
}

#[tokio::test]
async fn a_trace_without_changes_has_no_mapping_ends() {
    let big = "x".repeat(2000);
    let mapped = |id: &str, value: &Value| {
        json!({"id": id, "function": {"name": "map", "input": {"mappings": [
            {"path": format!("data.{id}"), "logic": value},
            {"path": format!("data.{id}_2"), "logic": 1},
        ]}}})
    };
    let e = engine(&[workflow(
        "w",
        vec![mapped("a", &json!(1)), mapped("b", &json!(big))],
        &[],
    )]);
    let ends = |trace: &Trace| -> Vec<Vec<u32>> {
        trace.steps.iter().map(|s| s.mapping_ends.clone()).collect()
    };
    let (_, whole) = trace_of(&e, &mut message(json!({})), TraceOptions::default()).await;
    assert_eq!(ends(&whole), [vec![1, 2], vec![1, 2]]);

    // A trace that records no changes: no ends, and no key.
    let (_, profiled) = trace_of(
        &e,
        &mut message(json!({})),
        TraceOptions::default().changes(false),
    )
    .await;
    assert_eq!(ends(&profiled), [Vec::<u32>::new(), vec![]]);
    assert!(!text(&profiled).contains("mapping_ends"));

    // Past the byte budget a step keeps no changes, so no ends either.
    let (_, cut) = trace_of(
        &e,
        &mut message(json!({})),
        TraceOptions::default().max_bytes(1000),
    )
    .await;
    assert!(cut.truncated);
    assert_eq!(ends(&cut), [vec![1, 2], vec![]]);
    assert!(cut.steps[1].changes.is_empty());
}
