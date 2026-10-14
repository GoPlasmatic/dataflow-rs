extern crate dataflow_rs_v4 as dataflow_rs;

// Host function calls end to end, through `Engine::builder()` and
// `run_with`: a `Ctx` reads the run's one view and writes an overlay, which
// lands on `Ok` and is dropped on `Err` (break 1); the recorder then runs as
// for a built-in; the run's layers wrap the registry's, which wrap the
// function; and the run future stays `Send` with all of it in it.
//
// One function, `probe`, drives every rule from its config: it writes
// `writes` in key order, then evaluates `reads`, logging what it saw, then
// returns what `outcome` names.

use dataflow_rs::expr::{ExprEngine, FieldKind, FieldSpec, Output, Secrets, ValueKind};
use dataflow_rs::function::{
    BoxFuture, CallLayer, FunctionConfig, FunctionSpec, Next, Templated, layers,
};
use dataflow_rs::run::{ErrorKind, ErrorRecord, RunStatus, WorkflowExit};
use dataflow_rs::{
    Ctx, Engine, Error, Expr, Function, Message, Outcome, Path, Registry, Result, Run, RunOptions,
};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

/// The probe's config.
#[derive(FunctionConfig)]
struct ProbeConfig {
    /// Evaluated after the writes, in order, each logged. A single-`var`
    /// read is also read through `Ctx::get`.
    reads: Vec<Expr>,
    /// Written in key order. A key's text after a space is a label, so one
    /// path can be written twice; a key starting with `-` removes the path
    /// after it.
    writes: BTreeMap<String, Expr>,
    /// What the call returns, or a list of steps: `"record"` records an
    /// error through `Ctx::record_error` and goes on; `"ok"`, `"skip"`,
    /// `"halt"`, a status number, or `"err"` (a `PARTNER_DOWN` service
    /// error) is the result.
    outcome: Expr,
}

/// What one read saw.
#[derive(Clone, Debug, PartialEq)]
struct Read {
    /// The read's expression, as authored.
    raw: Value,
    /// What it evaluated to.
    value: Value,
    /// Whether the evaluation lent its result.
    lent: bool,
    /// For a single-`var` read, what `Ctx::get` gave at that path and
    /// whether it was borrowed.
    got: Option<(Option<Value>, bool)>,
}

/// One call's `CallInfo`, owned.
#[derive(Clone, Debug, PartialEq)]
struct Info {
    program: u64,
    workflow: u32,
    task: u32,
    function: String,
    workflow_id: String,
    task_id: String,
    channel: Option<String>,
    loop_counter: Option<i64>,
    element_index: Option<usize>,
}

/// What the probe and the layers saw, in order.
#[derive(Debug, Default)]
struct Seen {
    events: Vec<String>,
    infos: Vec<Info>,
    reads: Vec<Read>,
}

type Shared = Arc<Mutex<Seen>>;

/// The function every test registers as `probe`.
struct Probe(Shared);

impl Probe {
    fn log(&self, f: impl FnOnce(&mut Seen)) {
        f(&mut self.0.lock().unwrap());
    }
}

impl Function for Probe {
    type Config = ProbeConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &ProbeConfig) -> Result<Outcome> {
        // Every call awaits, so the call is held across a suspension.
        tokio::task::yield_now().await;
        let info = ctx.info();
        let info = Info {
            program: info.program.index(),
            workflow: info.workflow.index(),
            task: info.task.index(),
            function: info.function.to_owned(),
            workflow_id: info.workflow_id.to_owned(),
            task_id: info.task_id.to_owned(),
            channel: info.channel.map(str::to_owned),
            loop_counter: info.loop_counter,
            element_index: info.element_index,
        };
        self.log(|seen| {
            seen.events.push("function".to_owned());
            seen.infos.push(info);
        });
        for (key, expr) in &cfg.writes {
            let target = key.split_once(' ').map_or(key.as_str(), |(path, _)| path);
            match target.strip_prefix('-') {
                Some(path) => ctx.remove(&path.parse()?)?,
                None => {
                    let value = ctx.eval(expr)?.into_owned();
                    ctx.set(&target.parse()?, value)?;
                }
            }
        }
        for expr in &cfg.reads {
            let evaluated = ctx.eval(expr);
            let lent = matches!(evaluated, Ok(Cow::Borrowed(_)));
            let value = Value::from(&*evaluated?);
            let got = match expr.raw() {
                Value::Object(o) if o.len() == 1 => o.get("var").and_then(Value::as_str).map(|p| {
                    let got = ctx.get(&p.parse().expect("a read's var is a path"));
                    let borrowed = matches!(got, Some(Cow::Borrowed(_)));
                    (got.map(|v| Value::from(&*v)), borrowed)
                }),
                _ => None,
            };
            let raw = expr.raw().clone();
            self.log(|seen| {
                seen.reads.push(Read {
                    raw,
                    value,
                    lent,
                    got,
                });
            });
        }
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
                _ => {
                    let status = step.as_u64().expect("a step is a word or a status");
                    Ok(Outcome::Status(u16::try_from(status).unwrap()))
                }
            };
        }
        result
    }
}

/// Logs its name on the way in and on the way out.
struct Named(&'static str, Shared);

impl CallLayer for Named {
    fn call<'a>(&'a self, ctx: &'a mut Ctx<'_>, next: Next<'a>) -> BoxFuture<'a, Result<Outcome>> {
        Box::pin(async move {
            self.1.lock().unwrap().events.push(self.0.to_owned());
            let outcome = next.run(ctx).await;
            self.1.lock().unwrap().events.push(self.0.to_owned());
            outcome
        })
    }
}

/// An engine over `workflows` with the probe registered, and what it sees.
fn engine(workflows: &[Value]) -> (Engine, Shared) {
    let seen = Shared::default();
    let e = workflows
        .iter()
        .fold(
            Engine::builder().register("probe", Probe(Arc::clone(&seen))),
            |b, w| b.workflow_json(w),
        )
        .build()
        .unwrap_or_else(|issues| panic!("the engine builds: {issues:?}"));
    (e, seen)
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

/// A probe task.
fn probe(id: &str, reads: Value, writes: Value, outcome: Value) -> Value {
    json!({"id": id, "function": {"name": "probe", "input": {
        "reads": reads, "writes": writes, "outcome": outcome}}})
}

/// A `map` task writing `logic` at `path`.
fn map(id: &str, path: &str, logic: Value) -> Value {
    json!({"id": id, "function": {"name": "map", "input": {
        "mappings": [{"path": path, "logic": logic}]}}})
}

/// `task` with each `(key, value)` of `extra`.
fn with(mut task: Value, extra: &[(&str, Value)]) -> Value {
    for (key, value) in extra {
        task[*key] = value.clone();
    }
    task
}

/// A workflow `id` holding `tasks`, with each `(key, value)` of `extra`.
fn workflow(id: &str, extra: &[(&str, Value)], tasks: Vec<Value>) -> Value {
    with(json!({"id": id, "tasks": tasks}), extra)
}

fn data(m: &Message) -> Value {
    m.to_json()["context"]["data"].clone()
}

/// `(workflow, task, status)` of each audit entry.
fn audit(m: &Message) -> Vec<(String, String, usize)> {
    m.audit()
        .iter()
        .map(|e| (e.workflow_id.to_string(), e.task_id.to_string(), e.status))
        .collect()
}

fn entry(workflow: &str, task: &str, status: usize) -> (String, String, usize) {
    (workflow.to_owned(), task.to_owned(), status)
}

/// The changes of the audit entry of `task`, as JSON.
fn changes(m: &Message, task: &str) -> Value {
    let entry = m
        .audit()
        .iter()
        .find(|e| &*e.task_id == task)
        .unwrap_or_else(|| panic!("an entry for {task}"));
    serde_json::to_value(&entry.changes).unwrap()
}

/// `(code, message)` of each error record.
fn errors(m: &Message) -> Vec<(String, String)> {
    m.errors()
        .iter()
        .map(|e| (e.code.clone(), e.message.clone()))
        .collect()
}

fn exits(run: &Run) -> Vec<Option<WorkflowExit>> {
    run.workflows().map(|w| w.exit).collect()
}

fn reads(seen: &Shared) -> Vec<Read> {
    seen.lock().unwrap().reads.clone()
}

#[tokio::test]
async fn a_host_call_reads_the_run_view_and_its_writes_land() {
    let (e, _) = engine(&[workflow(
        "w",
        &[],
        vec![
            map("m1", "data.a", json!(1)),
            probe(
                "p",
                json!([]),
                json!({
                    "data.b": {"var": "data.a"},
                    "data.c": {"cat": [{"var": "data.b"}, "!"]}
                }),
                json!("ok"),
            ),
            map("m2", "data.d", json!({"var": "data.c"})),
        ],
    )]);
    let mut m = message(json!({"x": "kept"}));
    let run = e.run(&mut m).await;
    assert!(
        matches!(run.status(), RunStatus::Completed),
        "{:?}",
        run.status()
    );
    // The probe read `map`'s write from the run view, its second write read
    // its first, and the next task read both.
    assert_eq!(
        data(&m),
        json!({"x": "kept", "a": 1, "b": 1, "c": "1!", "d": "1!"})
    );
    assert_eq!(
        audit(&m),
        [
            entry("w", "m1", 200),
            entry("w", "p", 200),
            entry("w", "m2", 200)
        ]
    );
    // One `Change` per write, in write order, the absent paths' old values
    // `null`.
    assert_eq!(
        changes(&m, "p"),
        json!([
            {"path": "data.b", "old_value": null, "new_value": 1},
            {"path": "data.c", "old_value": null, "new_value": "1!"}
        ])
    );
    assert_eq!(
        m.to_json()["context"]["metadata"]["progress"],
        json!({"workflow_id": "w", "task_id": "m2", "status_code": 200})
    );
}

#[tokio::test]
async fn a_second_write_to_one_path_leaves_two_changes_and_one_value() {
    let (e, seen) = engine(&[workflow(
        "w",
        &[],
        vec![probe(
            "p",
            json!([{"var": "data.x"}]),
            json!({"data.x 1": "first", "data.x 2": "second"}),
            json!("ok"),
        )],
    )]);
    let mut m = message(json!({"x": "zero", "y": 1}));
    e.run(&mut m).await;
    // Each `Change` carries the old value as the call saw it at that moment.
    assert_eq!(
        changes(&m, "p"),
        json!([
            {"path": "data.x", "old_value": "zero", "new_value": "first"},
            {"path": "data.x", "old_value": "first", "new_value": "second"}
        ])
    );
    assert_eq!(data(&m), json!({"x": "second", "y": 1}));
    assert_eq!(reads(&seen)[0].value, json!("second"));
}

#[tokio::test]
async fn an_evaluation_after_a_write_sees_it() {
    let (e, seen) = engine(&[workflow(
        "w",
        &[],
        vec![probe(
            "p",
            json!([
                {"var": "data.order.id"},
                {"var": "data.order"},
                {"cat": ["id=", {"var": "data.order.id"}]},
                {"var": "data.order.lines.1"}
            ]),
            json!({
                "data.order.id": "ORD-43",
                "data.order.lines.1": {"sku": "B"}
            }),
            json!("ok"),
        )],
    )]);
    let mut m = message(json!({"order": {"id": "ORD-42", "qty": 3, "lines": [{"sku": "A"}]}}));
    e.run(&mut m).await;
    let order = json!({"id": "ORD-43", "qty": 3, "lines": [{"sku": "A"}, {"sku": "B"}]});
    let seen = reads(&seen);
    let values: Vec<&Value> = seen.iter().map(|r| &r.value).collect();
    assert_eq!(
        values,
        [
            &json!("ORD-43"),
            &order,
            &json!("id=ORD-43"),
            &json!({"sku": "B"})
        ]
    );
    // `get` agrees with the evaluation.
    for read in &seen {
        if let Some((got, _)) = &read.got {
            assert_eq!(got.as_ref(), Some(&read.value), "{}", read.raw);
        }
    }
    assert_eq!(data(&m)["order"], order);
}

#[tokio::test]
async fn get_sees_the_overlay_then_the_run_then_the_owned_tree() {
    let (e, seen) = engine(&[workflow(
        "w",
        &[],
        vec![
            map("m", "data.from_map", json!("mapped")),
            probe(
                "p",
                json!([
                    {"var": "data.from_map"},
                    {"var": "data.untouched"},
                    {"var": "data.mine"},
                    {"var": "data.absent"},
                    {"var": "data"}
                ]),
                json!({"data.mine": {"own": true}}),
                json!("ok"),
            ),
        ],
    )]);
    let mut m = message(json!({"untouched": {"k": 1}}));
    e.run(&mut m).await;
    let got: Vec<_> = reads(&seen).into_iter().map(|r| r.got.unwrap()).collect();
    let whole = json!({"untouched": {"k": 1}, "from_map": "mapped", "mine": {"own": true}});
    assert_eq!(got[0].0, Some(json!("mapped")), "the run's write");
    assert_eq!(
        got[1],
        (Some(json!({"k": 1})), true),
        "the owned tree, lent"
    );
    assert_eq!(
        got[2],
        (Some(json!({"own": true})), true),
        "the call's own write, lent"
    );
    assert_eq!(got[3], (None, false), "nothing anywhere");
    // Written below: converted from the run view with the overlay spliced on.
    assert_eq!(got[4], (Some(whole.clone()), false));
    assert_eq!(data(&m), whole);
}

#[tokio::test]
async fn a_constant_is_lent() {
    let (e, seen) = engine(&[workflow(
        "w",
        &[],
        vec![probe(
            "p",
            json!(["constant", 5, {"var": "data.a"}]),
            json!({"data.b": "written"}),
            json!("ok"),
        )],
    )]);
    let mut m = message(json!({"a": "read"}));
    e.run(&mut m).await;
    let lent: Vec<(Value, bool)> = reads(&seen)
        .into_iter()
        .map(|r| (r.value, r.lent))
        .collect();
    assert_eq!(
        lent,
        [
            (json!("constant"), true),
            (json!(5), true),
            (json!("read"), false)
        ]
    );
}

#[tokio::test]
async fn an_err_drops_the_calls_writes_and_keeps_its_records() {
    // Break 1, as `breaks/handler_writes_then_err` records it against v3, and
    // the record a call adds, as `run/host_contract` records it: kept as
    // given, with the run's timestamp, before the recorder's own.
    let failing = probe(
        "t",
        json!([]),
        json!({"data.partial": true, "-data.input": null}),
        json!(["record", "err"]),
    );
    let (e, _) = engine(&[workflow(
        "w",
        &[],
        vec![
            with(failing.clone(), &[("on_failure", json!("continue"))]),
            map("after", "data.after", true.into()),
        ],
    )]);
    let mut m = message(json!({"input": "x"}));
    let run = e.run(&mut m).await;
    assert_eq!(data(&m), json!({"input": "x", "after": true}));
    assert_eq!(audit(&m), [entry("w", "t", 500), entry("w", "after", 200)]);
    assert_eq!(changes(&m, "t"), json!([]));
    let processed_at = m.to_json()["context"]["metadata"]["processed_at"].clone();
    assert_eq!(
        serde_json::to_value(m.errors()).unwrap(),
        json!([
            {"code": "HOST_WARNING", "message": "recorded by the handler", "path": null,
             "timestamp": processed_at},
            {"code": "PARTNER_DOWN", "message": "Task t error: down", "path": null,
             "workflow_id": "w", "task_id": "t", "timestamp": processed_at}
        ])
    );
    assert!(matches!(run.status(), RunStatus::CompletedWithErrors));

    // Under the default `on_failure: fail` the error stops the run, and the
    // writes are dropped all the same.
    let (e, _) = engine(&[workflow(
        "w",
        &[],
        vec![failing, map("after", "data.after", true.into())],
    )]);
    let mut m = message(json!({"input": "x"}));
    let run = e.run(&mut m).await;
    assert_eq!(data(&m), json!({"input": "x"}));
    assert_eq!(audit(&m), [entry("w", "t", 500)]);
    let RunStatus::Stopped(error) = run.status() else {
        panic!("{:?}", run.status());
    };
    assert_eq!((error.code(), error.message()), ("PARTNER_DOWN", "down"));
    assert_eq!(errors(&m).len(), 2);
}

#[tokio::test]
async fn a_skip_keeps_the_writes_and_records_nothing() {
    // v3's rule (`run/host_contract`'s `skipper`): no audit entry, no
    // progress, no record, and the writes stay.
    let (e, _) = engine(&[workflow(
        "w",
        &[],
        vec![
            map("m1", "data.a", json!(1)),
            probe(
                "s",
                json!([]),
                json!({"data.skipper_wrote": true}),
                json!("skip"),
            ),
            map(
                "m2",
                "data.seen",
                json!({"var": "metadata.progress.task_id"}),
            ),
        ],
    )]);
    let mut m = message(json!({}));
    let run = e.run(&mut m).await;
    assert_eq!(
        data(&m),
        json!({"a": 1, "skipper_wrote": true, "seen": "m1"})
    );
    assert_eq!(audit(&m), [entry("w", "m1", 200), entry("w", "m2", 200)]);
    // The skipper's `Change` is no one's: the next entry holds its own only.
    assert_eq!(
        changes(&m, "m2"),
        json!([{"path": "data.seen", "old_value": null, "new_value": "m1"}])
    );
    assert!(m.errors().is_empty());
    assert!(matches!(run.status(), RunStatus::Completed));
}

#[tokio::test]
async fn a_status_applies_the_policies_as_a_builtin_does() {
    let case = |status: u16, policy: (&'static str, &'static str)| async move {
        let task = with(
            probe("t", json!([]), json!({"data.wrote": status}), json!(status)),
            &[(policy.0, json!(policy.1))],
        );
        let (e, _) = engine(&[
            workflow(
                "w",
                &[],
                vec![task, map("after", "data.after", true.into())],
            ),
            workflow(
                "w2",
                &[("priority", json!(1))],
                vec![map("next", "data.next", true.into())],
            ),
        ]);
        let mut m = message(json!({}));
        let run = e.run(&mut m).await;
        (m, run)
    };
    let failed = vec![(
        "TASK_STATUS_ERROR".to_owned(),
        "Task t returned status 503".to_owned(),
    )];

    // A 4xx is a rejection: `continue` goes on, `halt` halts the workflow.
    let (m, run) = case(404, ("on_rejection", "continue")).await;
    assert_eq!(
        audit(&m),
        [
            entry("w", "t", 404),
            entry("w", "after", 200),
            entry("w2", "next", 200)
        ]
    );
    assert!(m.errors().is_empty());
    assert!(matches!(run.status(), RunStatus::Completed));
    let (m, run) = case(404, ("on_rejection", "halt")).await;
    assert_eq!(audit(&m), [entry("w", "t", 404), entry("w2", "next", 200)]);
    assert_eq!(
        exits(&run),
        [Some(WorkflowExit::Halted), Some(WorkflowExit::Completed)]
    );
    assert!(m.errors().is_empty());
    // The writes of any `Ok` land.
    assert_eq!(data(&m), json!({"wrote": 404, "next": true}));

    // A 5xx is a failure, recorded under every `on_failure`.
    let (m, run) = case(503, ("on_failure", "fail")).await;
    assert_eq!(audit(&m), [entry("w", "t", 503)]);
    assert_eq!(errors(&m), failed);
    let record = &m.errors()[0];
    assert_eq!(
        (record.workflow_id.as_deref(), record.task_id.as_deref()),
        (Some("w"), Some("t"))
    );
    let RunStatus::Stopped(error) = run.status() else {
        panic!("{:?}", run.status());
    };
    assert_eq!(
        (error.code(), error.message()),
        ("TASK_ERROR", "Task t failed with status 503")
    );
    let (m, run) = case(503, ("on_failure", "halt")).await;
    assert_eq!(audit(&m), [entry("w", "t", 503), entry("w2", "next", 200)]);
    assert_eq!(errors(&m), failed);
    assert_eq!(exits(&run)[0], Some(WorkflowExit::Halted));
    assert!(matches!(run.status(), RunStatus::CompletedWithErrors));
    let (m, run) = case(503, ("on_failure", "continue")).await;
    assert_eq!(
        audit(&m),
        [
            entry("w", "t", 503),
            entry("w", "after", 200),
            entry("w2", "next", 200)
        ]
    );
    assert_eq!(errors(&m), failed);
    assert!(matches!(run.status(), RunStatus::CompletedWithErrors));
    assert_eq!(data(&m), json!({"wrote": 503, "after": true, "next": true}));
}

#[tokio::test]
async fn a_root_write_takes_only_an_object_and_a_root_cannot_be_removed() {
    // R52, as `Path::set` and `Path::remove` apply it: a `Config` error.
    for (writes, text) in [
        (
            json!({"data": 5}),
            "Task t error: `data` is a root and takes only an object, not a number",
        ),
        (
            json!({"-data": null}),
            "Task t error: `data` is a root and cannot be removed",
        ),
    ] {
        let (e, _) = engine(&[workflow(
            "w",
            &[],
            vec![probe("t", json!([]), writes, json!("ok"))],
        )]);
        let mut m = message(json!({"kept": true}));
        let run = e.run(&mut m).await;
        let RunStatus::Stopped(error) = run.status() else {
            panic!("{:?}", run.status());
        };
        assert_eq!(error.kind(), &ErrorKind::Config);
        assert_eq!(errors(&m), [("CONFIG_ERROR".to_owned(), text.to_owned())]);
        assert_eq!(data(&m), json!({"kept": true}));
    }

    // An object replaces the root; a key below it is removed.
    let (e, _) = engine(&[workflow(
        "w",
        &[],
        vec![
            probe(
                "t",
                json!([]),
                json!({"data": {"fresh": true, "gone": 1}}),
                json!("ok"),
            ),
            probe("u", json!([]), json!({"-data.gone": null}), json!("ok")),
        ],
    )]);
    let mut m = message(json!({"kept": true}));
    let run = e.run(&mut m).await;
    assert!(
        matches!(run.status(), RunStatus::Completed),
        "{:?}",
        run.status()
    );
    assert_eq!(data(&m), json!({"fresh": true}));
    assert_eq!(
        changes(&m, "t"),
        json!([{"path": "data", "old_value": {"kept": true},
                "new_value": {"fresh": true, "gone": 1}}])
    );
    assert_eq!(
        changes(&m, "u"),
        json!([{"path": "data.gone", "old_value": 1, "new_value": null, "removed": true}])
    );
}

#[tokio::test]
async fn the_run_layers_wrap_the_registry_layers_which_wrap_the_function() {
    // The order `function::layer`'s unit test pins, end to end: the run's
    // layers outermost, then the registry's, each list first-added outermost.
    let seen = Shared::default();
    let e = Engine::builder()
        .register("probe", Probe(Arc::clone(&seen)))
        .layer(Named("registry-outer", Arc::clone(&seen)))
        .layer(Named("registry-inner", Arc::clone(&seen)))
        .workflow_json(&workflow(
            "w",
            &[],
            vec![probe("t", json!([]), json!({}), json!("ok"))],
        ))
        .build()
        .unwrap();
    let run_layers: Vec<Arc<dyn CallLayer>> = vec![
        Arc::new(Named("run-outer", Arc::clone(&seen))),
        Arc::new(Named("run-inner", Arc::clone(&seen))),
    ];
    let mut m = message(json!({}));
    let run = e
        .run_with(&mut m, &RunOptions::default().layers(&run_layers))
        .await;
    assert!(matches!(run.status(), RunStatus::Completed));
    assert_eq!(
        seen.lock().unwrap().events,
        [
            "run-outer",
            "run-inner",
            "registry-outer",
            "registry-inner",
            "function",
            "registry-inner",
            "registry-outer",
            "run-inner",
            "run-outer",
        ]
    );
    // A run without run layers still has the registry's.
    seen.lock().unwrap().events.clear();
    e.run(&mut message(json!({}))).await;
    assert_eq!(
        seen.lock().unwrap().events,
        [
            "registry-outer",
            "registry-inner",
            "function",
            "registry-inner",
            "registry-outer"
        ]
    );
}

#[tokio::test]
async fn call_info_carries_the_program_indices_ids_channel_and_counter() {
    let (e, seen) = engine(&[
        workflow(
            "first",
            &[("channel", json!("orders"))],
            vec![map("a", "data.a", json!(1)), map("b", "data.b", json!(2))],
        ),
        workflow(
            "second",
            &[
                ("channel", json!("orders")),
                ("priority", json!(1)),
                ("loop", json!({"counter": "i", "max": 2})),
            ],
            vec![
                map("c", "data.c", json!(3)),
                probe("p", json!([]), json!({}), json!("ok")),
            ],
        ),
    ]);
    let program = e.program();
    let second = program
        .workflows()
        .iter()
        .find(|w| w.id == "second")
        .unwrap()
        .ix;
    let task = program
        .tasks()
        .iter()
        .find(|t| t.id == "p" && t.workflow == second)
        .unwrap()
        .ix;
    assert_eq!(program.task(task).function, "probe");
    let info = |channel: Option<&str>, loop_counter: i64| Info {
        program: program.id().index(),
        workflow: second.index(),
        task: task.index(),
        function: "probe".to_owned(),
        workflow_id: "second".to_owned(),
        task_id: "p".to_owned(),
        channel: channel.map(str::to_owned),
        loop_counter: Some(loop_counter),
        element_index: None,
    };

    let mut m = message(json!({}));
    e.run_with(&mut m, &RunOptions::default().channel("orders"))
        .await;
    assert_eq!(
        seen.lock().unwrap().infos,
        [info(Some("orders"), 0), info(Some("orders"), 1)]
    );
    // The indices are the program's: the probe's task comes after the
    // first workflow's two and the loop's `map`.
    assert_eq!((second.index(), task.index()), (1, 3));

    seen.lock().unwrap().infos.clear();
    e.run(&mut message(json!({}))).await;
    assert_eq!(seen.lock().unwrap().infos, [info(None, 0), info(None, 1)]);
}

#[test]
fn the_run_future_is_send_with_a_host_call_and_layers() {
    fn assert_send<T: Send>(_: &T) {}
    let seen = Shared::default();
    let e = Engine::builder()
        .register("probe", Probe(Arc::clone(&seen)))
        .layer(Named("registry", Arc::clone(&seen)))
        .workflow_json(&workflow(
            "w",
            &[],
            vec![probe("t", json!([]), json!({"data.a": 1}), json!("ok"))],
        ))
        .build()
        .unwrap();
    let run_layers: Vec<Arc<dyn CallLayer>> = vec![Arc::new(Named("run", Arc::clone(&seen)))];
    let opts = RunOptions::default().layers(&run_layers);
    let mut m = message(json!({}));
    assert_send(&e.run_with(&mut m, &opts));
}

/// Two outputs, the literal one primary.
#[derive(FunctionConfig)]
struct EmitConfig {
    #[output(primary)]
    literal: Output,
    computed: Output,
    value: Expr,
}

/// Emits `value` through `literal`, then through `computed`.
struct Emit;

impl Function for Emit {
    type Config = EmitConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &EmitConfig) -> Result<Outcome> {
        let value = ctx.eval(&cfg.value)?.into_owned();
        ctx.emit(&cfg.literal, value.clone())?;
        ctx.emit(&cfg.computed, value)?;
        Ok(Outcome::Ok)
    }
}

#[tokio::test]
async fn emit_writes_through_a_literal_and_a_computed_output() {
    // The computed output is evaluated when it is emitted through, against
    // the context as the call sees it: after its first write.
    let e = Engine::builder()
        .register("emit", Emit)
        .workflow_json(&workflow(
            "w",
            &[],
            vec![json!({"id": "t", "function": {"name": "emit", "input": {
                "literal": "data.lit",
                "computed": {"cat": ["data.", {"var": "data.lit"}]},
                "value": {"var": "data.v"}}}})],
        ))
        .build()
        .unwrap();
    let mut m = message(json!({"v": "dyn"}));
    let run = e.run(&mut m).await;
    assert!(matches!(run.status(), RunStatus::Completed));
    assert_eq!(data(&m), json!({"v": "dyn", "lit": "dyn", "dyn": "dyn"}));
    assert_eq!(
        changes(&m, "t"),
        json!([
            {"path": "data.lit", "old_value": null, "new_value": "dyn"},
            {"path": "data.dyn", "old_value": null, "new_value": "dyn"}
        ])
    );
}

/// Two outputs, one marked primary.
#[derive(FunctionConfig)]
struct TwoOutputs {
    #[output(primary)]
    main: Output,
    side: Output,
}

/// One output: the primary without saying so.
#[derive(FunctionConfig)]
struct OneOutput {
    out: Output,
}

/// One optional output: the primary without saying so, when the input sets
/// it.
#[derive(FunctionConfig)]
struct MaybeOutput {
    out: Option<Output>,
}

/// Emits its own name through its config's primary output.
struct EmitDefault<C>(PhantomData<fn() -> C>);

impl<C> EmitDefault<C> {
    fn new() -> Self {
        Self(PhantomData)
    }
}

impl<C: FunctionConfig> Function for EmitDefault<C> {
    type Config = C;

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &C) -> Result<Outcome> {
        let name = OwnedDataValue::from(ctx.info().function);
        ctx.emit_default(name)?;
        Ok(Outcome::Ok)
    }
}

#[tokio::test]
async fn emit_default_writes_the_primary_output_and_fails_without_one() {
    let task =
        |name: &str, input: Value| json!({"id": name, "function": {"name": name, "input": input}});
    let e = Engine::builder()
        .register("two", EmitDefault::<TwoOutputs>::new())
        .register("one", EmitDefault::<OneOutput>::new())
        .register("maybe", EmitDefault::<MaybeOutput>::new())
        // `()` is hand-written and has no output.
        .register("none", EmitDefault::<()>::new())
        .workflow_json(&workflow(
            "w",
            &[],
            vec![
                task("two", json!({"main": "data.main", "side": "data.side"})),
                task("one", json!({"out": "data.one"})),
                task("maybe", json!({"out": "data.maybe"})),
                task("none", json!({})),
                map("after", "data.after", true.into()),
            ],
        ))
        .build()
        .unwrap();
    let mut m = message(json!({}));
    let run = e.run(&mut m).await;
    assert_eq!(
        data(&m),
        json!({"main": "two", "one": "one", "maybe": "maybe"})
    );
    assert_eq!(
        audit(&m),
        [
            entry("w", "two", 200),
            entry("w", "one", 200),
            entry("w", "maybe", 200),
            entry("w", "none", 500)
        ]
    );
    let RunStatus::Stopped(error) = run.status() else {
        panic!("{:?}", run.status());
    };
    assert_eq!(error.kind(), &ErrorKind::Config);
    assert_eq!(
        errors(&m),
        [(
            "CONFIG_ERROR".to_owned(),
            "Task none error: `none` has no primary output".to_owned()
        )]
    );

    // An optional primary output the input left unset is named.
    let e = Engine::builder()
        .register("maybe", EmitDefault::<MaybeOutput>::new())
        .workflow_json(&workflow("w", &[], vec![task("maybe", json!({}))]))
        .build()
        .unwrap();
    let mut m = message(json!({}));
    e.run(&mut m).await;
    assert_eq!(
        errors(&m),
        [(
            "CONFIG_ERROR".to_owned(),
            "Task maybe error: `maybe`'s primary output `out` is not set".to_owned()
        )]
    );
}

/// One output, with a default.
#[derive(FunctionConfig)]
struct AnswerConfig {
    #[output(default = "data.answer")]
    out: Output,
}

/// Writes, logs that it ran, and fails.
struct Fails(Shared);

impl Function for Fails {
    type Config = AnswerConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &AnswerConfig) -> Result<Outcome> {
        self.0.lock().unwrap().events.push("fails".to_owned());
        ctx.set(&"data.partial".parse()?, true.into())?;
        Err(Error::service("PARTNER_DOWN", "down"))
    }
}

#[tokio::test]
async fn an_answering_layer_writes_the_primary_output_and_the_function_never_runs() {
    let seen = Shared::default();
    let e = Engine::builder()
        .register("fails", Fails(Arc::clone(&seen)))
        .register("probe", Probe(Arc::clone(&seen)))
        .layer(Named("registry", Arc::clone(&seen)))
        .workflow_json(&workflow(
            "w",
            &[],
            vec![
                json!({"id": "f", "function": {"name": "fails", "input": {}}}),
                probe("p", json!([]), json!({"data.b": 1}), json!("ok")),
            ],
        ))
        .build()
        .unwrap();
    // A dry run answers `fails` and passes every other call on.
    let answer = layers::Answer::new(|info| {
        (info.function == "fails").then(|| OwnedDataValue::from("answered"))
    });
    assert_eq!(format!("{answer:?}"), "Answer { .. }");
    let run_layers: Vec<Arc<dyn CallLayer>> = vec![Arc::new(answer)];
    let mut m = message(json!({}));
    let run = e
        .run_with(&mut m, &RunOptions::default().layers(&run_layers))
        .await;
    assert!(
        matches!(run.status(), RunStatus::Completed),
        "{:?}",
        run.status()
    );
    assert_eq!(data(&m), json!({"answer": "answered", "b": 1}));
    assert_eq!(audit(&m), [entry("w", "f", 200), entry("w", "p", 200)]);
    assert_eq!(
        changes(&m, "f"),
        json!([{"path": "data.answer", "old_value": null, "new_value": "answered"}])
    );
    assert!(m.errors().is_empty());
    // Neither the registry's layer nor the function ran for the answered
    // call; the probe went through both.
    assert_eq!(
        seen.lock().unwrap().events,
        ["registry", "function", "registry"]
    );
}

/// What a templated function read of its config, one entry per call.
type Templates = Arc<Mutex<Vec<(Vec<(&'static str, Option<Value>)>, Value)>>>;

/// A plugin function: its fields are declared at runtime. Each call reads
/// every field by name, then the whole config, which it emits through its
/// primary output.
struct Plugin(Templates);

impl Function for Plugin {
    type Config = Templated;

    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default().with_runtime_fields(vec![
            FieldSpec::runtime("expr", FieldKind::Expr(ValueKind::Any), true),
            FieldSpec::runtime("doc", FieldKind::Interpolated, true),
            FieldSpec::runtime("lit", FieldKind::Literal(ValueKind::String), true),
            FieldSpec::runtime("unset", FieldKind::Literal(ValueKind::String), false),
            FieldSpec::runtime("out", FieldKind::Output, true),
        ])
    }

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &Templated) -> Result<Outcome> {
        let mut got = Vec::new();
        for name in ["expr", "doc", "lit", "out", "unset", "undeclared"] {
            got.push((name, cfg.get(ctx, name)?.map(|v| Value::from(&v))));
        }
        let resolved = cfg.resolved(ctx)?;
        self.0.lock().unwrap().push((got, Value::from(&resolved)));
        ctx.emit_default(resolved)?;
        Ok(Outcome::Ok)
    }
}

#[tokio::test]
async fn a_templated_config_reads_its_fields_per_call() {
    let seen = Templates::default();
    let e = Engine::builder()
        .register("plugin", Plugin(Arc::clone(&seen)))
        .workflow_json(&workflow(
            "w",
            &[("loop", json!({"counter": "i", "max": 2}))],
            vec![json!({"id": "t", "function": {"name": "plugin", "input": {
                "expr": {"+": [{"var": "temp_data.i"}, 10]},
                "doc": {"$n": {"var": "temp_data.i"}, "k": "v"},
                "lit": "label",
                "out": {"cat": ["data.out_", {"var": "temp_data.i"}]}}}})],
        ))
        .build()
        .unwrap();
    let mut m = message(json!({}));
    let run = e.run(&mut m).await;
    assert!(
        matches!(run.status(), RunStatus::Completed),
        "{:?}",
        run.status()
    );
    // Each call: an expression evaluated, a document rendered, a literal,
    // an output's path, and nothing for a field the input did not set or a
    // name no field has.
    let call = |i: i64| {
        let got = vec![
            ("expr", Some(json!(i + 10))),
            ("doc", Some(json!({"$n": i, "k": "v"}))),
            ("lit", Some(json!("label"))),
            ("out", Some(json!(format!("data.out_{i}")))),
            ("unset", None),
            ("undeclared", None),
        ];
        let resolved = json!({"expr": i + 10, "doc": {"$n": i, "k": "v"}, "lit": "label",
            "out": format!("data.out_{i}")});
        (got, resolved)
    };
    assert_eq!(*seen.lock().unwrap(), [call(0), call(1)]);
    // `resolved` keeps the declaration order, and the single `Output` field
    // is the primary output each call emitted through.
    let out: Path = "data.out_1".parse().unwrap();
    assert_eq!(
        serde_json::to_string(m.context().get(&out).unwrap()).unwrap(),
        r#"{"expr":11,"doc":{"$n":1,"k":"v"},"lit":"label","out":"data.out_1"}"#
    );
    assert_eq!(data(&m), json!({"out_0": call(0).1, "out_1": call(1).1}));
}

#[tokio::test]
async fn a_call_without_capture_writes_as_it_does_with_and_records_no_change() {
    // `Ctx::set` and `Ctx::remove` under `capture_changes(false)`: the same
    // writes land, a removal of nothing is still nothing, and the entry has
    // no `Change`.
    let (e, _) = engine(&[workflow(
        "w",
        &[],
        vec![probe(
            "p",
            json!([]),
            json!({
                "-data.gone": null,
                "-data.absent": null,
                "data.a": 1,
                "data.b": {"var": "data.a"}
            }),
            json!("ok"),
        )],
    )]);
    let run_with = |capture: bool| {
        let e = &e;
        async move {
            let mut m = message(json!({"gone": "x", "kept": true}));
            let opts = RunOptions::default().capture_changes(capture);
            let run = e.run_with(&mut m, &opts).await;
            assert!(matches!(run.status(), RunStatus::Completed));
            m
        }
    };
    let on = run_with(true).await;
    let off = run_with(false).await;
    assert_eq!(data(&off), json!({"kept": true, "a": 1, "b": 1}));
    assert_eq!(data(&off), data(&on));
    assert_eq!(audit(&off), [entry("w", "p", 200)]);
    assert_eq!(changes(&off, "p"), json!([]));
    assert_eq!(
        changes(&on, "p"),
        json!([
            {"path": "data.gone", "old_value": "x", "new_value": null, "removed": true},
            {"path": "data.a", "old_value": null, "new_value": 1},
            {"path": "data.b", "old_value": null, "new_value": 1}
        ])
    );
}

#[tokio::test]
async fn a_host_halt_keeps_its_writes_and_halts_the_workflow() {
    // `Outcome::Halt` is an `Ok`: the writes land, the entry records 299,
    // and the workflow halts while the next one runs.
    let (e, _) = engine(&[
        workflow(
            "w",
            &[],
            vec![
                probe("h", json!([]), json!({"data.h": true}), json!("halt")),
                map("after", "data.after", true.into()),
            ],
        ),
        workflow(
            "w2",
            &[("priority", json!(1))],
            vec![map("next", "data.next", true.into())],
        ),
    ]);
    let mut m = message(json!({}));
    let run = e.run(&mut m).await;
    assert_eq!(data(&m), json!({"h": true, "next": true}));
    assert_eq!(audit(&m), [entry("w", "h", 299), entry("w2", "next", 200)]);
    assert_eq!(
        changes(&m, "h"),
        json!([{"path": "data.h", "old_value": null, "new_value": true}])
    );
    assert_eq!(
        exits(&run),
        [Some(WorkflowExit::Halted), Some(WorkflowExit::Completed)]
    );
    assert!(m.errors().is_empty());
    assert!(matches!(run.status(), RunStatus::Completed));
}

/// What `Peek` saw: the payload, and two secrets by name.
type Peeked = Arc<Mutex<Vec<(Value, Option<Value>, Option<Value>)>>>;

/// Reads the payload and the store's secrets, and copies the payload's
/// order id into `data`.
struct Peek(Peeked);

impl Function for Peek {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        let payload = Value::from(ctx.payload());
        let key = ctx.secret("api.key").map(Value::from);
        let absent = ctx.secret("api.absent").map(Value::from);
        let order = OwnedDataValue::from(&payload["order_id"]);
        self.0.lock().unwrap().push((payload, key, absent));
        ctx.set(&"data.order_id".parse()?, order)?;
        Ok(Outcome::Ok)
    }
}

#[tokio::test]
async fn a_host_reads_the_payload_and_the_engines_secrets() {
    let secrets =
        Secrets::new(OwnedDataValue::from(&json!({"api": {"key": "s3cr3t"}}))).expect("an object");
    let exprs = ExprEngine::builder()
        .secrets(secrets)
        .build()
        .expect("the expression engine builds");
    let seen = Peeked::default();
    let e = Engine::builder()
        .registry(Registry::new(exprs))
        .register("peek", Peek(Arc::clone(&seen)))
        .workflow_json(&workflow(
            "w",
            &[],
            vec![json!({"id": "t", "function": {"name": "peek"}})],
        ))
        .build()
        .unwrap();
    let payload = json!({"order_id": "ORD-7"});
    let mut m = Message::from_parts(
        OwnedDataValue::from(&payload),
        OwnedDataValue::from(&json!({})),
        OwnedDataValue::Null,
    );
    let run = e.run(&mut m).await;
    assert!(matches!(run.status(), RunStatus::Completed));
    assert_eq!(
        *seen.lock().unwrap(),
        [(payload, Some(json!("s3cr3t")), None)]
    );
    assert_eq!(data(&m), json!({"order_id": "ORD-7"}));
    // What the function did with the secret is its own; the engine wrote
    // it nowhere.
    assert!(!m.to_json().to_string().contains("s3cr3t"));
}
