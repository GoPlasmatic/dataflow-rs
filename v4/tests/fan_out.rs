extern crate dataflow_rs_v4 as dataflow_rs;

// `for_each` end to end: one task's host function called once per element of
// an array, every call isolated over the run's one view, then folded into the
// message in element order. Every rule v3 can run is held to v3: one twin
// function, `echo`, is registered on both sides, and each message is compared
// as `run_vs_v3.rs` compares it (`support/vs_v3.rs`: the normalised wire text
// and the outcome's code). What v3 cannot run (I91's error, the bound on
// calls in flight, the `Send` pin) is checked against literal expectations.
//
// `echo` reads its element at `temp_data.<bound>` and the index at
// `temp_data.<bound>_index` through `get`, yields `y` times, then makes the
// element's writes (`w`, `[path, value]` pairs), appends (`a`, `[path,
// value]` pairs, each read, pushed onto and written back) and increment
// (`inc`, a path), writes `temp_data.out = {"id": <element.id>, "i":
// <index>}` with the element read again through `get`, and
// `temp_data.by_eval` from its `by_eval` expression. It records `WARNED`
// when the element says `warn`, and returns what `o` names: `"success"` (the
// default), `"skip"`, `"halt"`, `{"status": n}`, or `{"error": {"code": c,
// "message": m}}`, a service error.

#[path = "support/vs_v3.rs"]
mod vs_v3;

use dataflow_rs::function::{BoxFuture, CallLayer, FunctionConfig, Next};
use dataflow_rs::run::{ErrorKind, ErrorRecord, RunStatus, WorkflowExit};
use dataflow_rs::{Ctx, Engine, Error, Expr, Function, Message, Outcome, Result, RunOptions};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use vs_v3::{Run, data, on_v3, on_v4, v3_engine, v3_engine_with, v4_engine, v4_engine_with};

/// What an element names as its call's result.
enum Wanted {
    Success,
    Skip,
    Halt,
    Status(u16),
    Error(String, String),
}

/// What an element asks `echo` to do, read from the element itself.
struct Asks {
    yields: u64,
    writes: Vec<(String, Value)>,
    appends: Vec<(String, Value)>,
    inc: Option<String>,
    warn: bool,
    outcome: Wanted,
}

fn asks(item: &Value) -> Asks {
    let pairs = |key: &str| -> Vec<(String, Value)> {
        item.get(key)
            .and_then(Value::as_array)
            .map(|pairs| {
                pairs
                    .iter()
                    .map(|pair| (pair[0].as_str().unwrap().to_owned(), pair[1].clone()))
                    .collect()
            })
            .unwrap_or_default()
    };
    let outcome = match item.get("o") {
        None => Wanted::Success,
        Some(o) => match o.as_str() {
            Some("success") => Wanted::Success,
            Some("skip") => Wanted::Skip,
            Some("halt") => Wanted::Halt,
            _ if o.get("status").is_some() => {
                Wanted::Status(u16::try_from(o["status"].as_u64().unwrap()).unwrap())
            }
            _ => {
                let e = &o["error"];
                Wanted::Error(
                    e["code"].as_str().unwrap().to_owned(),
                    e["message"].as_str().unwrap().to_owned(),
                )
            }
        },
    };
    Asks {
        yields: item.get("y").and_then(Value::as_u64).unwrap_or(0),
        writes: pairs("w"),
        appends: pairs("a"),
        inc: item.get("inc").and_then(Value::as_str).map(str::to_owned),
        warn: item.get("warn") == Some(&json!(true)),
        outcome,
    }
}

/// v3's `echo`, through `async_trait`.
mod v3_twin {
    use super::{Wanted, asks};
    use async_trait::async_trait;
    use dataflow_rs_v3::{
        AsyncFunctionHandler, DataflowError, ErrorInfo, Result, TaskContext, TaskOutcome, Template,
        TemplateCompiler,
    };
    use datavalue::OwnedDataValue;
    use serde::Deserialize;
    use serde_json::{Value, json};

    #[derive(Deserialize)]
    pub struct EchoInput {
        bound: String,
        by_eval: Template,
    }

    pub struct Echo;

    fn get(ctx: &TaskContext<'_>, path: &str) -> Value {
        ctx.get(path).map_or(Value::Null, Value::from)
    }

    #[async_trait]
    impl AsyncFunctionHandler for Echo {
        type Input = EchoInput;

        fn compile_input(input: &mut EchoInput, c: &TemplateCompiler) -> Result<()> {
            input.by_eval.compile(c, "by_eval")
        }

        async fn execute(
            &self,
            ctx: &mut TaskContext<'_>,
            input: &EchoInput,
        ) -> Result<TaskOutcome> {
            let item_path = format!("temp_data.{}", input.bound);
            let index = get(ctx, &format!("{item_path}_index"));
            assert_eq!(ctx.element_index().map(|i| json!(i)), Some(index.clone()));
            let asks = asks(&get(ctx, &item_path));
            for _ in 0..asks.yields {
                tokio::task::yield_now().await;
            }
            for (path, value) in &asks.writes {
                ctx.set(path, OwnedDataValue::from(value));
            }
            for (path, value) in &asks.appends {
                let mut list = match get(ctx, path) {
                    Value::Array(list) => list,
                    _ => Vec::new(),
                };
                list.push(value.clone());
                ctx.set(path, OwnedDataValue::from(&Value::Array(list)));
            }
            if let Some(path) = &asks.inc {
                let n = get(ctx, path).as_i64().unwrap_or(0);
                ctx.set(path, OwnedDataValue::from(&json!(n + 1)));
            }
            let id = get(ctx, &item_path)["id"].clone();
            ctx.set(
                "temp_data.out",
                OwnedDataValue::from(&json!({"id": id, "i": index})),
            );
            let by_eval = input.by_eval.resolve(ctx)?;
            ctx.set("temp_data.by_eval", by_eval);
            if asks.warn {
                ctx.add_error(ErrorInfo::builder("WARNED", "careful").build());
            }
            match asks.outcome {
                Wanted::Success => Ok(TaskOutcome::Success),
                Wanted::Skip => Ok(TaskOutcome::Skip),
                Wanted::Halt => Ok(TaskOutcome::Halt),
                Wanted::Status(s) => Ok(TaskOutcome::Status(s)),
                Wanted::Error(code, message) => Err(DataflowError::service(code, message).build()),
            }
        }
    }
}

/// v4's `echo`'s config.
#[derive(FunctionConfig)]
struct EchoConfig {
    /// The binding's name: the element is at `temp_data.<bound>`.
    bound: String,
    /// Evaluated after the writes, into `temp_data.by_eval`.
    by_eval: Expr,
}

/// v4's `echo`, which also counts its calls in flight, and lists the
/// elements in the order their calls finished.
#[derive(Default)]
struct Echo {
    in_flight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    finished: Arc<Mutex<Vec<usize>>>,
}

fn get(ctx: &Ctx<'_>, path: &str) -> Result<Value> {
    Ok(ctx
        .get(&path.parse()?)
        .map_or(Value::Null, |v| Value::from(&*v)))
}

fn set(ctx: &mut Ctx<'_>, path: &str, value: &Value) -> Result<()> {
    ctx.set(&path.parse()?, OwnedDataValue::from(value))
}

impl Echo {
    async fn echo(ctx: &mut Ctx<'_>, cfg: &EchoConfig) -> Result<Outcome> {
        let item_path = format!("temp_data.{}", cfg.bound);
        let index = get(ctx, &format!("{item_path}_index"))?;
        assert_eq!(
            ctx.info().element_index.map(|i| json!(i)),
            Some(index.clone()),
            "the call's info and the binding agree"
        );
        let asks = asks(&get(ctx, &item_path)?);
        for _ in 0..asks.yields {
            tokio::task::yield_now().await;
        }
        for (path, value) in &asks.writes {
            set(ctx, path, value)?;
        }
        for (path, value) in &asks.appends {
            let mut list = match get(ctx, path)? {
                Value::Array(list) => list,
                _ => Vec::new(),
            };
            list.push(value.clone());
            set(ctx, path, &Value::Array(list))?;
        }
        if let Some(path) = &asks.inc {
            let n = get(ctx, path)?.as_i64().unwrap_or(0);
            set(ctx, path, &json!(n + 1))?;
        }
        let id = get(ctx, &item_path)?["id"].clone();
        set(ctx, "temp_data.out", &json!({"id": id, "i": index}))?;
        let by_eval = ctx.eval(&cfg.by_eval)?.into_owned();
        ctx.set(&"temp_data.by_eval".parse()?, by_eval)?;
        if asks.warn {
            ctx.record_error(ErrorRecord::new("WARNED", "careful"));
        }
        match asks.outcome {
            Wanted::Success => Ok(Outcome::Ok),
            Wanted::Skip => Ok(Outcome::Skip),
            Wanted::Halt => Ok(Outcome::Halt),
            Wanted::Status(s) => Ok(Outcome::Status(s)),
            Wanted::Error(code, message) => Err(Error::service(code, message)),
        }
    }
}

impl Function for Echo {
    type Config = EchoConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &EchoConfig) -> Result<Outcome> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        let result = Self::echo(ctx, cfg).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        if let Some(index) = ctx.info().element_index {
            self.finished.lock().unwrap().push(index);
        }
        result
    }
}

/// The fan-out task `t`: `echo` with its element bound at `temp_data.p`.
fn fan(for_each: Value, extra: &[(&str, Value)]) -> Value {
    let mut task = json!({"id": "t", "for_each": for_each, "function": {"name": "echo", "input": {
        "bound": "p", "by_eval": {"var": "temp_data.p"}}}});
    for (key, value) in extra {
        task[*key] = value.clone();
    }
    task
}

/// `map` setting `data.<id>_ran`: the workflow got this far.
fn mark(id: &str) -> Value {
    json!({"id": id, "function": {"name": "map", "input": {"mappings": [
        {"path": format!("data.{id}_ran"), "logic": true}]}}})
}

/// `map` setting `path` to `value`.
fn seed(path: &str, value: Value) -> Value {
    json!({"id": "seed", "function": {"name": "map", "input": {"mappings": [
        {"path": path, "logic": value}]}}})
}

/// Workflow `id`, with `extra` on it.
fn workflow(id: &str, tasks: Vec<Value>, extra: &[(&str, Value)]) -> Value {
    let mut w = json!({"id": id, "tasks": tasks});
    for (key, value) in extra {
        w[*key] = value.clone();
    }
    w
}

/// The fan-out, then `after`, in workflow `w`.
fn fanning(for_each: Value, extra: &[(&str, Value)]) -> Vec<Value> {
    vec![workflow(
        "w",
        vec![fan(for_each, extra), mark("after")],
        &[],
    )]
}

/// `over` `data.ps` bound at `p`, collecting `temp_data.out` into `data.outs`.
fn collect_into() -> Value {
    json!({"over": {"var": "data.ps"}, "as": "p",
           "collect": "temp_data.out", "into": "data.outs"})
}

/// [`collect_into`] at `max_concurrency`.
fn concurrent(max: usize) -> Value {
    let mut for_each = collect_into();
    for_each["max_concurrency"] = json!(max);
    for_each
}

/// A run over `data.ps = ps`.
fn over(ps: Value) -> Run {
    data(json!({"ps": ps}))
}

/// v3's engine and v4's over `workflows`, `echo` registered on both.
fn engines(workflows: &[Value]) -> (dataflow_rs_v3::Engine, Engine) {
    let v3 = v3_engine_with(workflows, |b| b.register("echo", v3_twin::Echo))
        .unwrap_or_else(|e| panic!("v3 builds: {e}"));
    let v4 = v4_engine_with(workflows, |b| b.register("echo", Echo::default()))
        .unwrap_or_else(|e| panic!("v4 builds: {e}"));
    (v3, v4)
}

/// `run` on v3 and on v4, which must agree: v4's message as JSON, the
/// outcome's code, and v4's report.
async fn same(workflows: &[Value], run: &Run) -> (Value, String, dataflow_rs::Run) {
    let (v3, v4) = engines(workflows);
    let (want, want_code) = on_v3(&v3, run).await;
    let (got, code, report) = on_v4(&v4, run).await;
    assert_eq!(
        (&got, &code),
        (&want, &want_code),
        "v4 against v3 over {workflows:#?}\nwith {:?}",
        run.data
    );
    (serde_json::from_str(&got).unwrap(), code, report)
}

/// `(element_index, status)` of each audit entry of `task`.
fn entries(m: &Value, task: &str) -> Vec<(Value, Value)> {
    m["audit_trail"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["task_id"] == task)
        .map(|e| (e["element_index"].clone(), e["status"].clone()))
        .collect()
}

/// The `(code, element_index)` of each record.
fn records(m: &Value) -> Vec<(String, Value)> {
    m["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["code"].as_str().unwrap().to_owned(),
                e["element_index"].clone(),
            )
        })
        .collect()
}

fn ab(n: usize) -> Value {
    json!((0..n).map(|i| json!({"id": i})).collect::<Vec<_>>())
}

#[tokio::test]
async fn results_land_in_element_order_and_bindings_do_not_leak() {
    let ps = json!([{"id": "a"}, {"id": "b"}, {"id": "c"}]);
    let (m, code, _) = same(&fanning(collect_into(), &[]), &over(ps)).await;
    assert_eq!(code, "ok");
    assert_eq!(
        m["context"]["data"]["outs"],
        json!([{"id": "a", "i": 0}, {"id": "b", "i": 1}, {"id": "c", "i": 2}])
    );
    assert_eq!(
        m["context"]["temp_data"],
        json!({"out": {"id": "c", "i": 2}, "by_eval": {"id": "c"}}),
        "the bindings live only in each call; the writes are replayed in order"
    );
    assert_eq!(
        entries(&m, "t"),
        [
            (json!(0), json!(200)),
            (json!(1), json!(200)),
            (json!(2), json!(200))
        ]
    );
    assert_eq!(m["context"]["data"]["after_ran"], json!(true));
    assert_eq!(
        m["context"]["metadata"]["progress"],
        json!({"workflow_id": "w", "task_id": "after", "status_code": 200})
    );
}

#[tokio::test]
async fn audit_changes_carry_each_elements_writes_and_its_result_slot() {
    let (m, _, _) = same(&fanning(collect_into(), &[]), &over(ab(2))).await;
    let paths: Vec<Vec<Value>> = m["audit_trail"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["task_id"] == "t")
        .map(|e| {
            e["changes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["path"].clone())
                .collect()
        })
        .collect();
    assert_eq!(
        paths,
        [
            [
                json!("temp_data.out"),
                json!("temp_data.by_eval"),
                json!("data.outs.0")
            ],
            [
                json!("temp_data.out"),
                json!("temp_data.by_eval"),
                json!("data.outs.1")
            ],
        ]
    );
}

#[tokio::test]
async fn capture_changes_off_keeps_the_entries_and_the_results() {
    let run = Run {
        capture_off: true,
        ..over(ab(2))
    };
    let (m, _, _) = same(&fanning(collect_into(), &[]), &run).await;
    assert_eq!(
        m["context"]["data"]["outs"],
        json!([{"id": 0, "i": 0}, {"id": 1, "i": 1}])
    );
    assert_eq!(
        entries(&m, "t"),
        [(json!(0), json!(200)), (json!(1), json!(200))]
    );
    assert!(
        m["audit_trail"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["changes"] == json!([])),
        "no change captured, but every entry kept"
    );
}

#[tokio::test]
async fn calls_are_isolated_from_each_other() {
    // Every call sees `temp_data.seen` as it was before the fan-out, and the
    // writes are replayed in order, each writing 11.
    let for_each = json!({"over": {"var": "data.ps"}, "as": "p",
                          "collect": "temp_data.seen", "into": "data.seen_by"});
    let workflows = [workflow(
        "w",
        vec![seed("temp_data.seen", json!(10)), fan(for_each, &[])],
        &[],
    )];
    let ps =
        json!([{"inc": "temp_data.seen"}, {"inc": "temp_data.seen"}, {"inc": "temp_data.seen"}]);
    let (m, _, _) = same(&workflows, &over(ps)).await;
    assert_eq!(m["context"]["data"]["seen_by"], json!([11, 11, 11]));
    assert_eq!(m["context"]["temp_data"]["seen"], json!(11));
}

#[tokio::test]
async fn without_collect_and_into_the_writes_are_simply_replayed() {
    let for_each = json!({"over": {"var": "data.ps"}, "as": "p"});
    let (m, _, _) = same(&fanning(for_each, &[]), &over(ab(2))).await;
    assert_eq!(
        entries(&m, "t"),
        [(json!(0), json!(200)), (json!(1), json!(200))]
    );
    assert_eq!(m["context"]["temp_data"]["out"], json!({"id": 1, "i": 1}));
    assert!(m["context"]["data"].get("outs").is_none());
}

#[tokio::test]
async fn an_empty_over_writes_an_empty_array_and_one_record() {
    // Whatever `into` held, its `Change` records `null` before, as v3's.
    for (data, terminal) in [
        (json!({"ps": []}), false),
        (json!({"ps": [], "outs": "before"}), false),
        (json!({"ps": []}), true),
    ] {
        let workflows = fanning(collect_into(), &[("terminal", json!(terminal))]);
        let (m, _, _) = same(&workflows, &vs_v3::data(data)).await;
        assert_eq!(m["context"]["data"]["outs"], json!([]));
        assert_eq!(entries(&m, "t"), [(Value::Null, json!(200))]);
        let entry = &m["audit_trail"][0];
        assert_eq!(
            entry["changes"],
            json!([{"path": "data.outs", "old_value": null, "new_value": []}])
        );
        assert_eq!(
            m["context"]["data"].get("after_ran").is_none(),
            terminal,
            "terminal halts after an empty fan-out too"
        );
    }
}

#[tokio::test]
async fn a_failed_element_leaves_null_under_continue() {
    // A 5xx element's writes land and are audited; an `Err` element's do not,
    // but the records it made itself are kept. Every record carries its
    // element.
    let ps = json!([
        {"id": "a"},
        {"id": "b", "o": {"status": 500}, "w": [["data.wrote_b", true]]},
        {"id": "c", "o": {"error": {"code": "PARTNER_DOWN", "message": "down"}},
         "w": [["data.wrote_c", true]], "warn": true},
        {"id": "d"}
    ]);
    let workflows = fanning(collect_into(), &[("continue_on_error", json!(true))]);
    let (m, code, _) = same(&workflows, &over(ps)).await;
    assert_eq!(code, "ok");
    let data = &m["context"]["data"];
    assert_eq!(
        data["outs"],
        json!([{"id": "a", "i": 0}, null, null, {"id": "d", "i": 3}])
    );
    assert_eq!(data["wrote_b"], json!(true));
    assert!(data.get("wrote_c").is_none(), "an `Err` writes nothing");
    assert_eq!(data["after_ran"], json!(true));
    assert_eq!(
        records(&m),
        [
            ("TASK_STATUS_ERROR".to_owned(), json!(1)),
            ("WARNED".to_owned(), json!(2)),
            ("PARTNER_DOWN".to_owned(), json!(2)),
        ]
    );
    assert_eq!(m["audit_trail"][2]["changes"], json!([]));
    assert_eq!(
        m["audit_trail"][1]["changes"][3],
        json!({"path": "data.outs.1", "old_value": null, "new_value": null})
    );
}

#[tokio::test]
async fn a_failed_element_fails_the_task_without_continue() {
    // `b` fails the task: `a` folded, `b` recorded, `c` never started.
    for failure in [
        json!({"status": 503}),
        json!({"error": {"code": "PARTNER_DOWN", "message": "down"}}),
    ] {
        for workflow_continues in [false, true] {
            let ps = json!([{"id": "a"}, {"id": "b", "o": failure}, {"id": "c"}]);
            let workflows = vec![
                workflow(
                    "w",
                    vec![fan(collect_into(), &[]), mark("after")],
                    &[("continue_on_error", json!(workflow_continues))],
                ),
                workflow("w2", vec![mark("next")], &[("priority", json!(1))]),
            ];
            let (m, code, _) = same(&workflows, &over(ps)).await;
            let data = &m["context"]["data"];
            assert_eq!(
                data["outs"],
                json!([{"id": "a", "i": 0}, null, null]),
                "{failure}"
            );
            let status = if failure.get("status").is_some() {
                503
            } else {
                500
            };
            assert_eq!(
                entries(&m, "t"),
                [(json!(0), json!(200)), (json!(1), json!(status))]
            );
            assert!(data.get("after_ran").is_none());
            assert_eq!(data.get("next_ran").is_some(), workflow_continues);
            assert_eq!(code == "ok", workflow_continues, "{code}");
        }
    }
}

#[tokio::test]
async fn errors_a_call_records_itself_are_stamped_and_kept() {
    let ps = json!([{"id": "a"}, {"id": "b", "warn": true}]);
    let (m, _, _) = same(&fanning(collect_into(), &[]), &over(ps)).await;
    assert_eq!(records(&m), [("WARNED".to_owned(), json!(1))]);
    assert_eq!(m["errors"][0]["timestamp"], json!("<timestamp>"));
}

#[tokio::test]
async fn an_element_halt_stops_new_calls_and_halts_the_workflow() {
    let ps = json!([{"id": "a"}, {"id": "b", "o": "halt"}, {"id": "c"}]);
    let workflows = vec![
        workflow("w", vec![fan(collect_into(), &[]), mark("after")], &[]),
        workflow("w2", vec![mark("next")], &[("priority", json!(1))]),
    ];
    let (m, code, report) = same(&workflows, &over(ps)).await;
    assert_eq!(code, "ok");
    assert_eq!(
        entries(&m, "t"),
        [(json!(0), json!(200)), (json!(1), json!(299))]
    );
    let data = &m["context"]["data"];
    assert_eq!(
        data["outs"],
        json!([{"id": "a", "i": 0}, {"id": "b", "i": 1}, null]),
        "a halt is not a failure: its result lands"
    );
    assert!(data.get("after_ran").is_none());
    assert_eq!(data["next_ran"], json!(true));
    let exits: Vec<_> = report.workflows().map(|w| w.exit).collect();
    assert_eq!(
        exits,
        [Some(WorkflowExit::Halted), Some(WorkflowExit::Completed)]
    );
}

#[tokio::test]
async fn on_rejection_halt_halts_after_the_whole_fan_out() {
    // v3's `halt_on: "failure"` is `on_rejection: halt`, and with
    // `continue_on_error` also `on_failure: halt`: each applies once, after
    // every element ran.
    for (flags, first) in [
        (json!({"halt_on": "failure"}), json!({"status": 404})),
        (
            json!({"halt_on": "failure", "continue_on_error": true}),
            json!({"status": 404}),
        ),
        (
            json!({"halt_on": "failure", "continue_on_error": true}),
            json!({"status": 500}),
        ),
        (
            json!({"halt_on": "failure", "continue_on_error": true}),
            json!({"error": {"code": "PARTNER_DOWN", "message": "down"}}),
        ),
        // Without `halt_on` a rejection halts nothing.
        (json!({}), json!({"status": 404})),
    ] {
        let extra: Vec<(&str, Value)> = flags
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        let ps = json!([{"id": "a", "o": first}, {"id": "b"}, {"id": "c"}]);
        let (m, _, _) = same(&fanning(collect_into(), &extra), &over(ps)).await;
        let indices: Vec<Value> = entries(&m, "t").into_iter().map(|(i, _)| i).collect();
        assert_eq!(indices, [json!(0), json!(1), json!(2)], "{flags} {first}");
        assert_eq!(
            m["context"]["data"].get("after_ran").is_none(),
            flags.get("halt_on").is_some(),
            "{flags} {first}"
        );
    }
}

#[tokio::test]
async fn terminal_halts_after_the_whole_fan_out() {
    let workflows = fanning(collect_into(), &[("terminal", json!(true))]);
    let (m, _, _) = same(&workflows, &over(ab(2))).await;
    assert_eq!(
        entries(&m, "t"),
        [(json!(0), json!(200)), (json!(1), json!(200))]
    );
    assert!(m["context"]["data"].get("after_ran").is_none());
}

#[tokio::test]
async fn a_skip_element_keeps_its_writes_and_gets_no_entry() {
    let ps = json!([{"id": "a"}, {"id": "b", "o": "skip", "w": [["data.skipped_wrote", true]]}, {"id": "c"}]);
    let (m, _, _) = same(&fanning(collect_into(), &[]), &over(ps)).await;
    assert_eq!(
        entries(&m, "t"),
        [(json!(0), json!(200)), (json!(2), json!(200))]
    );
    let data = &m["context"]["data"];
    assert_eq!(data["skipped_wrote"], json!(true));
    assert_eq!(
        data["outs"],
        json!([{"id": "a", "i": 0}, {"id": "b", "i": 1}, {"id": "c", "i": 2}])
    );
}

#[tokio::test]
async fn a_fan_out_inside_a_loop_stamps_both_coordinates() {
    let for_each = json!({"over": [{"id": "a"}, {"id": "b"}], "as": "p"});
    let workflows = [workflow(
        "w",
        vec![fan(for_each, &[])],
        &[("loop", json!({"counter": "i", "max": 2}))],
    )];
    let (m, _, _) = same(&workflows, &Run::default()).await;
    let stamps: Vec<(Value, Value)> = m["audit_trail"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["loop_counter"].clone(), e["element_index"].clone()))
        .collect();
    assert_eq!(
        stamps,
        [
            (json!(0), json!(0)),
            (json!(0), json!(1)),
            (json!(1), json!(0)),
            (json!(1), json!(1))
        ]
    );
}

#[tokio::test]
async fn a_fan_out_in_loop_setup_runs_once() {
    let mut setup = fan(
        json!({"over": [{"id": "a"}, {"id": "b"}], "as": "p",
               "collect": "temp_data.out", "into": "temp_data.batch"}),
        &[],
    );
    setup["id"] = json!("s");
    let workflows = [workflow(
        "w",
        vec![json!({"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.n", "logic": {"var": "temp_data.i"}}]}}})],
        &[("loop", json!({"counter": "i", "max": 3, "setup": [setup]}))],
    )];
    let (m, _, _) = same(&workflows, &Run::default()).await;
    assert_eq!(
        entries(&m, "s"),
        [(json!(0), json!(200)), (json!(1), json!(200))]
    );
    assert!(
        m["audit_trail"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["task_id"] == "s")
            .all(|e| e.get("loop_counter").is_none()),
        "setup is not a sweep"
    );
    assert_eq!(
        m["context"]["temp_data"]["batch"],
        json!([{"id": "a", "i": 0}, {"id": "b", "i": 1}])
    );
}

#[tokio::test]
async fn results_are_identical_at_every_concurrency() {
    // Earlier elements yield more, so concurrent calls finish out of order;
    // one fails under `continue_on_error`, and the fold is the same.
    let mut ps: Vec<Value> = (0..6)
        .map(|i| json!({"id": i, "y": 6 - i, "w": [[format!("data.log.{i}"), i]]}))
        .collect();
    ps[2]["o"] = json!({"status": 500});
    let mut seen = Vec::new();
    for max in [1, 2, 4, 8] {
        let workflows = fanning(concurrent(max), &[("continue_on_error", json!(true))]);
        let (m, _, _) = same(&workflows, &over(json!(ps))).await;
        let mut outs = json!([0, 1, null, 3, 4, 5]);
        for (i, out) in outs.as_array_mut().unwrap().iter_mut().enumerate() {
            if !out.is_null() {
                *out = json!({"id": i, "i": i});
            }
        }
        assert_eq!(m["context"]["data"]["outs"], outs, "max_concurrency {max}");
        assert_eq!(
            m["context"]["data"]["log"],
            json!([0, 1, 2, 3, 4, 5]),
            "max_concurrency {max}"
        );
        seen.push(m);
    }
    assert!(seen.windows(2).all(|w| w[0] == w[1]));
}

#[tokio::test]
async fn a_halt_yields_the_same_message_at_every_concurrency() {
    // `1` halts. `2` and `3` yield least, so above one call in flight they
    // finish first and are kept: the fold must still stop at `1`.
    let ps = json!([
        {"id": 0, "y": 3},
        {"id": 1, "y": 3, "o": "halt"},
        {"id": 2, "y": 0, "w": [["data.late", 2]]},
        {"id": 3, "y": 0, "w": [["data.late", 3]]},
    ]);
    let mut seen = Vec::new();
    for max in [1, 2, 4, 8] {
        let (m, _, _) = same(&fanning(concurrent(max), &[]), &over(ps.clone())).await;
        assert_eq!(
            m["context"]["data"]["outs"],
            json!([{"id": 0, "i": 0}, {"id": 1, "i": 1}, null, null]),
            "max_concurrency {max}"
        );
        assert_eq!(
            entries(&m, "t"),
            [(json!(0), json!(200)), (json!(1), json!(299))],
            "max_concurrency {max}"
        );
        assert!(
            m["context"]["data"].get("late").is_none(),
            "max_concurrency {max}: a finished element after the halt contributes nothing"
        );
        seen.push(m);
    }
    assert!(seen.windows(2).all(|w| w[0] == w[1]));
}

#[tokio::test]
async fn max_concurrency_bounds_calls_in_flight() {
    // Every call yields, so `max` calls are in flight at once; a later
    // element yields less, so above one in flight the calls finish out of
    // element order, as the other concurrency tests rely on.
    for max in [1, 2, 4, 8] {
        let echo = Echo::default();
        let peak = Arc::clone(&echo.peak);
        let finished = Arc::clone(&echo.finished);
        let e =
            v4_engine_with(&fanning(concurrent(max), &[]), |b| b.register("echo", echo)).unwrap();
        let ps: Vec<Value> = (0..8).map(|i| json!({"id": i, "y": 9 - i})).collect();
        let mut m = Message::from_parts(
            OwnedDataValue::Null,
            OwnedDataValue::from(&json!({"ps": ps})),
            OwnedDataValue::Null,
        );
        let run = e.run(&mut m).await;
        assert!(matches!(run.status(), RunStatus::Completed));
        assert_eq!(peak.load(Ordering::SeqCst), max, "max_concurrency {max}");
        let outs = Value::from(m.context().data())["outs"].clone();
        let ids: Vec<Value> = outs
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["id"].clone())
            .collect();
        assert_eq!(ids, (0..8).map(|i| json!(i)).collect::<Vec<_>>());
        let finished = finished.lock().unwrap().clone();
        assert_eq!(
            finished.is_sorted(),
            max == 1,
            "max_concurrency {max}: {finished:?}"
        );
    }
}

#[tokio::test]
async fn no_call_starts_after_one_that_stops_the_fan_out() {
    // An element stops new calls when it halts, or fails the task: an `Err`
    // or a 5xx under `on_failure: fail`. A rejection, or a failure the task
    // continues past, stops nothing. The calls in flight finish: at two in
    // flight, `b` is running when `a` halts, and `c` never starts.
    let err = json!({"error": {"code": "PARTNER_DOWN", "message": "down"}});
    for (max, flags, first, called) in [
        (1, json!({}), json!("halt"), vec![0]),
        (1, json!({}), json!({"status": 503}), vec![0]),
        (1, json!({}), err.clone(), vec![0]),
        (
            1,
            json!({"continue_on_error": true}),
            json!({"status": 503}),
            vec![0, 1, 2],
        ),
        (1, json!({"continue_on_error": true}), err, vec![0, 1, 2]),
        (
            1,
            json!({"halt_on": "failure"}),
            json!({"status": 404}),
            vec![0, 1, 2],
        ),
        (2, json!({}), json!("halt"), vec![0, 1]),
    ] {
        let extra: Vec<(&str, Value)> = flags
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        let echo = Echo::default();
        let finished = Arc::clone(&echo.finished);
        let e = v4_engine_with(&fanning(concurrent(max), &extra), |b| {
            b.register("echo", echo)
        })
        .unwrap();
        let ps = json!([{"id": "a", "o": first}, {"id": "b", "y": 2}, {"id": "c"}]);
        let mut m = Message::from_parts(
            OwnedDataValue::Null,
            OwnedDataValue::from(&json!({"ps": ps})),
            OwnedDataValue::Null,
        );
        e.run(&mut m).await;
        assert_eq!(
            *finished.lock().unwrap(),
            called,
            "max_concurrency {max}, {flags}, {first}"
        );
    }
}

#[tokio::test]
async fn elements_writing_under_one_new_parent_both_land() {
    // `data.prices` is absent before the fan-out: each call creates it with
    // its own key, and both keys land, as v3's replayed `Change`s do.
    let ps = json!([
        {"id": "a", "w": [["data.prices.a", 1]]},
        {"id": "b", "w": [["data.prices.b", 2]]},
    ]);
    let (m, _, _) = same(&fanning(collect_into(), &[]), &over(ps)).await;
    assert_eq!(m["context"]["data"]["prices"], json!({"a": 1, "b": 2}));
}

#[tokio::test]
async fn an_element_writing_below_its_binding_reads_it_back_and_the_write_is_replayed() {
    // Each call reads its own write over its binding. The binding is never
    // written to the run, so the settle replays each write at its path where
    // only the earlier elements' writes are: element 0's write below a
    // string is blocked in its call and lands at the settle, which creates
    // `temp_data.p`; element 1's replaces what that created.
    let ps = json!([
        {"id": "a", "w": [["temp_data.p.id.x", 1]]},
        {"id": "b", "w": [["temp_data.p.id", "B"]]},
        {"id": "c", "w": [["temp_data.p.extra", true]]},
    ]);
    let (m, _, _) = same(&fanning(collect_into(), &[]), &over(ps)).await;
    assert_eq!(
        m["context"]["data"]["outs"],
        json!([{"id": "a", "i": 0}, {"id": "B", "i": 1}, {"id": "c", "i": 2}])
    );
    let temp = &m["context"]["temp_data"];
    assert_eq!(temp["p"], json!({"id": "B", "extra": true}));
    assert!(temp.get("p_index").is_none());
    assert_eq!(
        temp["by_eval"],
        json!({"id": "c", "w": [["temp_data.p.extra", true]], "extra": true})
    );
}

#[tokio::test]
async fn elements_appending_to_one_array_land_in_element_order() {
    // An append read over the run's view and written whole: the later
    // element's array is the one that stays. An append at an index of its
    // own: both land, in element order, where neither existed before.
    let ps = json!([
        {"id": "a", "a": [["data.list", "a"]], "w": [["data.log.0", "a"]]},
        {"id": "b", "a": [["data.list", "b"]], "w": [["data.log.1", "b"]]},
    ]);
    let workflows = [workflow(
        "w",
        vec![seed("data.list", json!([0])), fan(collect_into(), &[])],
        &[],
    )];
    let (m, _, _) = same(&workflows, &over(ps)).await;
    assert_eq!(m["context"]["data"]["list"], json!([0, "b"]));
    assert_eq!(m["context"]["data"]["log"], json!(["a", "b"]));
}

#[test]
fn a_for_each_on_a_builtin_is_refused_on_both() {
    let workflows = [workflow(
        "w",
        vec![json!({"id": "t", "for_each": {"over": [1], "as": "p"},
            "function": {"name": "map", "input": {"mappings": [
                {"path": "data.x", "logic": 1}]}}})],
        &[],
    )];
    assert!(v3_engine(&workflows).is_err());
    assert!(v4_engine(&workflows).is_err());
}

#[tokio::test]
async fn a_non_array_over_is_one_logic_error() {
    // I91: a loop's rule, `LOGIC_ERROR` of kind `Eval`, where v3 wrote
    // `TASK_ERROR` (break 4, widened).
    let text = "Task t: for_each.over must evaluate to an array, got a string";
    for workflow_continues in [false, true] {
        let workflows = vec![
            workflow(
                "w",
                vec![fan(collect_into(), &[]), mark("after")],
                &[("continue_on_error", json!(workflow_continues))],
            ),
            workflow("w2", vec![mark("next")], &[("priority", json!(1))]),
        ];
        let e = v4_engine_with(&workflows, |b| b.register("echo", Echo::default())).unwrap();
        let mut m = Message::from_parts(
            OwnedDataValue::Null,
            OwnedDataValue::from(&json!({"ps": "nope"})),
            OwnedDataValue::Null,
        );
        let run = e.run(&mut m).await;
        match (run.status(), workflow_continues) {
            (RunStatus::Stopped(e), false) => {
                assert_eq!(e.code(), "LOGIC_ERROR");
                assert_eq!(e.kind(), &ErrorKind::Eval);
                assert_eq!(e.message(), text);
            }
            (RunStatus::CompletedWithErrors, true) => {}
            (status, _) => panic!("{status:?}"),
        }
        let audit: Vec<_> = m
            .audit()
            .iter()
            .map(|e| (e.task_id.to_string(), e.status, e.element_index))
            .collect();
        let mut want = vec![("t".to_owned(), 500, None)];
        if workflow_continues {
            want.push(("next".to_owned(), 200, None));
        }
        assert_eq!(audit, want);
        let [record] = m.errors() else {
            panic!("one record: {:?}", m.errors());
        };
        assert_eq!(
            (
                record.code.as_str(),
                record.message.as_str(),
                record.workflow_id.as_deref(),
                record.task_id.as_deref(),
                record.element_index
            ),
            (
                "LOGIC_ERROR",
                format!("Task t error: Logic evaluation error: {text}").as_str(),
                Some("w"),
                Some("t"),
                None
            )
        );
        let data = Value::from(m.context().data());
        assert!(data.get("outs").is_none() && data.get("after_ran").is_none());
    }
}

/// A layer that goes straight on.
struct Pass;

impl CallLayer for Pass {
    fn call<'a>(&'a self, ctx: &'a mut Ctx<'_>, next: Next<'a>) -> BoxFuture<'a, Result<Outcome>> {
        next.run(ctx)
    }
}

#[test]
fn the_run_future_is_send_with_a_fan_out_and_layers() {
    fn assert_send<T: Send>(_: &T) {}
    let e = v4_engine_with(&fanning(concurrent(4), &[]), |b| {
        b.register("echo", Echo::default()).layer(Pass)
    })
    .unwrap();
    let run_layers: Vec<Arc<dyn CallLayer>> = vec![Arc::new(Pass)];
    let opts = RunOptions::default().layers(&run_layers);
    let mut m = Message::from_parts(
        OwnedDataValue::Null,
        OwnedDataValue::from(&json!({"ps": ab(3)})),
        OwnedDataValue::Null,
    );
    assert_send(&e.run_with(&mut m, &opts));
}
