// Loops end to end: the counter, its bounds and its sweeps, the condition
// before every sweep, `setup`, `over`, `as` and `scratch`, a halt and a
// failure inside a loop, a non-array `over`, the checkpoints a long loop
// takes, and the run future's `Send`. Each test reads the `Run` the
// engine reports and asserts the facts it is about; most also hold every
// message they run whole, key order included, to their golden file under
// `tests/golden/run_loops/` (`support/run_helpers.rs`;
// `DATAFLOW_GOLDEN_WRITE=1` rewrites them).

use crate::support::run_helpers::{Golden, Ran, Run, build, data, run_on};
use crate::support::{map, message, with};
use dataflow_rs::run::{RunStatus, Selection, WorkflowExit};
use dataflow_rs::{Run as Report, RunOptions};
use serde_json::{Value, json};

/// A `map` task appending `logic` to the array at `path`.
fn append(id: &str, path: &str, logic: Value) -> Value {
    json!({"id": id, "function": {"name": "map", "input": {
        "mappings": [{"path": path, "logic": logic, "mode": "append"}]}}})
}

/// A `map` task that returns 500: an append to `data.text`, a string.
fn fails(id: &str) -> Value {
    append(id, "data.text", json!("x"))
}

fn var(path: &str) -> Value {
    json!({"var": path})
}

/// A workflow `id` at `priority` looping by `config` over `tasks`.
fn looping(id: &str, priority: u32, config: Value, tasks: Vec<Value>) -> Value {
    json!({"id": id, "priority": priority, "loop": config, "tasks": tasks})
}

/// Builds `workflows`, runs each of `runs` over them, and holds every run to
/// test `name`'s golden file.
async fn golden(name: &str, workflows: &[Value], runs: &[Run]) -> Vec<Ran> {
    let mut golden = Golden::new(crate::support::golden::module_dir(module_path!()), name);
    let ran = golden_runs(&mut golden, workflows, runs).await;
    golden.check();
    ran
}

/// [`golden`]'s runs, added to `golden`.
async fn golden_runs(golden: &mut Golden, workflows: &[Value], runs: &[Run]) -> Vec<Ran> {
    let engine = build(workflows).unwrap_or_else(|e| panic!("builds: {e}"));
    let mut ran = Vec::new();
    for run in runs {
        let one = run_on(&engine, run).await;
        golden.push(&one);
        ran.push(one);
    }
    ran
}

/// One workflow the run considered: `(id, selection, sweeps, exit)`.
type Considered = (String, &'static str, u32, Option<WorkflowExit>);

fn considered(report: &Report) -> Vec<Considered> {
    report
        .workflows()
        .map(|w| {
            let selection = match w.selection {
                Selection::Ran => "ran",
                Selection::SkippedByCondition => "condition",
                Selection::ConditionFailed(_) => "condition failed",
                Selection::SkippedByRollout => "rollout",
                _ => "other",
            };
            (w.id.to_owned(), selection, w.sweeps, w.exit)
        })
        .collect()
}

fn ran(id: &str, sweeps: u32, exit: WorkflowExit) -> Considered {
    (id.to_owned(), "ran", sweeps, Some(exit))
}

#[tokio::test]
async fn counter_sequences_follow_init_increment_and_max() {
    let mut g = Golden::new(
        crate::support::golden::module_dir(module_path!()),
        "counter_sequences_follow_init_increment_and_max",
    );
    let mut compared = 0;
    for init in [0, 2] {
        for increment in [1, 3] {
            for max in [1, 5, 7] {
                let workflows = [looping(
                    "w",
                    0,
                    json!({"counter": "i", "init": init, "increment": increment, "max": max}),
                    vec![append("seen", "data.seen", var("temp_data.i"))],
                )];
                if max <= init {
                    // An empty bound is refused.
                    assert!(build(&workflows).is_err(), "refuses {max} <= {init}");
                    continue;
                }
                let reports = golden_runs(&mut g, &workflows, &[data(json!({}))]).await;
                let seen: Vec<u32> = (init..max).step_by(increment as usize).collect();
                assert_eq!(
                    considered(&reports[0].report),
                    [ran("w", seen.len() as u32, WorkflowExit::Completed)],
                    "init {init}, increment {increment}, max {max}"
                );
                // Every counter in bounds, in order; after the loop the slot
                // holds the first one out of bounds.
                let context = &reports[0].json()["context"];
                assert_eq!(context["data"]["seen"], json!(seen));
                assert_eq!(
                    context["temp_data"]["i"],
                    json!(seen.last().unwrap() + increment)
                );
                compared += 1;
            }
        }
    }
    assert_eq!(compared, 10);
    g.check();
}

#[tokio::test]
async fn the_engine_owns_the_counter() {
    // A task overwriting the counter changes nothing the loop does: the next
    // sweep writes the engine's counter again, and after the loop the slot
    // holds the first counter out of bounds.
    let workflows = [looping(
        "w",
        0,
        json!({"counter": "i", "max": 3}),
        vec![
            append("seen", "data.seen", var("temp_data.i")),
            map("clobber", "temp_data.i", json!(100)),
            append("after", "data.after", var("temp_data.i")),
        ],
    )];
    let reports = golden(
        "the_engine_owns_the_counter",
        &workflows,
        &[data(json!({}))],
    )
    .await;
    assert_eq!(
        considered(&reports[0].report),
        [ran("w", 3, WorkflowExit::Completed)]
    );

    let e = build(&workflows).unwrap();
    let mut m = message(json!({}));
    e.run(&mut m).await;
    let context = m.to_json()["context"].clone();
    assert_eq!(context["data"]["seen"], json!([0, 1, 2]));
    assert_eq!(context["data"]["after"], json!([100, 100, 100]));
    assert_eq!(context["temp_data"]["i"], json!(3));
}

#[tokio::test]
async fn the_condition_is_rechecked_every_sweep() {
    // Before the first sweep the counter is missing, which reads as 0.
    let below =
        |slot: &str| json!({"<": [{"var": [format!("temp_data.{slot}"), 0]}, var("data.limit")]});
    // True for the first two sweeps, then an evaluation error, which ends the
    // loop as a false does.
    let breaks =
        json!({"if": [{"<": [var("temp_data.j"), 2]}, true, {"/": [1, var("data.zero")]}]});
    let workflows = [
        with(
            looping(
                "counter",
                0,
                json!({"counter": "i", "max": 10}),
                vec![append("t", "data.counter", var("temp_data.i"))],
            ),
            &[("condition", below("i"))],
        ),
        // With `setup`, the condition gates the pre-phase too.
        with(
            looping(
                "setup",
                1,
                json!({"counter": "k", "max": 10,
                       "setup": [append("s", "data.setup", json!("ran"))]}),
                vec![append("t", "data.with_setup", var("temp_data.k"))],
            ),
            &[("condition", below("k"))],
        ),
        with(
            looping(
                "breaks",
                2,
                json!({"counter": "j", "max": 10}),
                vec![append("t", "data.breaks", var("temp_data.j"))],
            ),
            &[("condition", breaks)],
        ),
    ];
    let runs = [
        data(json!({"limit": 0, "zero": 0})),
        data(json!({"limit": 3, "zero": 0})),
        data(json!({"limit": 20, "zero": 0})),
    ];
    let reports = golden("the_condition_is_rechecked_every_sweep", &workflows, &runs).await;
    let skipped = |id: &str| (id.to_owned(), "condition", 0, None);
    let breaks = ran("breaks", 2, WorkflowExit::Completed);
    assert_eq!(
        considered(&reports[0].report),
        [skipped("counter"), skipped("setup"), breaks.clone()]
    );
    assert_eq!(
        considered(&reports[1].report),
        [
            ran("counter", 3, WorkflowExit::Completed),
            ran("setup", 3, WorkflowExit::Completed),
            breaks.clone()
        ]
    );
    assert_eq!(
        considered(&reports[2].report),
        [
            ran("counter", 10, WorkflowExit::Completed),
            ran("setup", 10, WorkflowExit::Completed),
            breaks
        ]
    );

    // A condition that fails at its first evaluation is `ConditionFailed`.
    let e = build(&[with(
        looping(
            "w",
            0,
            json!({"counter": "i", "max": 3}),
            vec![append("t", "data.t", var("temp_data.i"))],
        ),
        &[("condition", json!({"/": [1, var("data.zero")]}))],
    )])
    .unwrap();
    let report = e.run(&mut message(json!({"zero": 0}))).await;
    assert_eq!(
        considered(&report),
        [("w".to_owned(), "condition failed", 0, None)]
    );

    // What each run's sweeps wrote: nothing past the condition's bound, and
    // `breaks` two sweeps whatever the limit.
    let written = |n: usize| {
        let data = reports[n].json()["context"]["data"].clone();
        ["counter", "setup", "with_setup", "breaks"].map(|key| data[key].clone())
    };
    let upto = |n: u32| json!((0..n).collect::<Vec<_>>());
    assert_eq!(
        written(0),
        [Value::Null, Value::Null, Value::Null, json!([0, 1])]
    );
    assert_eq!(
        written(1),
        [upto(3), json!(["ran"]), upto(3), json!([0, 1])]
    );
    assert_eq!(
        written(2),
        [upto(10), json!(["ran"]), upto(10), json!([0, 1])]
    );
}

#[tokio::test]
async fn setup_runs_once_without_a_loop_counter() {
    let workflows = [looping(
        "w",
        0,
        json!({"counter": "i", "max": 3, "setup": [
            append("s1", "data.order", json!("setup")),
            append("s2", "data.order", json!("setup again")),
        ]}),
        vec![append("t", "data.order", var("temp_data.i"))],
    )];
    let reports = golden(
        "setup_runs_once_without_a_loop_counter",
        &workflows,
        &[data(json!({}))],
    )
    .await;
    assert_eq!(
        considered(&reports[0].report),
        [ran("w", 3, WorkflowExit::Completed)]
    );

    let e = build(&workflows).unwrap();
    let mut m = message(json!({}));
    e.run(&mut m).await;
    let counters: Vec<_> = m
        .audit()
        .iter()
        .map(|a| (a.task_id.to_string(), a.loop_counter))
        .collect();
    let at = |task: &str, counter| (task.to_owned(), counter);
    assert_eq!(
        counters,
        [
            at("s1", None),
            at("s2", None),
            at("t", Some(0)),
            at("t", Some(1)),
            at("t", Some(2)),
        ]
    );
}

#[tokio::test]
async fn over_is_evaluated_once_after_setup() {
    // Setup writes the array `over` reads; each sweep grows it, which the
    // loop never sees.
    let workflows = [looping(
        "w",
        0,
        json!({"counter": "i", "max": 10, "over": var("data.items"), "as": "x",
               "setup": [map("load", "data.items", json!([1, 2, 3]))]}),
        vec![
            append("seen", "data.seen", var("temp_data.x")),
            append("grow", "data.items", json!(99)),
        ],
    )];
    let reports = golden(
        "over_is_evaluated_once_after_setup",
        &workflows,
        &[data(json!({"items": "replaced by setup"}))],
    )
    .await;
    assert_eq!(
        considered(&reports[0].report),
        [ran("w", 3, WorkflowExit::Completed)]
    );
    // Three sweeps over setup's array, though each sweep grew it.
    let data = &reports[0].json()["context"]["data"];
    assert_eq!(data["seen"], json!([1, 2, 3]));
    assert_eq!(data["items"], json!([1, 2, 3, 99, 99, 99]));
}

#[tokio::test]
async fn max_bounds_an_over_loop() {
    let items = json!(["a", "b", "c", "d", "e"]);
    let workflows = [
        looping(
            "max",
            0,
            json!({"counter": "i", "max": 2, "over": items, "as": "x"}),
            vec![append("t", "data.max", var("temp_data.x"))],
        ),
        looping(
            "stride",
            1,
            json!({"counter": "j", "init": 1, "increment": 2, "max": 9,
                   "over": var("data.items"), "as": "y"}),
            vec![append("t", "data.stride", var("temp_data.y"))],
        ),
    ];
    let reports = golden(
        "max_bounds_an_over_loop",
        &workflows,
        &[data(json!({"items": items}))],
    )
    .await;
    assert_eq!(
        considered(&reports[0].report),
        [
            ran("max", 2, WorkflowExit::Completed),
            ran("stride", 2, WorkflowExit::Completed)
        ]
    );
    let data = &reports[0].json()["context"]["data"];
    assert_eq!(data["max"], json!(["a", "b"]));
    assert_eq!(data["stride"], json!(["b", "d"]), "elements 1 and 3");
}

#[tokio::test]
async fn an_empty_over_writes_init_and_runs_no_sweep() {
    let workflows = [
        looping(
            "literal",
            0,
            json!({"counter": "i", "init": 2, "max": 5, "over": [], "as": "x", "scratch": "acc"}),
            vec![append("t", "data.literal", var("temp_data.x"))],
        ),
        looping(
            "read",
            1,
            json!({"counter": "j", "max": 5, "over": var("data.items"), "as": "y",
                   "scratch": "acc2"}),
            vec![append("t", "data.read", var("temp_data.y"))],
        ),
    ];
    let reports = golden(
        "an_empty_over_writes_init_and_runs_no_sweep",
        &workflows,
        &[data(json!({"items": []}))],
    )
    .await;
    assert_eq!(
        considered(&reports[0].report),
        [
            ran("literal", 0, WorkflowExit::Completed),
            ran("read", 0, WorkflowExit::Completed)
        ]
    );

    let e = build(&workflows).unwrap();
    let mut m = message(json!({"items": []}));
    e.run(&mut m).await;
    // The counter holds `init`; no binding, no scratch, no task ran.
    assert_eq!(m.to_json()["context"]["temp_data"], json!({"i": 2, "j": 0}));
    assert!(m.audit().is_empty());
}

/// The one record a non-array `over` leaves, as JSON.
fn over_record(workflow: &str, kind: &str, now: &Value) -> Value {
    json!({
        "code": "LOGIC_ERROR",
        "message": format!("Workflow {workflow}: loop.over must evaluate to an array, got {kind}"),
        "path": null,
        "workflow_id": workflow,
        "timestamp": now,
    })
}

#[tokio::test]
async fn a_non_array_over_records_one_logic_error_and_writes_no_counter() {
    // One `LOGIC_ERROR` record, with no wrapper record beside it.
    for continues in [true, false] {
        for (scalar, kind) in [
            (json!("nope"), "a string"),
            (json!(null), "null"),
            (json!(7), "a number"),
            (json!(true), "a boolean"),
            (json!({"a": 1}), "an object"),
        ] {
            let policy = if continues { "continue" } else { "fail" };
            let e = build(&[
                with(
                    looping(
                        "w",
                        0,
                        json!({"counter": "i", "max": 5, "over": var("data.scalar"),
                               "as": "x", "scratch": "acc",
                               "setup": [map("s", "data.setup", json!(true))]}),
                        vec![append("t", "data.seen", var("temp_data.x"))],
                    ),
                    &[("on_failure", json!(policy))],
                ),
                json!({"id": "after", "priority": 1,
                       "tasks": [map("after", "data.after", json!(true))]}),
            ])
            .unwrap();
            let mut m = message(json!({"scalar": scalar}));
            let report = e.run(&mut m).await;
            let wire = m.to_json();
            let now = &wire["context"]["metadata"]["processed_at"];
            let context = &wire["context"];
            assert_eq!(
                serde_json::to_value(m.errors()).unwrap(),
                json!([over_record("w", kind, now)]),
                "{policy}, over {scalar}"
            );
            // Setup ran; no counter, binding or scratch was written.
            assert_eq!(context["data"]["setup"], json!(true));
            assert_eq!(context["temp_data"], json!({}), "{policy}, over {scalar}");
            if continues {
                assert!(matches!(report.status(), RunStatus::CompletedWithErrors));
                assert_eq!(
                    considered(&report),
                    [
                        ran("w", 0, WorkflowExit::Failed),
                        ran("after", 1, WorkflowExit::Completed)
                    ]
                );
                assert_eq!(context["data"]["after"], json!(true));
            } else {
                let RunStatus::Stopped(error) = report.status() else {
                    panic!("fail stops the run: {:?}", report.status());
                };
                assert_eq!(error.code(), "LOGIC_ERROR");
                assert_eq!(
                    error.message(),
                    format!("Workflow w: loop.over must evaluate to an array, got {kind}")
                );
                assert_eq!(considered(&report), [ran("w", 0, WorkflowExit::Stopped)]);
                assert_eq!(context["data"].get("after"), None);
            }
        }
    }
}

#[tokio::test]
async fn an_over_that_fails_to_evaluate_records_the_evaluation_error() {
    // Not that case: one record with the workflow id, the
    // evaluation error's code and its own text, with no workflow wrapper.
    let workflows = [with(
        looping(
            "w",
            0,
            json!({"counter": "i", "max": 5, "over": {"/": [1, var("data.zero")]}, "as": "x"}),
            vec![append("t", "data.seen", var("temp_data.x"))],
        ),
        &[("continue_on_error", json!(true))],
    )];
    let e = build(&workflows).unwrap();
    let mut m = message(json!({"zero": 0}));
    let report = e.run(&mut m).await;
    let [record] = m.errors() else {
        panic!("one record: {:?}", m.errors());
    };
    assert_eq!(record.code, "LOGIC_ERROR");
    assert_eq!(
        dataflow_corpus::normalise::dependency_text(&record.message),
        "Workflow w: loop.over failed to evaluate: Logic evaluation error: Thrown: <dependency message>"
    );
    assert_eq!(record.workflow_id.as_deref(), Some("w"));
    assert_eq!(record.task_id, None);
    assert_eq!(m.to_json()["context"]["temp_data"], json!({}));
    assert_eq!(considered(&report), [ran("w", 0, WorkflowExit::Failed)]);
}

#[tokio::test]
async fn scratch_is_reset_every_sweep() {
    // Each sweep appends to its scratch and snapshots it: a snapshot holds
    // only its own sweep's entry, and the condition sees the reset too.
    let workflows = [with(
        looping(
            "w",
            0,
            json!({"counter": "i", "max": 5, "over": var("data.items"), "as": "x",
                   "scratch": "acc"}),
            vec![
                append("add", "temp_data.acc.list", var("temp_data.x")),
                append("snap", "data.snapshots", var("temp_data.acc")),
            ],
        ),
        &[(
            "condition",
            json!({"==": [{"var": ["temp_data.acc.list", "none"]}, "none"]}),
        )],
    )];
    let reports = golden(
        "scratch_is_reset_every_sweep",
        &workflows,
        &[data(json!({"items": [1, 2, 3]}))],
    )
    .await;
    assert_eq!(
        considered(&reports[0].report),
        [ran("w", 3, WorkflowExit::Completed)]
    );
    assert_eq!(
        reports[0].json()["context"]["data"]["snapshots"],
        json!([{"list": [1]}, {"list": [2]}, {"list": [3]}])
    );
}

#[tokio::test]
async fn a_halt_or_terminal_ends_the_whole_loop() {
    let at = |n: i64| json!({"==": [var("temp_data.i"), n]});
    let filter_halts = json!({"id": "gate", "function": {"name": "filter", "input": {
        "condition": {"<": [var("temp_data.i"), 2]}, "on_reject": "halt"}}});
    let workflows = [
        looping(
            "filter",
            0,
            json!({"counter": "i", "max": 5}),
            vec![append("t", "data.filter", var("temp_data.i")), filter_halts],
        ),
        looping(
            "terminal_task",
            1,
            json!({"counter": "i", "max": 5}),
            vec![
                with(
                    append("stop", "data.terminal_task", json!("stop")),
                    &[("condition", at(1)), ("terminal", json!(true))],
                ),
                append("t", "data.terminal_task", var("temp_data.i")),
            ],
        ),
        looping(
            "terminal_group",
            2,
            json!({"counter": "i", "max": 5}),
            vec![
                json!({"id": "g", "condition": at(3), "terminal": true, "tasks": [
                    append("in_group", "data.terminal_group", json!("group"))]}),
                append("t", "data.terminal_group", var("temp_data.i")),
            ],
        ),
        looping(
            "setup_halts",
            3,
            json!({"counter": "i", "max": 5, "setup": [
                with(append("s", "data.setup_halts", json!("setup")), &[("terminal", json!(true))])
            ]}),
            vec![append("t", "data.setup_halts", var("temp_data.i"))],
        ),
        json!({"id": "after", "priority": 4, "tasks": [map("after", "data.after", json!(true))]}),
    ];
    let reports = golden(
        "a_halt_or_terminal_ends_the_whole_loop",
        &workflows,
        &[data(json!({}))],
    )
    .await;
    assert_eq!(
        considered(&reports[0].report),
        [
            ran("filter", 3, WorkflowExit::Halted),
            ran("terminal_task", 2, WorkflowExit::Halted),
            ran("terminal_group", 4, WorkflowExit::Halted),
            ran("setup_halts", 0, WorkflowExit::Halted),
            ran("after", 1, WorkflowExit::Completed),
        ]
    );
    let data = &reports[0].json()["context"]["data"];
    assert_eq!(data["filter"], json!([0, 1, 2]));
    assert_eq!(data["terminal_task"], json!([0, "stop"]));
    assert_eq!(data["terminal_group"], json!([0, 1, 2, "group"]));
    assert_eq!(data["setup_halts"], json!(["setup"]));
    assert_eq!(data["after"], json!(true));
}

#[tokio::test]
async fn a_failure_under_continue_starts_the_next_sweep() {
    let on_one = json!({"==": [var("temp_data.i"), 1]});
    let body = || {
        vec![
            append("before", "data.order", var("temp_data.i")),
            with(fails("bad"), &[("condition", on_one.clone())]),
            append("after", "data.order", json!("after")),
        ]
    };
    let workflows = [
        with(
            looping("continues", 0, json!({"counter": "i", "max": 3}), body()),
            &[("continue_on_error", json!(true))],
        ),
        // A task's own `continue_on_error` steps past it inside the sweep.
        looping(
            "task_continues",
            1,
            json!({"counter": "i", "max": 3}),
            vec![
                append("before", "data.task", var("temp_data.i")),
                with(
                    fails("bad"),
                    &[
                        ("condition", on_one.clone()),
                        ("continue_on_error", json!(true)),
                    ],
                ),
                append("after", "data.task", json!("after")),
            ],
        ),
        looping("stops", 2, json!({"counter": "i", "max": 3}), body()),
        json!({"id": "never", "priority": 3, "tasks": [map("never", "data.never", json!(true))]}),
    ];
    let reports = golden(
        "a_failure_under_continue_starts_the_next_sweep",
        &workflows,
        &[data(json!({"text": "a string"}))],
    )
    .await;
    assert_eq!(
        considered(&reports[0].report),
        [
            ran("continues", 3, WorkflowExit::Failed),
            ran("task_continues", 3, WorkflowExit::Completed),
            ran("stops", 2, WorkflowExit::Stopped),
        ]
    );
    // `stops` fails the run with the failing task's code.
    assert_eq!(reports[0].code, "TASK_ERROR");
    let data = &reports[0].json()["context"]["data"];
    // `continues` (sweep 1 cut short at `bad`), then `stops` (the same).
    assert_eq!(
        data["order"],
        json!([0, "after", 1, 2, "after", 0, "after", 1])
    );
    assert_eq!(data["task"], json!([0, "after", 1, "after", 2, "after"]));
    assert!(data.get("never").is_none());
}

#[tokio::test]
async fn a_setup_failure_stops_unless_the_workflow_continues() {
    let mut g = Golden::new(
        crate::support::golden::module_dir(module_path!()),
        "a_setup_failure_stops_unless_the_workflow_continues",
    );
    for continues in [true, false] {
        let workflows = [
            with(
                looping(
                    "w",
                    0,
                    json!({"counter": "i", "max": 3, "over": [1, 2, 3], "as": "x",
                           "setup": [append("ok", "data.setup", json!("ok")), fails("bad")]}),
                    vec![append("t", "data.seen", var("temp_data.x"))],
                ),
                &[("continue_on_error", json!(continues))],
            ),
            json!({"id": "after", "priority": 1, "tasks": [map("after", "data.after", json!(true))]}),
        ];
        let reports = golden_runs(&mut g, &workflows, &[data(json!({"text": "a string"}))]).await;
        let expected = if continues {
            vec![
                ran("w", 0, WorkflowExit::Failed),
                ran("after", 1, WorkflowExit::Completed),
            ]
        } else {
            vec![ran("w", 0, WorkflowExit::Stopped)]
        };
        assert_eq!(
            considered(&reports[0].report),
            expected,
            "continues {continues}"
        );
        let data = &reports[0].json()["context"]["data"];
        assert_eq!(data["setup"], json!(["ok"]), "continues {continues}");
        assert!(data.get("seen").is_none(), "continues {continues}");
        assert_eq!(data.get("after").is_some(), continues);
        assert_eq!(reports[0].code == "ok", continues);
    }
    g.check();
}

/// About 200 KiB of JSON in 1,600 records, which no step reads.
fn large_input() -> Value {
    let records: Vec<Value> = (0..1600)
        .map(|i| {
            json!({"id": i, "name": format!("record-{i:05}"), "tags": ["alpha", "beta"],
                   "score": i * 3, "note": "x".repeat(60)})
        })
        .collect();
    let input = Value::Array(records);
    let bytes = input.to_string().len();
    assert!((190 << 10..230 << 10).contains(&bytes), "{bytes} bytes");
    input
}

#[tokio::test]
async fn a_long_loop_checkpoints_and_stays_within_the_arena_bound() {
    const SWEEPS: usize = 1000;
    let e = build(&[looping(
        "w",
        0,
        json!({"counter": "i", "max": SWEEPS}),
        vec![
            json!({"id": "step", "function": {"name": "map", "input": {"mappings": [
                {"path": "data.log", "mode": "append",
                 "logic": {"turn": var("temp_data.i"), "half": {"/": [var("temp_data.i"), 2]}}},
                {"path": "data.last", "logic": var("temp_data.i")},
            ]}}}),
        ],
    )])
    .unwrap();
    let mut m = message(json!({"input": large_input()}));
    let report = e.run(&mut m).await;
    assert!(matches!(report.status(), RunStatus::Completed));
    assert_eq!(
        considered(&report),
        [ran("w", SWEEPS as u32, WorkflowExit::Completed)]
    );
    let data = &m.to_json()["context"]["data"];
    let log = data["log"].as_array().expect("data.log is an array");
    assert_eq!(log.len(), SWEEPS);
    assert_eq!(log[SWEEPS - 1], json!({"turn": SWEEPS - 1, "half": 499.5}));
    assert_eq!(data["last"], json!(SWEEPS - 1));
    assert_eq!(m.audit().len(), SWEEPS);

    let stats = report.arena_stats();
    // The run's finish takes one checkpoint; the garbage rule took the rest.
    assert!(stats.checkpoints > 1, "{stats:?}");
    assert!(stats.views <= stats.checkpoints + 1, "{stats:?}");
    // The bound plus one op's garbage.
    let bound = 2 * stats.live_max.max(64 << 10) + (64 << 10);
    assert!(
        stats.peak_arena <= bound,
        "peak {} over the bound {bound}: {stats:?}",
        stats.peak_arena
    );
}

#[test]
fn the_run_future_is_send() {
    fn assert_send<T: Send>(_: &T) {}
    let e = build(&[looping(
        "w",
        0,
        json!({"counter": "i", "max": 3, "over": var("data.items"), "as": "x",
               "setup": [map("s", "data.s", json!(1))]}),
        vec![append("t", "data.seen", var("temp_data.x"))],
    )])
    .unwrap();
    let mut msg = message(json!({"items": [1, 2, 3]}));
    let opts = RunOptions::default();
    assert_send(&e.run_with(&mut msg, &opts));
}
