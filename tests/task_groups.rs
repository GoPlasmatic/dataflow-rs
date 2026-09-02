//! `Task::terminal` and task groups: the guard-clause shape.
//!
//! A step in a workflow's `tasks` list is either a task (carries `function`) or
//! a group (carries `tasks`). Both accept `condition` and `terminal`, which
//! gives `if (…) { … }`, an early `return`, and `if (…) { …; return; }`.
//!
//! A task additionally accepts `halt_on`, the *outcome* axis to `terminal`'s
//! position axis — an early `return` taken only when the task failed. It is
//! task-only: a group has no outcome of its own, and carrying it is refused at
//! parse time.

use async_trait::async_trait;
use dataflow_rs::engine::functions::AsyncFunctionHandler;
use dataflow_rs::engine::message::Message;
use dataflow_rs::engine::trace::TraceOptions;
use dataflow_rs::{Engine, HaltOn, Result, TaskContext, TaskOutcome, Workflow};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

mod common;

use common::dv;

/// Writes `data.<key>` so a test can assert exactly which tasks ran, and counts
/// its own calls. Not a sync built-in, so it also forces the async task path
/// and breaks a sync stretch in two.
#[derive(Debug)]
struct MarkAsync {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl AsyncFunctionHandler for MarkAsync {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &Value) -> Result<TaskOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let key = input
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("async")
            .to_string();
        ctx.set(
            &format!("data.{key}"),
            datavalue::OwnedDataValue::Bool(true),
        );
        Ok(TaskOutcome::Success)
    }
}

/// Returns whatever outcome the test asked for, so `Skip` / `Status(500)` can
/// be paired with `terminal`.
#[derive(Debug)]
struct Outcome(TaskOutcome);

#[async_trait]
impl AsyncFunctionHandler for Outcome {
    type Input = Value;

    async fn execute(&self, _ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        Ok(self.0)
    }
}

/// Always fails, so `terminal` can be tested against a failed task.
#[derive(Debug)]
struct Boom;

#[async_trait]
impl AsyncFunctionHandler for Boom {
    type Input = Value;

    async fn execute(&self, _ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        Err(dataflow_rs::DataflowError::Task("boom".to_string()))
    }
}

/// A `map` task writing `data.<key> = true`, as a JSON step.
fn mark(id: &str) -> String {
    format!(
        r#"{{ "id": "{id}", "name": "{id}", "function": {{ "name": "map",
             "input": {{ "mappings": [ {{ "path": "data.{id}", "logic": true }} ] }} }} }}"#
    )
}

/// Wrap `steps` in a workflow whose condition is always true.
fn workflow(steps: &str) -> Workflow {
    Workflow::from_json(&format!(
        r#"{{ "id": "w", "name": "w", "priority": 0, "tasks": [{steps}] }}"#
    ))
    .expect("workflow should parse")
}

/// Run one message through `engine` and hand back the finished message.
async fn run(engine: &Engine, data: Value) -> Message {
    let mut message = Message::builder().data(dv(data)).build();
    engine
        .process_message(&mut message)
        .await
        .expect("processing should succeed");
    message
}

/// Which of `data.<key>` were written, in the order asked about.
fn ran(message: &Message, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .filter(|k| Value::from(&message.context["data"][**k]) == json!(true))
        .map(|k| (*k).to_string())
        .collect()
}

fn engine_for(workflows: Vec<Workflow>) -> Engine {
    Engine::builder()
        .with_workflows(workflows)
        .build()
        .expect("engine should build")
}

// =============================================================================
// `terminal` on a task
// =============================================================================

#[tokio::test]
async fn terminal_task_stops_the_workflow_when_its_condition_holds() {
    let engine = engine_for(vec![workflow(&format!(
        r#"{},
           {{ "id": "exit", "name": "exit", "terminal": true,
              "condition": {{ "var": "data.stop" }},
              "function": {{ "name": "map", "input": {{ "mappings": [
                  {{ "path": "data.exit", "logic": true }} ] }} }} }},
           {}"#,
        mark("first"),
        mark("last")
    ))]);

    let message = run(&engine, json!({ "stop": true })).await;
    assert_eq!(
        ran(&message, &["first", "exit", "last"]),
        vec!["first", "exit"],
        "the terminal task ran and nothing after it did"
    );
}

#[tokio::test]
async fn terminal_task_with_a_false_condition_does_not_halt() {
    let engine = engine_for(vec![workflow(&format!(
        r#"{},
           {{ "id": "exit", "name": "exit", "terminal": true,
              "condition": {{ "var": "data.stop" }},
              "function": {{ "name": "map", "input": {{ "mappings": [
                  {{ "path": "data.exit", "logic": true }} ] }} }} }},
           {}"#,
        mark("first"),
        mark("last")
    ))]);

    let message = run(&engine, json!({ "stop": false })).await;
    assert_eq!(
        ran(&message, &["first", "exit", "last"]),
        vec!["first", "last"],
        "a guard that did not fire cannot end the workflow"
    );
}

#[tokio::test]
async fn terminal_task_returning_skip_does_not_halt() {
    // `TaskOutcome::Skip` opts out of the per-task record entirely — the task
    // declined to act, so "stop after this task" has nothing to stop after.
    let engine = Engine::builder()
        .with_workflows(vec![workflow(&format!(
            r#"{{ "id": "skipper", "name": "skipper", "terminal": true,
                  "function": {{ "name": "skipper", "input": {{}} }} }},
               {}"#,
            mark("last")
        ))])
        .register("skipper", Outcome(TaskOutcome::Skip))
        .build()
        .expect("engine should build");

    let message = run(&engine, json!({})).await;
    assert_eq!(
        ran(&message, &["last"]),
        vec!["last"],
        "a skipped task does not halt, even when marked terminal"
    );
}

#[tokio::test]
async fn terminal_task_that_failed_under_continue_on_error_still_halts() {
    // `terminal` is about position, not outcome: the author said "nothing after
    // this runs". The error is still recorded.
    let engine = Engine::builder()
        .with_workflows(vec![workflow(&format!(
            r#"{{ "id": "boom", "name": "boom", "terminal": true,
                  "continue_on_error": true,
                  "function": {{ "name": "boom", "input": {{}} }} }},
               {}"#,
            mark("last")
        ))])
        .register("boom", Boom)
        .build()
        .expect("engine should build");

    let message = run(&engine, json!({})).await;
    assert!(
        ran(&message, &["last"]).is_empty(),
        "a failed terminal task still ends the workflow"
    );
    assert_eq!(
        message.errors().len(),
        1,
        "and its error is still on the message"
    );
    assert_eq!(message.errors()[0].task_id.as_deref(), Some("boom"));
}

#[tokio::test]
async fn terminal_task_returning_5xx_still_records_and_propagates() {
    // Regression guard: folding `terminal` into the halt decision *before* the
    // status classification would skip the `TASK_STATUS_ERROR` push and the
    // `Err` return, silently turning a failure into a clean stop.
    let engine = Engine::builder()
        .with_workflows(vec![workflow(
            r#"{ "id": "five_hundred", "name": "five_hundred", "terminal": true,
                 "function": { "name": "five_hundred", "input": {} } }"#,
        )])
        .register("five_hundred", Outcome(TaskOutcome::Status(500)))
        .build()
        .expect("engine should build");

    let mut message = Message::builder().data(dv(json!({}))).build();
    let result = engine.process_message(&mut message).await;

    assert!(
        result.is_err(),
        "a 5xx with continue_on_error unset still stops the engine"
    );
    assert!(
        message
            .errors()
            .iter()
            .any(|e| e.code == "TASK_STATUS_ERROR"),
        "and the status error is still recorded, got {:?}",
        message.errors()
    );
}

#[tokio::test]
async fn terminal_task_keeps_its_own_audit_status() {
    let engine = Engine::builder()
        .with_workflows(vec![workflow(
            r#"{ "id": "not_found", "name": "not_found", "terminal": true,
                 "function": { "name": "not_found", "input": {} } }"#,
        )])
        .register("not_found", Outcome(TaskOutcome::Status(404)))
        .build()
        .expect("engine should build");

    let message = run(&engine, json!({})).await;
    let statuses: Vec<usize> = message.audit_trail().iter().map(|e| e.status).collect();
    assert_eq!(
        statuses,
        vec![404],
        "the task's own status survives; `terminal` is not `HALT_STATUS_CODE`"
    );
}

#[tokio::test]
async fn terminal_halt_stops_only_its_own_workflow() {
    let engine = engine_for(vec![
        Workflow::from_json(&format!(
            r#"{{ "id": "first", "name": "first", "priority": 0, "tasks": [
                 {{ "id": "exit", "name": "exit", "terminal": true,
                    "function": {{ "name": "map", "input": {{ "mappings": [
                        {{ "path": "data.exit", "logic": true }} ] }} }} }},
                 {}
               ] }}"#,
            mark("unreachable")
        ))
        .unwrap(),
        Workflow::from_json(&format!(
            r#"{{ "id": "second", "name": "second", "priority": 1, "tasks": [{}] }}"#,
            mark("later_workflow")
        ))
        .unwrap(),
    ]);

    let message = run(&engine, json!({})).await;
    assert_eq!(
        ran(&message, &["exit", "unreachable", "later_workflow"]),
        vec!["exit", "later_workflow"],
        "halt scopes to the workflow, exactly like TaskOutcome::Halt"
    );
}

#[tokio::test]
async fn terminal_task_breaks_the_whole_loop() {
    let engine = engine_for(vec![
        Workflow::from_json(
            r#"{ "id": "sweeps", "name": "sweeps", "priority": 1,
                 "loop": { "counter": "i", "max": 5 },
                 "tasks": [
                   { "id": "tick", "name": "tick", "function": { "name": "map",
                     "input": { "mappings": [ { "path": "data.ticks",
                       "logic": {"+": [{"var": "data.ticks"}, 1]} } ] } } },
                   { "id": "exit", "name": "exit", "terminal": true,
                     "condition": {">=": [{"var": "temp_data.i"}, 2]},
                     "function": { "name": "map", "input": { "mappings": [
                       { "path": "data.exit", "logic": true } ] } } }
                 ] }"#,
        )
        .unwrap(),
    ]);

    let message = run(&engine, json!({ "ticks": 0 })).await;
    assert_eq!(
        Value::from(&message.context["data"]["ticks"]),
        json!(3),
        "sweeps 0, 1 and 2 ran; the terminal task on sweep 2 broke the loop"
    );
}

// =============================================================================
// `halt_on` on a task — the outcome axis
//
// `terminal` halts whatever the task returned; `halt_on: "failure"` halts only
// when it failed. Failure is a recorded status of 400 or above, which is what
// lets a `validation` task reject without hand-writing the negated condition on
// every task after it.
// =============================================================================

#[tokio::test]
async fn halt_on_failure_halts_on_a_4xx_status() {
    // The `validation` shape: 400 is *not* covered by `continue_on_error`, so
    // without `halt_on` this is the bug in issue #53.
    let engine = Engine::builder()
        .with_workflows(vec![workflow(&format!(
            r#"{{ "id": "check", "name": "check", "halt_on": "failure",
                 "function": {{ "name": "reject", "input": {{}} }} }}, {}"#,
            mark("last")
        ))])
        .register("reject", Outcome(TaskOutcome::Status(400)))
        .build()
        .expect("engine should build");

    let message = run(&engine, json!({})).await;
    assert!(
        ran(&message, &["last"]).is_empty(),
        "a 400 halted the workflow, so nothing after it ran"
    );
}

#[tokio::test]
async fn halt_on_failure_does_not_halt_on_success() {
    // The distinction `terminal` cannot express: a passing assertion falls
    // through to the rest of the pipeline.
    let engine = Engine::builder()
        .with_workflows(vec![workflow(&format!(
            r#"{{ "id": "check", "name": "check", "halt_on": "failure",
                 "function": {{ "name": "pass", "input": {{}} }} }}, {}"#,
            mark("last")
        ))])
        .register("pass", Outcome(TaskOutcome::Success))
        .build()
        .expect("engine should build");

    let message = run(&engine, json!({})).await;
    assert_eq!(
        ran(&message, &["last"]),
        vec!["last"],
        "the task succeeded, so `halt_on` did nothing"
    );
}

#[tokio::test]
async fn halt_on_failure_does_not_halt_on_skip() {
    let engine = Engine::builder()
        .with_workflows(vec![workflow(&format!(
            r#"{{ "id": "skipper", "name": "skipper", "halt_on": "failure",
                 "function": {{ "name": "skipper", "input": {{}} }} }}, {}"#,
            mark("last")
        ))])
        .register("skipper", Outcome(TaskOutcome::Skip))
        .build()
        .expect("engine should build");

    let message = run(&engine, json!({})).await;
    assert_eq!(
        ran(&message, &["last"]),
        vec!["last"],
        "a task that declined to act did not fail"
    );
}

#[tokio::test]
async fn halt_on_failure_with_a_false_condition_does_not_halt() {
    let engine = Engine::builder()
        .with_workflows(vec![workflow(&format!(
            r#"{{ "id": "check", "name": "check", "halt_on": "failure",
                 "condition": false,
                 "function": {{ "name": "reject", "input": {{}} }} }}, {}"#,
            mark("last")
        ))])
        .register("reject", Outcome(TaskOutcome::Status(400)))
        .build()
        .expect("engine should build");

    let message = run(&engine, json!({})).await;
    assert_eq!(
        ran(&message, &["last"]),
        vec!["last"],
        "the task never ran, so it never failed"
    );
}

#[tokio::test]
async fn halt_on_failure_halts_after_a_handler_err_under_continue_on_error() {
    let engine = Engine::builder()
        .with_workflows(vec![workflow(&format!(
            r#"{{ "id": "boom", "name": "boom", "halt_on": "failure",
                 "continue_on_error": true,
                 "function": {{ "name": "boom", "input": {{}} }} }}, {}"#,
            mark("last")
        ))])
        .register("boom", Boom)
        .build()
        .expect("engine should build");

    let message = run(&engine, json!({})).await;
    assert!(
        ran(&message, &["last"]).is_empty(),
        "a handler `Err` is a failure, so the workflow halted"
    );
    assert_eq!(
        message.errors().len(),
        1,
        "the error is still recorded: {:?}",
        message.errors()
    );
}

#[tokio::test]
async fn halt_on_failure_with_5xx_and_no_continue_on_error_still_propagates() {
    // The mirror of `terminal_task_returning_5xx_still_records_and_propagates`:
    // `halt_on` is applied in the fold *after* the status classification, so a
    // 5xx still records `TASK_STATUS_ERROR` and still propagates.
    let engine = Engine::builder()
        .with_workflows(vec![workflow(
            r#"{ "id": "five_hundred", "name": "five_hundred", "halt_on": "failure",
                 "function": { "name": "five_hundred", "input": {} } }"#,
        )])
        .register("five_hundred", Outcome(TaskOutcome::Status(500)))
        .build()
        .expect("engine should build");

    let mut message = Message::builder().data(dv(json!({}))).build();
    let result = engine.process_message(&mut message).await;

    assert!(result.is_err(), "a 5xx propagates rather than halting");
    assert!(
        message
            .errors()
            .iter()
            .any(|e| e.code == "TASK_STATUS_ERROR"),
        "the status error is still recorded: {:?}",
        message.errors()
    );
}

#[tokio::test]
async fn halt_on_failure_keeps_its_own_audit_status() {
    // The point of routing through the `flow` fold rather than returning
    // `TaskOutcome::Halt`: the 400 the host answers with survives.
    let engine = Engine::builder()
        .with_workflows(vec![workflow(
            r#"{ "id": "check", "name": "check", "halt_on": "failure",
                 "function": { "name": "reject", "input": {} } }"#,
        )])
        .register("reject", Outcome(TaskOutcome::Status(400)))
        .build()
        .expect("engine should build");

    let message = run(&engine, json!({})).await;
    let statuses: Vec<usize> = message.audit_trail().iter().map(|e| e.status).collect();
    assert_eq!(
        statuses,
        vec![400],
        "the task's own status survives; `halt_on` is not `HALT_STATUS_CODE`"
    );
    assert_eq!(
        Value::from(&message.context["metadata"]["progress"]["status_code"]),
        json!(400),
        "and downstream workflows can route on it"
    );
}

#[tokio::test]
async fn terminal_is_stronger_than_halt_on() {
    // The two compose by `or`, so there is no contradictory combination:
    // `terminal` halts even on the success `halt_on` would let through.
    let engine = Engine::builder()
        .with_workflows(vec![workflow(&format!(
            r#"{{ "id": "check", "name": "check", "terminal": true, "halt_on": "failure",
                 "function": {{ "name": "pass", "input": {{}} }} }}, {}"#,
            mark("last")
        ))])
        .register("pass", Outcome(TaskOutcome::Success))
        .build()
        .expect("engine should build");

    let message = run(&engine, json!({})).await;
    assert!(
        ran(&message, &["last"]).is_empty(),
        "`terminal` halts regardless of outcome"
    );
}

#[tokio::test]
async fn halt_on_failure_breaks_the_whole_loop() {
    let engine = Engine::builder()
        .with_workflows(vec![
            Workflow::from_json(
                r#"{ "id": "sweeps", "name": "sweeps", "priority": 1,
                     "loop": { "counter": "i", "max": 5 },
                     "tasks": [
                       { "id": "tick", "name": "tick", "function": { "name": "map",
                         "input": { "mappings": [ { "path": "data.ticks",
                           "logic": {"+": [{"var": "data.ticks"}, 1]} } ] } } },
                       { "id": "exit", "name": "exit", "halt_on": "failure",
                         "condition": {">=": [{"var": "temp_data.i"}, 2]},
                         "function": { "name": "reject", "input": {} } }
                     ] }"#,
            )
            .unwrap(),
        ])
        .register("reject", Outcome(TaskOutcome::Status(400)))
        .build()
        .expect("engine should build");

    let message = run(&engine, json!({ "ticks": 0 })).await;
    assert_eq!(
        Value::from(&message.context["data"]["ticks"]),
        json!(3),
        "sweeps 0, 1 and 2 ran; failing on sweep 2 broke the whole loop"
    );
}

#[tokio::test]
async fn halt_on_failure_stops_only_its_own_workflow() {
    // Why `halt_on` is not a security control: a later workflow still runs.
    let engine = Engine::builder()
        .with_workflows(vec![
            Workflow::from_json(
                r#"{ "id": "w1", "name": "w1", "priority": 1,
                     "tasks": [ { "id": "check", "name": "check", "halt_on": "failure",
                       "function": { "name": "reject", "input": {} } } ] }"#,
            )
            .unwrap(),
            Workflow::from_json(&format!(
                r#"{{ "id": "w2", "name": "w2", "priority": 0, "tasks": [{}] }}"#,
                mark("later")
            ))
            .unwrap(),
        ])
        .register("reject", Outcome(TaskOutcome::Status(400)))
        .build()
        .expect("engine should build");

    let message = run(&engine, json!({})).await;
    assert_eq!(
        ran(&message, &["later"]),
        vec!["later"],
        "halting scopes to one workflow — stopping a message needs an `Err`"
    );
}

#[test]
fn halt_on_defaults_to_never_and_a_group_cannot_carry_it() {
    let wf = workflow(&format!("{}, {}", mark("a"), mark("b")));
    assert!(
        wf.tasks.iter().all(|t| t.halt_on == HaltOn::Never),
        "`halt_on` defaults to Never"
    );

    let err = Workflow::from_json(
        r#"{ "id": "w", "name": "w", "priority": 0, "tasks": [
             { "id": "g", "halt_on": "failure", "tasks": [
               { "id": "t", "name": "t", "function": { "name": "map",
                 "input": { "mappings": [] } } } ] } ] }"#,
    )
    .expect_err("a group carrying halt_on is refused at parse time");
    assert!(
        format!("{err:?}").contains("halt_on"),
        "the parse error names the offending key, got: {err:?}"
    );
}

// =============================================================================
// Task groups
// =============================================================================

#[tokio::test]
async fn group_with_a_false_condition_skips_every_member() {
    let engine = engine_for(vec![workflow(&format!(
        r#"{{ "id": "guarded", "condition": {{ "var": "data.go" }}, "tasks": [{}, {}] }},
           {}"#,
        mark("a"),
        mark("b"),
        mark("after")
    ))]);

    let message = run(&engine, json!({ "go": false })).await;
    assert_eq!(
        ran(&message, &["a", "b", "after"]),
        vec!["after"],
        "the whole span is skipped, and the list resumes past it"
    );

    let message = run(&engine, json!({ "go": true })).await;
    assert_eq!(
        ran(&message, &["a", "b", "after"]),
        vec!["a", "b", "after"],
        "and runs in full when the condition holds"
    );
}

#[tokio::test]
async fn a_skipped_group_records_one_skipped_step_per_member() {
    let engine = engine_for(vec![workflow(&format!(
        r#"{{ "id": "guarded", "condition": {{ "var": "data.go" }}, "tasks": [{}, {}] }},
           {}"#,
        mark("a"),
        mark("b"),
        mark("after")
    ))]);

    let mut message = Message::builder().data(dv(json!({ "go": false }))).build();
    let mut trace =
        dataflow_rs::engine::trace::ExecutionTrace::with_options(TraceOptions::default());
    engine
        .process_message_tracing(&mut message, &mut trace)
        .await
        .expect("processing should succeed");

    let skipped: Vec<&str> = trace
        .steps
        .iter()
        .filter(|s| s.result == dataflow_rs::engine::trace::StepResult::Skipped)
        .filter_map(|s| s.task_id.as_deref())
        .collect();
    assert_eq!(
        skipped,
        vec!["a", "b"],
        "the trace stays task-granular — no group-level step variant"
    );
}

#[tokio::test]
async fn a_group_condition_is_evaluated_once_on_entry() {
    // The behaviour that rules out AND-folding the group condition into each
    // child: a task inside the block mutates what the block's condition reads,
    // and its siblings must still run. This is an `if` block, not N guards.
    let engine = engine_for(vec![workflow(&format!(
        r#"{{ "id": "guarded", "condition": {{ "var": "data.go" }}, "tasks": [
             {{ "id": "clear", "name": "clear", "function": {{ "name": "map",
                "input": {{ "mappings": [ {{ "path": "data.go", "logic": false }} ] }} }} }},
             {}
           ] }}"#,
        mark("sibling")
    ))]);

    let message = run(&engine, json!({ "go": true })).await;
    assert_eq!(
        ran(&message, &["sibling"]),
        vec!["sibling"],
        "the group was entered once; a member turning the condition off mid-block \
         must not switch off its own siblings"
    );
}

#[tokio::test]
async fn terminal_group_halts_after_its_last_task() {
    let steps = format!(
        r#"{{ "id": "exit_branch", "terminal": true, "condition": {{ "var": "data.stop" }},
              "tasks": [{}, {}] }},
           {}"#,
        mark("body_one"),
        mark("body_two"),
        mark("after")
    );
    let engine = engine_for(vec![workflow(&steps)]);

    let message = run(&engine, json!({ "stop": true })).await;
    assert_eq!(
        ran(&message, &["body_one", "body_two", "after"]),
        vec!["body_one", "body_two"],
        "the whole branch ran, then the workflow ended"
    );

    let message = run(&engine, json!({ "stop": false })).await;
    assert_eq!(
        ran(&message, &["body_one", "body_two", "after"]),
        vec!["after"],
        "a branch that never fired cannot end the workflow"
    );
}

#[tokio::test]
async fn nested_groups_skip_only_their_own_span() {
    let steps = format!(
        r#"{{ "id": "outer", "condition": {{ "var": "data.outer" }}, "tasks": [
             {},
             {{ "id": "inner", "condition": {{ "var": "data.inner" }}, "tasks": [{}] }},
             {}
           ] }},
           {}"#,
        mark("before_inner"),
        mark("in_inner"),
        mark("after_inner"),
        mark("after_outer")
    );
    let engine = engine_for(vec![workflow(&steps)]);
    let keys = ["before_inner", "in_inner", "after_inner", "after_outer"];

    let message = run(&engine, json!({ "outer": true, "inner": false })).await;
    assert_eq!(
        ran(&message, &keys),
        vec!["before_inner", "after_inner", "after_outer"],
        "a false inner condition skips only the inner span"
    );

    let message = run(&engine, json!({ "outer": false, "inner": true })).await;
    assert_eq!(
        ran(&message, &keys),
        vec!["after_outer"],
        "a false outer condition skips the whole subtree"
    );

    let message = run(&engine, json!({ "outer": true, "inner": true })).await;
    assert_eq!(ran(&message, &keys), keys, "both true runs everything");
}

#[tokio::test]
async fn a_terminal_outer_group_halts_even_when_its_only_child_was_skipped() {
    // The case a per-task close counter gets wrong: nothing inside `outer` ever
    // executes, so the task that would carry "outer closes here" is jumped
    // straight over — yet `outer` was entered and is terminal.
    let steps = format!(
        r#"{{ "id": "outer", "terminal": true, "condition": {{ "var": "data.outer" }},
              "tasks": [
                {{ "id": "inner", "condition": {{ "var": "data.inner" }}, "tasks": [{}] }}
              ] }},
           {}"#,
        mark("in_inner"),
        mark("after_outer")
    );
    let engine = engine_for(vec![workflow(&steps)]);

    let message = run(&engine, json!({ "outer": true, "inner": false })).await;
    assert_eq!(
        ran(&message, &["in_inner", "after_outer"]),
        Vec::<String>::new(),
        "the entered terminal group still ends the workflow"
    );
}

#[tokio::test]
async fn a_group_spanning_an_async_boundary_enters_and_exits_correctly() {
    let calls = Arc::new(AtomicUsize::new(0));
    let steps = format!(
        r#"{{ "id": "guarded", "condition": {{ "var": "data.go" }}, "tasks": [
             {},
             {{ "id": "mid", "name": "mid",
                "function": {{ "name": "mark_async", "input": {{ "key": "mid" }} }} }},
             {}
           ] }},
           {}"#,
        mark("pre"),
        mark("post"),
        mark("after")
    );
    let engine = Engine::builder()
        .with_workflows(vec![workflow(&steps)])
        .register(
            "mark_async",
            MarkAsync {
                calls: Arc::clone(&calls),
            },
        )
        .build()
        .expect("engine should build");
    let keys = ["pre", "mid", "post", "after"];

    let message = run(&engine, json!({ "go": true })).await;
    assert_eq!(ran(&message, &keys), keys, "sync, async, sync all ran");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let message = run(&engine, json!({ "go": false })).await;
    assert_eq!(
        ran(&message, &keys),
        vec!["after"],
        "a false condition jumps past the async task too"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the async handler was not called for the skipped span"
    );
}

#[tokio::test]
async fn groups_behave_the_same_in_the_shared_arena_cross_workflow_run() {
    // Two consecutive fully-sync workflows run inside one shared `ArenaContext`
    // via `execute_sync_workflow_run`; the group gate is per-workflow.
    let engine = engine_for(vec![
        Workflow::from_json(&format!(
            r#"{{ "id": "first", "name": "first", "priority": 0, "tasks": [
                 {{ "id": "g1", "condition": {{ "var": "data.go" }}, "tasks": [{}] }},
                 {}
               ] }}"#,
            mark("a"),
            mark("b")
        ))
        .unwrap(),
        Workflow::from_json(&format!(
            r#"{{ "id": "second", "name": "second", "priority": 1, "tasks": [
                 {{ "id": "g2", "condition": {{ "var": "data.go" }}, "tasks": [{}] }},
                 {}
               ] }}"#,
            mark("c"),
            mark("d")
        ))
        .unwrap(),
    ]);

    let message = run(&engine, json!({ "go": false })).await;
    assert_eq!(
        ran(&message, &["a", "b", "c", "d"]),
        vec!["b", "d"],
        "each workflow's group is gated independently"
    );

    let message = run(&engine, json!({ "go": true })).await;
    assert_eq!(ran(&message, &["a", "b", "c", "d"]), ["a", "b", "c", "d"]);
}

// =============================================================================
// Parsing and validation
// =============================================================================

#[tokio::test]
async fn a_workflow_without_groups_records_no_spans() {
    let wf = workflow(&format!("{}, {}", mark("a"), mark("b")));
    assert_eq!(wf.tasks.len(), 2);
    assert!(
        wf.tasks.iter().all(|t| t.group_starts.is_empty()),
        "no groups authored, no spans recorded"
    );
    assert!(
        wf.tasks.iter().all(|t| !t.terminal),
        "`terminal` defaults to false"
    );
}

#[test]
fn a_group_flattens_into_the_task_list_in_document_order() {
    let wf = workflow(&format!(
        r#"{}, {{ "id": "g", "condition": true, "tasks": [{}, {}] }}, {}"#,
        mark("a"),
        mark("b"),
        mark("c"),
        mark("d")
    ));

    let ids: Vec<&str> = wf.tasks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, vec!["a", "b", "c", "d"]);
    assert_eq!(wf.tasks[1].group_starts.len(), 1, "the group opens at `b`");
    assert_eq!(
        wf.tasks[1].group_starts[0].end, 3,
        "and its half-open span ends after `c`"
    );
}

#[test]
fn nested_groups_are_recorded_outermost_first() {
    let wf = workflow(&format!(
        r#"{{ "id": "outer", "condition": true, "tasks": [
             {{ "id": "inner", "condition": true, "tasks": [{}] }}
           ] }}"#,
        mark("only")
    ));

    let ids: Vec<&str> = wf.tasks[0]
        .group_starts
        .iter()
        .map(|g| g.id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["outer", "inner"],
        "the gate evaluates the outer condition first"
    );
}

#[test]
fn an_empty_group_is_rejected_at_parse_time() {
    let err = Workflow::from_json(
        r#"{ "id": "w", "name": "w", "tasks": [ { "id": "g", "tasks": [] } ] }"#,
    )
    .expect_err("an empty group should not parse");
    assert!(format!("{err}").contains("contains no tasks"), "got: {err}");
}

#[test]
fn groups_nested_too_deeply_are_rejected_at_parse_time() {
    let mut json = mark("leaf");
    for i in 0..9 {
        json = format!(r#"{{ "id": "g{i}", "condition": true, "tasks": [{json}] }}"#);
    }
    let err = Workflow::from_json(&format!(
        r#"{{ "id": "w", "name": "w", "tasks": [{json}] }}"#
    ))
    .expect_err("nine levels should not parse");
    assert!(format!("{err}").contains("nested deeper"), "got: {err}");
}

#[test]
fn a_group_id_colliding_with_a_task_id_is_rejected() {
    let wf = workflow(&format!(
        r#"{{ "id": "a", "condition": true, "tasks": [{}] }}"#,
        mark("a")
    ));
    let err = wf.validate().expect_err("the collision should be rejected");
    assert!(format!("{err}").contains("Duplicate"), "got: {err}");
}

#[test]
fn a_malformed_task_still_reports_its_own_missing_field() {
    // The reason this is not `#[serde(untagged)]`: an untagged enum would
    // report "data did not match any variant" instead.
    let err = Workflow::from_json(
        r#"{ "id": "w", "name": "w", "tasks": [ { "id": "t", "name": "t" } ] }"#,
    )
    .expect_err("a task with no function should not parse");
    assert!(
        format!("{err}").contains("missing field") && format!("{err}").contains("function"),
        "got: {err}"
    );
}

/// #54: `continue_on_error` on a group parses, is recorded so `check_workflow`
/// can report it, and changes nothing at run time.
///
/// This is the half that is easy to lose. Reporting the key could drift into
/// honouring it, which would silently change the error semantics of every
/// definition that already carries it — so the "does nothing" half is pinned
/// here, and a future change making it real has to delete this test on purpose.
#[tokio::test]
async fn a_group_continue_on_error_is_recorded_but_never_honoured() {
    let w = workflow(&format!(
        r#"{{ "id": "g", "continue_on_error": true, "tasks": [
               {{ "id": "boom", "name": "boom",
                  "function": {{ "name": "boom", "input": {{}} }} }} ] }},
           {}"#,
        mark("after")
    ));

    assert!(
        w.tasks[0].group_starts[0].continue_on_error,
        "the parser records what the author wrote, so the lint has something to read"
    );

    let engine = Engine::builder()
        .with_workflows(vec![w])
        .register("boom", Boom)
        .build()
        .expect("informational, not refused — the workflow still builds");

    let mut message = Message::builder().data(dv(json!({}))).build();
    let result = engine.process_message(&mut message).await;

    assert!(
        result.is_err(),
        "the group's flag is not honoured: the failing task's own \
         `continue_on_error` — defaulted false — still governs"
    );
    assert!(
        ran(&message, &["after"]).is_empty(),
        "and nothing after the group ran"
    );
}
