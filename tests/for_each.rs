//! Task-level `for_each` (#61): one task's handler runs once per element of an
//! array — isolated calls, folded back in element order, results in `into`.

use async_trait::async_trait;
use dataflow_rs::engine::functions::AsyncFunctionHandler;
use dataflow_rs::engine::message::Message;
use dataflow_rs::{
    Engine, ExecutionObserver, IssueCode, Result, TaskContext, TaskEvent, TaskOutcome, Template,
    TemplateCompiler, Workflow,
};
use datavalue::OwnedDataValue;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod common;

use common::dv;

// =============================================================================
// Handlers
// =============================================================================

/// Writes `temp_data.out = {"id": <p.id>, "i": <index>}`. An element with
/// `fail: true` returns status 500 (after writing), one with `err: true`
/// returns an `Err`, one with `halt: true` returns `Halt`, one with
/// `warn: true` records an error through `add_error` and succeeds.
struct Echo;

#[async_trait]
impl AsyncFunctionHandler for Echo {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        let p = Value::from(ctx.get("temp_data.p").unwrap_or(&OwnedDataValue::Null));
        let i = Value::from(
            ctx.get("temp_data.p_index")
                .unwrap_or(&OwnedDataValue::Null),
        );
        assert_eq!(
            ctx.element_index().map(|x| json!(x)),
            Some(i.clone()),
            "the accessor and the binding agree"
        );
        if p["err"] == json!(true) {
            ctx.set("temp_data.partial", dv(json!(true)));
            return Err(dataflow_rs::DataflowError::Task("boom".into()));
        }
        if p["halt"] == json!(true) {
            return Ok(TaskOutcome::Halt);
        }
        ctx.set("temp_data.out", dv(json!({"id": p["id"], "i": i})));
        if p["warn"] == json!(true) {
            ctx.add_error(dataflow_rs::ErrorInfo::builder("WARNED", "careful").build());
        }
        if p["fail"] == json!(true) {
            return Ok(TaskOutcome::Status(500));
        }
        Ok(TaskOutcome::Success)
    }
}

/// Reads `temp_data.seen`, writes it back plus one. Isolation means every call
/// sees the value from before the fan-out.
struct Increment;

#[async_trait]
impl AsyncFunctionHandler for Increment {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        let seen = ctx
            .get("temp_data.seen")
            .and_then(|v| Value::from(v).as_i64())
            .unwrap_or(0);
        ctx.set("temp_data.seen", dv(json!(seen + 1)));
        Ok(TaskOutcome::Success)
    }
}

/// Tracks calls in flight and their peak; sleeps for `p.sleep_ms` (default
/// 10) so calls overlap, then writes `temp_data.out = p.id`.
#[derive(Default)]
struct Slow {
    in_flight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

#[async_trait]
impl AsyncFunctionHandler for Slow {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        let p = Value::from(ctx.get("temp_data.p").unwrap_or(&OwnedDataValue::Null));
        let ms = p["sleep_ms"].as_u64().unwrap_or(10);
        tokio::time::sleep(Duration::from_millis(ms)).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        ctx.set("temp_data.out", dv(p["id"].clone()));
        Ok(TaskOutcome::Success)
    }
}

/// The issue's `model_infer` shape: a `Template` input resolved per call, and
/// an `output` path the handler writes to.
#[derive(Deserialize)]
struct InferInput {
    model: Template,
    output: String,
}

struct Infer;

#[async_trait]
impl AsyncFunctionHandler for Infer {
    type Input = InferInput;

    fn compile_input(input: &mut InferInput, c: &TemplateCompiler) -> Result<()> {
        input.model.compile(c, "model")
    }

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &InferInput) -> Result<TaskOutcome> {
        let model = input.model.resolve_string(ctx)?;
        ctx.set(&input.output, dv(json!(format!("move-by-{model}"))));
        Ok(TaskOutcome::Success)
    }
}

// =============================================================================
// Fixtures
// =============================================================================

/// A one-task workflow: task `t` carries `for_each` and calls `function`,
/// with `extra` spliced into the task. A trailing `after` task records that
/// the workflow carried on.
fn workflow(for_each: Value, function: &str, extra: Value) -> Workflow {
    let mut task = json!({
        "id": "t", "name": "t", "for_each": for_each,
        "function": {"name": function, "input": {}}
    });
    for (k, v) in extra.as_object().unwrap() {
        task[k] = v.clone();
    }
    Workflow::from_json(
        &json!({"id": "w", "name": "w", "tasks": [
            task,
            {"id": "after", "name": "after", "function": {"name": "map", "input": {"mappings": [
                {"path": "data.after_ran", "logic": true}]}}}
        ]})
        .to_string(),
    )
    .unwrap()
}

fn engine(workflows: Vec<Workflow>) -> Engine {
    Engine::builder()
        .with_workflows(workflows)
        .register("echo", Echo)
        .register("increment", Increment)
        .register("infer", Infer)
        .build()
        .expect("engine should build")
}

fn collect_into() -> Value {
    json!({"over": {"var": "data.ps"}, "as": "p",
           "collect": "temp_data.out", "into": "data.outs"})
}

fn message(ps: Value) -> Message {
    Message::builder().data(dv(json!({"ps": ps}))).build()
}

fn element_indices(m: &Message, task: &str) -> Vec<Option<usize>> {
    m.audit_trail()
        .iter()
        .filter(|e| e.task_id.as_ref() == task)
        .map(|e| e.element_index)
        .collect()
}

fn data(m: &Message, key: &str) -> Value {
    Value::from(&m.context["data"][key])
}

// =============================================================================
// Results, bindings and records
// =============================================================================

#[tokio::test]
async fn results_land_in_element_order_and_bindings_do_not_leak() {
    let e = engine(vec![workflow(collect_into(), "echo", json!({}))]);
    let mut m = message(json!([{"id": "a"}, {"id": "b"}, {"id": "c"}]));
    e.process_message(&mut m).await.unwrap();

    assert_eq!(
        data(&m, "outs"),
        json!([{"id": "a", "i": 0}, {"id": "b", "i": 1}, {"id": "c", "i": 2}])
    );
    assert_eq!(element_indices(&m, "t"), vec![Some(0), Some(1), Some(2)]);
    assert!(
        m.context["temp_data"].get("p").is_none()
            && m.context["temp_data"].get("p_index").is_none(),
        "the bindings live only in each call's copy"
    );
    assert_eq!(
        Value::from(&m.context["temp_data"]["out"]),
        json!({"id": "c", "i": 2}),
        "collect's own writes are replayed, so it holds the last element's value"
    );
    assert_eq!(data(&m, "after_ran"), json!(true));
    assert_eq!(
        Value::from(&m.context["metadata"]["progress"]),
        json!({"workflow_id": "w", "task_id": "after", "status_code": 200})
    );
}

#[tokio::test]
async fn an_ordinary_task_records_no_element_index() {
    let e = engine(vec![workflow(collect_into(), "echo", json!({}))]);
    let mut m = message(json!([{"id": "a"}]));
    e.process_message(&mut m).await.unwrap();
    assert_eq!(element_indices(&m, "after"), vec![None]);
    let json = serde_json::to_value(&m.audit_trail()[1]).unwrap();
    assert!(json.get("element_index").is_none(), "wire shape unchanged");
}

#[tokio::test]
async fn audit_changes_carry_each_elements_writes_and_its_result_slot() {
    let e = engine(vec![workflow(collect_into(), "echo", json!({}))]);
    let mut m = message(json!([{"id": "a"}, {"id": "b"}]));
    e.process_message(&mut m).await.unwrap();
    let paths: Vec<Vec<String>> = m
        .audit_trail()
        .iter()
        .filter(|e| e.task_id.as_ref() == "t")
        .map(|e| e.changes.iter().map(|c| c.path.to_string()).collect())
        .collect();
    assert_eq!(
        paths,
        vec![
            vec!["temp_data.out".to_string(), "data.outs.0".to_string()],
            vec!["temp_data.out".to_string(), "data.outs.1".to_string()],
        ]
    );
}

#[tokio::test]
async fn calls_are_isolated_from_each_other() {
    let e = engine(vec![workflow(
        json!({"over": {"var": "data.ps"}, "as": "p"}),
        "increment",
        json!({}),
    )]);
    let mut m = Message::builder()
        .data(dv(json!({"ps": [1, 2, 3]})))
        .temp_data(dv(json!({"seen": 10})))
        .build();
    e.process_message(&mut m).await.unwrap();
    assert_eq!(
        Value::from(&m.context["temp_data"]["seen"]),
        json!(11),
        "every call saw 10; the writes were replayed in order, each writing 11"
    );
}

#[tokio::test]
async fn the_template_input_resolves_per_call() {
    let wf = Workflow::from_json(
        &json!({"id": "w", "name": "w", "tasks": [{
            "id": "t", "name": "t",
            "for_each": {"over": {"var": "data.ps"}, "as": "p",
                         "collect": "temp_data.move", "into": "temp_data.moves"},
            "function": {"name": "infer", "input": {
                "model": {"var": "temp_data.p.model"}, "output": "temp_data.move"}}
        }]})
        .to_string(),
    )
    .unwrap();
    let e = engine(vec![wf]);
    let mut m = message(json!([{"model": "m1"}, {"model": "m2"}]));
    e.process_message(&mut m).await.unwrap();
    assert_eq!(
        Value::from(&m.context["temp_data"]["moves"]),
        json!(["move-by-m1", "move-by-m2"])
    );
}

#[tokio::test]
async fn without_collect_and_into_the_writes_are_simply_replayed() {
    let e = engine(vec![workflow(
        json!({"over": {"var": "data.ps"}, "as": "p"}),
        "echo",
        json!({}),
    )]);
    let mut m = message(json!([{"id": "a"}, {"id": "b"}]));
    e.process_message(&mut m).await.unwrap();
    assert_eq!(element_indices(&m, "t"), vec![Some(0), Some(1)]);
    assert_eq!(
        Value::from(&m.context["temp_data"]["out"]),
        json!({"id": "b", "i": 1})
    );
}

// =============================================================================
// Empty and failing `over`
// =============================================================================

#[tokio::test]
async fn an_empty_over_writes_an_empty_array_and_one_record() {
    let e = engine(vec![workflow(collect_into(), "echo", json!({}))]);
    let mut m = message(json!([]));
    e.process_message(&mut m).await.unwrap();
    assert_eq!(data(&m, "outs"), json!([]));
    assert_eq!(
        element_indices(&m, "t"),
        vec![None],
        "one record, no element"
    );
    assert_eq!(m.audit_trail()[0].status, 200);
}

#[tokio::test]
async fn a_non_array_over_is_one_task_error() {
    for ps in [json!("nope"), json!(null), json!({"a": 1})] {
        let e = engine(vec![workflow(collect_into(), "echo", json!({}))]);
        let mut m = message(ps.clone());
        assert!(e.process_message(&mut m).await.is_err(), "{ps}");
        assert_eq!(element_indices(&m, "t"), vec![None]);
        assert_eq!(m.audit_trail()[0].status, 500);
        assert!(
            m.errors()
                .iter()
                .any(|x| x.message.contains("for_each.over")),
            "{:?}",
            m.errors()
        );
        assert_eq!(m.context["data"].get("after_ran"), None);

        // With continue_on_error the error is recorded and the workflow goes on.
        let e = engine(vec![workflow(
            collect_into(),
            "echo",
            json!({"continue_on_error": true}),
        )]);
        let mut m = message(ps);
        e.process_message(&mut m).await.unwrap();
        assert_eq!(data(&m, "after_ran"), json!(true));
    }
}

#[tokio::test]
async fn the_condition_is_evaluated_once_and_a_false_one_runs_nothing() {
    let e = engine(vec![workflow(
        collect_into(),
        "echo",
        json!({"condition": {"var": "data.go"}}),
    )]);
    let mut m = message(json!([{"id": "a"}, {"id": "b"}]));
    e.process_message(&mut m).await.unwrap();
    assert!(element_indices(&m, "t").is_empty());
    assert_eq!(m.context["data"].get("outs"), None);
}

// =============================================================================
// Failures and halting
// =============================================================================

#[tokio::test]
async fn a_failed_element_leaves_null_under_continue_on_error() {
    let e = engine(vec![workflow(
        collect_into(),
        "echo",
        json!({"continue_on_error": true}),
    )]);
    let mut m = message(json!([{"id": "a"}, {"id": "b", "fail": true}, {"id": "c"}]));
    e.process_message(&mut m).await.unwrap();

    assert_eq!(
        data(&m, "outs"),
        json!([{"id": "a", "i": 0}, null, {"id": "c", "i": 2}])
    );
    let status_errors: Vec<Option<usize>> = m
        .errors()
        .iter()
        .filter(|x| x.code == "TASK_STATUS_ERROR")
        .map(|x| x.element_index)
        .collect();
    assert_eq!(status_errors, vec![Some(1)]);
    assert_eq!(data(&m, "after_ran"), json!(true));
}

#[tokio::test]
async fn a_hard_error_contributes_no_writes() {
    let e = engine(vec![workflow(
        collect_into(),
        "echo",
        json!({"continue_on_error": true}),
    )]);
    let mut m = message(json!([{"id": "a", "err": true}, {"id": "b"}]));
    e.process_message(&mut m).await.unwrap();
    assert_eq!(data(&m, "outs"), json!([null, {"id": "b", "i": 1}]));
    assert_eq!(
        m.context["temp_data"].get("partial"),
        None,
        "a call that returned Err leaves nothing behind"
    );
    assert_eq!(
        m.errors()
            .iter()
            .find(|x| x.task_id.as_deref() == Some("t"))
            .unwrap()
            .element_index,
        Some(0)
    );
}

#[tokio::test]
async fn a_failed_element_fails_the_task_without_continue_on_error() {
    let e = engine(vec![workflow(collect_into(), "echo", json!({}))]);
    let mut m = message(json!([{"id": "a"}, {"id": "b", "fail": true}, {"id": "c"}]));
    assert!(e.process_message(&mut m).await.is_err());
    assert_eq!(
        data(&m, "outs"),
        json!([{"id": "a", "i": 0}, null, null]),
        "`a` folded, `b` failed the task, `c` never started"
    );
    assert_eq!(element_indices(&m, "t"), vec![Some(0), Some(1)]);
    assert_eq!(m.context["data"].get("after_ran"), None);
}

#[tokio::test]
async fn errors_a_call_records_itself_are_stamped_and_kept() {
    let e = engine(vec![workflow(collect_into(), "echo", json!({}))]);
    let mut m = message(json!([{"id": "a"}, {"id": "b", "warn": true}]));
    e.process_message(&mut m).await.unwrap();
    let warned: Vec<Option<usize>> = m
        .errors()
        .iter()
        .filter(|x| x.code == "WARNED")
        .map(|x| x.element_index)
        .collect();
    assert_eq!(warned, vec![Some(1)]);
}

#[tokio::test]
async fn an_element_halt_stops_new_calls_and_halts_the_workflow() {
    let e = engine(vec![workflow(collect_into(), "echo", json!({}))]);
    let mut m = message(json!([{"id": "a"}, {"id": "b", "halt": true}, {"id": "c"}]));
    e.process_message(&mut m).await.unwrap();
    assert_eq!(element_indices(&m, "t"), vec![Some(0), Some(1)]);
    assert_eq!(
        m.context["data"].get("after_ran"),
        None,
        "the workflow halted"
    );
}

#[tokio::test]
async fn halt_on_failure_halts_after_the_whole_fan_out() {
    let e = engine(vec![workflow(
        collect_into(),
        "echo",
        json!({"continue_on_error": true, "halt_on": "failure"}),
    )]);
    let mut m = message(json!([{"id": "a", "fail": true}, {"id": "b"}, {"id": "c"}]));
    e.process_message(&mut m).await.unwrap();
    assert_eq!(
        element_indices(&m, "t"),
        vec![Some(0), Some(1), Some(2)],
        "halt_on applies to the fan-out as a whole, not to its first failure"
    );
    assert_eq!(m.context["data"].get("after_ran"), None);
}

#[tokio::test]
async fn terminal_halts_after_the_whole_fan_out() {
    let e = engine(vec![workflow(
        collect_into(),
        "echo",
        json!({"terminal": true}),
    )]);
    let mut m = message(json!([{"id": "a"}, {"id": "b"}]));
    e.process_message(&mut m).await.unwrap();
    assert_eq!(element_indices(&m, "t"), vec![Some(0), Some(1)]);
    assert_eq!(m.context["data"].get("after_ran"), None);
}

// =============================================================================
// Concurrency
// =============================================================================

fn slow_engine(max_concurrency: usize, slow: Slow) -> Engine {
    Engine::builder()
        .with_workflows(vec![workflow(
            json!({"over": {"var": "data.ps"}, "as": "p", "max_concurrency": max_concurrency,
                   "collect": "temp_data.out", "into": "data.outs"}),
            "slow",
            json!({}),
        )])
        .register("slow", slow)
        .build()
        .unwrap()
}

#[tokio::test]
async fn max_concurrency_bounds_calls_in_flight() {
    for (max, expected_peak) in [(1, 1), (3, 3)] {
        let peak = Arc::new(AtomicUsize::new(0));
        let e = slow_engine(
            max,
            Slow {
                in_flight: Arc::default(),
                peak: Arc::clone(&peak),
            },
        );
        let ps: Vec<Value> = (0..8).map(|i| json!({"id": i})).collect();
        let mut m = message(json!(ps));
        e.process_message(&mut m).await.unwrap();
        assert_eq!(
            peak.load(Ordering::SeqCst),
            expected_peak,
            "max_concurrency {max}"
        );
        assert_eq!(data(&m, "outs"), json!([0, 1, 2, 3, 4, 5, 6, 7]));
    }
}

#[tokio::test]
async fn results_are_identical_at_every_concurrency() {
    // Earlier elements sleep longer, so concurrent calls finish in reverse.
    let ps: Vec<Value> = (0..6)
        .map(|i| json!({"id": i, "sleep_ms": (6 - i) * 5}))
        .collect();
    let mut outs = Vec::new();
    for max in [1, 2, 6] {
        let e = slow_engine(max, Slow::default());
        let mut m = message(json!(ps.clone()));
        e.process_message(&mut m).await.unwrap();
        outs.push((data(&m, "outs"), element_indices(&m, "t")));
    }
    assert_eq!(outs[0], outs[1]);
    assert_eq!(outs[0], outs[2]);
    assert_eq!(outs[0].0, json!([0, 1, 2, 3, 4, 5]));
}

// =============================================================================
// Loops, trace, observer, reload, authoring
// =============================================================================

#[tokio::test]
async fn a_fan_out_inside_a_loop_stamps_both_coordinates() {
    let wf = Workflow::from_json(
        &json!({"id": "w", "name": "w",
                "loop": {"counter": "i", "max": 2},
                "tasks": [{"id": "t", "name": "t",
                           "for_each": {"over": [{"id": "a"}, {"id": "b"}], "as": "p"},
                           "function": {"name": "echo", "input": {}}}]})
        .to_string(),
    )
    .unwrap();
    let e = engine(vec![wf]);
    let mut m = Message::builder().build();
    e.process_message(&mut m).await.unwrap();
    let stamps: Vec<(Option<i64>, Option<usize>)> = m
        .audit_trail()
        .iter()
        .map(|x| (x.loop_counter, x.element_index))
        .collect();
    assert_eq!(
        stamps,
        vec![
            (Some(0), Some(0)),
            (Some(0), Some(1)),
            (Some(1), Some(0)),
            (Some(1), Some(1))
        ]
    );
}

#[tokio::test]
async fn a_fan_out_in_loop_setup_runs_once() {
    let wf = Workflow::from_json(
        &json!({"id": "w", "name": "w",
                "loop": {"counter": "i", "max": 3, "setup": [
                    {"id": "s", "name": "s",
                     "for_each": {"over": [{"id": "a"}, {"id": "b"}], "as": "p",
                                  "collect": "temp_data.out", "into": "temp_data.batch"},
                     "function": {"name": "echo", "input": {}}}]},
                "tasks": [{"id": "t", "name": "t", "function": {"name": "map", "input": {
                    "mappings": [{"path": "data.n", "logic": {"var": "temp_data.i"}}]}}}]})
        .to_string(),
    )
    .unwrap();
    let e = engine(vec![wf]);
    let mut m = Message::builder().build();
    e.process_message(&mut m).await.unwrap();
    assert_eq!(element_indices(&m, "s"), vec![Some(0), Some(1)]);
    assert!(
        m.audit_trail()
            .iter()
            .filter(|x| x.task_id.as_ref() == "s")
            .all(|x| x.loop_counter.is_none()),
        "setup is not a sweep"
    );
    assert_eq!(
        Value::from(&m.context["temp_data"]["batch"]),
        json!([{"id": "a", "i": 0}, {"id": "b", "i": 1}])
    );
}

#[tokio::test]
async fn trace_steps_carry_element_index() {
    let e = engine(vec![workflow(collect_into(), "echo", json!({}))]);
    let mut m = message(json!([{"id": "a"}, {"id": "b"}]));
    let trace = e.process_message_with_trace(&mut m).await.unwrap();
    let steps: Vec<(Option<&str>, Option<usize>)> = trace
        .steps
        .iter()
        .map(|s| (s.task_id.as_deref(), s.element_index))
        .collect();
    assert_eq!(
        steps,
        vec![
            (Some("t"), Some(0)),
            (Some("t"), Some(1)),
            (Some("after"), None)
        ]
    );
    assert!(trace.steps.iter().all(|s| s.duration_us.is_some()));
}

#[derive(Default)]
struct Events(Mutex<Vec<(String, Option<u16>)>>);

impl ExecutionObserver for Events {
    fn task_finished(&self, event: &TaskEvent<'_>) {
        self.0
            .lock()
            .unwrap()
            .push((event.task_id.to_string(), event.status));
    }
}

#[tokio::test]
async fn an_observer_sees_one_event_per_element() {
    let events = Arc::new(Events::default());
    let e = Engine::builder()
        .with_workflows(vec![workflow(
            collect_into(),
            "echo",
            json!({"continue_on_error": true}),
        )])
        .register("echo", Echo)
        .with_observer(Arc::clone(&events) as Arc<dyn ExecutionObserver>)
        .build()
        .unwrap();
    let mut m = message(json!([{"id": "a"}, {"id": "b", "fail": true}]));
    e.process_message(&mut m).await.unwrap();
    assert_eq!(
        events.0.lock().unwrap().clone(),
        vec![
            ("t".to_string(), Some(200)),
            ("t".to_string(), Some(500)),
            ("after".to_string(), Some(200)),
        ]
    );
}

#[tokio::test]
async fn a_hot_reload_recompiles_for_each() {
    let e = engine(vec![workflow(collect_into(), "echo", json!({}))]);
    let reloaded = e
        .with_new_workflows(vec![workflow(collect_into(), "echo", json!({}))])
        .unwrap();
    let mut m = message(json!([{"id": "a"}]));
    reloaded.process_message(&mut m).await.unwrap();
    assert_eq!(data(&m, "outs"), json!([{"id": "a", "i": 0}]));
}

#[tokio::test]
async fn build_refuses_a_for_each_on_a_builtin() {
    let wf = Workflow::from_json(
        &json!({"id": "w", "name": "w", "tasks": [{"id": "t", "name": "t",
            "for_each": {"over": [], "as": "p"},
            "function": {"name": "map", "input": {"mappings": []}}}]})
        .to_string(),
    )
    .unwrap();
    let err = Engine::builder()
        .with_workflows(vec![wf])
        .build()
        .err()
        .expect("refused");
    assert!(err.to_string().contains("handler-backed"), "{err}");
}

#[tokio::test]
async fn check_workflow_refuses_a_secret_in_over() {
    let wf = Workflow::from_json(
        &json!({"id": "w", "name": "w", "tasks": [{"id": "t", "name": "t",
            "for_each": {"over": {"secret": "ps"}, "as": "p"},
            "function": {"name": "echo", "input": {}}}]})
        .to_string(),
    )
    .unwrap();
    let builder = Engine::builder()
        .with_secrets_json(&json!({"ps": [1]}))
        .register("echo", Echo);
    let issues = builder.check_workflow(&wf);
    let issue = issues
        .iter()
        .find(|i| i.code == IssueCode::SecretInMessageWrite)
        .unwrap_or_else(|| panic!("{issues:?}"));
    assert_eq!(issue.path.as_deref(), Some("for_each.over"));
    assert_eq!(issue.task_id.as_deref(), Some("t"));
    assert!(builder.with_workflows(vec![wf]).build().is_err());
}

/// Hosts spawn `process_message` on a multi-threaded runtime, so a fan-out
/// inside it must not make the future `!Send`.
#[test]
fn process_message_stays_send_with_a_fan_out() {
    fn assert_send<T: Send>(_: T) {}
    let e = engine(vec![workflow(collect_into(), "echo", json!({}))]);
    let mut m = message(json!([]));
    assert_send(e.process_message(&mut m));
}
