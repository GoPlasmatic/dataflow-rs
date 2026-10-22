// `ExprEngineBuilder::ops_budget`: the per-evaluation operation ceiling, and
// the `ErrorKind::Budget` (`BUDGET_EXCEEDED`) it raises. v3's
// `tests/ops_budget.rs` ported, with the reload through `compiler()`,
// `link` and `with_program`, plus the built-ins over the ceiling: a `map`
// mapping, a `filter` condition and a `validation` rule each fail their task.
//
// The whole module is gated (`main.rs`): without the `budget` feature there
// is no `ops_budget` to call, and datalogic charges nothing, so every
// assertion here would be vacuous rather than merely absent.

use dataflow_rs::def::{ParseOptions, WorkflowDef, parse};
use dataflow_rs::expr::ExprEngine;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::run::{ErrorRecord, RunStatus};
use dataflow_rs::{
    Ctx, Engine, ErrorKind, Expr, Function, Message, Outcome, Path, Registry, Result, Run,
};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};

/// How many elements the expensive expression folds over. Large enough that a
/// four-figure ceiling is crossed well before the fold finishes, small enough
/// that the unbudgeted run stays instant.
const N: usize = 20_000;

/// A ceiling far below the cost of folding [`N`] elements, but far above the
/// handful of operations a trivial expression spends, so the two tasks below
/// are separated by the budget, not by luck.
const TIGHT: u64 = 2_000;

// -----------------------------------------------------------------------------
// Fixtures
// -----------------------------------------------------------------------------

/// `sum_via_eval`'s config.
#[derive(FunctionConfig)]
struct SumConfig {
    expr: Expr,
}

/// Sums `data.xs` through `Ctx::eval`: the host-function path, which is
/// where a datalogic evaluation error becomes an `Error` rather than being
/// logged and skipped.
struct SumViaEval;

impl Function for SumViaEval {
    type Config = SumConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &SumConfig) -> Result<Outcome> {
        let total = ctx.eval(&cfg.expr)?.into_owned();
        ctx.set(&"data.total".parse()?, total)?;
        Ok(Outcome::Ok)
    }
}

/// A fold over `data.xs`: one charged operation per element examined, plus the
/// dispatch of each `+`.
fn fold_expr() -> Value {
    json!({
        "reduce": [
            { "var": "data.xs" },
            { "+": [{ "var": "current" }, { "var": "accumulator" }] },
            0
        ]
    })
}

fn sum_workflow(expr: Value) -> Value {
    json!({
        "id": "w", "priority": 0,
        "tasks": [
            { "id": "sum", "function": {
                "name": "sum_via_eval",
                "input": { "expr": expr } } }
        ]
    })
}

/// The expression engine, optionally bounded.
fn exprs(budget: Option<u64>) -> ExprEngine {
    let builder = ExprEngine::builder();
    match budget {
        Some(b) => builder.ops_budget(b),
        None => builder,
    }
    .build()
    .unwrap()
}

/// An engine over `workflow`, optionally bounded, with `sum_via_eval`.
fn engine_over(workflow: &Value, budget: Option<u64>) -> Engine {
    let mut registry = Registry::new(exprs(budget));
    registry.register("sum_via_eval", SumViaEval);
    crate::support::engine_with(std::slice::from_ref(workflow), |b| b.registry(registry))
}

/// An engine over `expr`, optionally bounded.
fn engine(expr: Value, budget: Option<u64>) -> Engine {
    engine_over(&sum_workflow(expr), budget)
}

/// A message whose `data` is `data`.
fn message_with(data: Value) -> Message {
    crate::support::message(data)
}

/// A message carrying [`N`] elements at `data.xs`.
fn message() -> Message {
    message_with(json!({ "xs": vec![1; N] }))
}

fn data(m: &Message) -> Value {
    Value::from(m.context().data())
}

fn def(json: &Value) -> WorkflowDef {
    parse(json, ParseOptions::default()).def.unwrap()
}

/// The task-level error recorded on `m`: the classified one.
fn task_error(m: &Message) -> &ErrorRecord {
    m.errors()
        .iter()
        .find(|e| e.task_id.is_some())
        .unwrap_or_else(|| panic!("no task-level error recorded, got {:?}", m.errors()))
}

/// The run stopped with a budget error, its code `BUDGET_EXCEEDED`.
fn assert_stopped_by_the_budget(run: &Run) {
    let RunStatus::Stopped(e) = run.status() else {
        panic!("a task whose only evaluation was refused should not report success: {run:?}");
    };
    assert_eq!(e.kind(), &ErrorKind::Budget, "{e:?}");
    assert_eq!(e.code(), "BUDGET_EXCEEDED");
}

// =============================================================================
// The ceiling
// =============================================================================

#[tokio::test]
async fn an_expression_over_the_ceiling_is_refused_with_its_own_error_code() {
    let engine = engine(fold_expr(), Some(TIGHT));

    let mut m = message();
    let run = engine.run(&mut m).await;

    assert_stopped_by_the_budget(&run);
    let err = task_error(&m);
    assert_eq!(
        err.code, "BUDGET_EXCEEDED",
        "a budget abort must not collapse into LOGIC_ERROR: {err:?}"
    );
    assert!(
        data(&m).get("total").is_none_or(Value::is_null),
        "the write must not happen: the ceiling aborts *before* the work"
    );
    assert_eq!(
        err.message.matches("Operation budget exceeded").count(),
        1,
        "datalogic's message already names the condition; the record must \
         not prefix it again: {}",
        err.message
    );
}

#[tokio::test]
async fn the_same_expression_is_fine_unbudgeted() {
    // Two properties at once. The control: nothing about this workflow is
    // inherently too expensive, so the previous test measures the ceiling and
    // not a broken fold. And the default: never calling `ops_budget` leaves
    // evaluation unbounded, so merely compiling with the feature is not a
    // breaking change for a host that never asked for a ceiling.
    let engine = engine(fold_expr(), None);

    let mut m = message();
    let run = engine.run(&mut m).await;

    assert!(matches!(run.status(), RunStatus::Completed), "{run:?}");
    assert!(m.errors().is_empty(), "unbudgeted run: {:?}", m.errors());
    assert_eq!(
        data(&m)["total"],
        json!(N),
        "the fold should have completed"
    );
}

#[tokio::test]
async fn a_cheap_expression_passes_under_the_same_ceiling() {
    // The ceiling is per *evaluation*, and a small one is nowhere near it, so
    // `ops_budget` is not simply failing everything it touches. v3's
    // `{"+": [1, 2]}` folds to a constant at compile and evaluates nothing,
    // so a cheap expression that reads the message runs beside it.
    for (expr, input, total) in [
        (json!({ "+": [1, 2] }), json!({}), json!(3)),
        (
            json!({ "+": [{ "var": "data.one" }, 2] }),
            json!({ "one": 1 }),
            json!(3),
        ),
    ] {
        let engine = engine(expr.clone(), Some(TIGHT));

        // Neither expression reads `data.xs`, so a message without it is the
        // honest input: it is the ceiling being tested, not the data.
        let mut m = message_with(input);
        let run = engine.run(&mut m).await;

        assert!(
            matches!(run.status(), RunStatus::Completed),
            "{expr}: {run:?}"
        );
        assert!(m.errors().is_empty(), "{expr}: {:?}", m.errors());
        assert_eq!(data(&m)["total"], total, "{expr}");
    }
}

#[tokio::test]
async fn a_map_mapping_over_the_ceiling_fails_its_task() {
    // 3.x logged the mapping, skipped it and returned 500, so the ceiling was
    // spent silently. As `validation`'s, it is the task's failure: recorded
    // as `BUDGET_EXCEEDED` and governed by `on_failure`, and the call ends
    // there. Under `on_failure: continue` the run completes, with errors.
    let wf = json!({
        "id": "mapw", "on_failure": "continue",
        "tasks": [ { "id": "t", "on_failure": "continue",
            "function": { "name": "map", "input": { "mappings": [
                { "path": "data.sum", "logic": fold_expr() },
                { "path": "data.after_map", "logic": { "+": [{ "var": "data.one" }, 1] } }
            ] } } } ]
    });
    let engine = engine_over(&wf, Some(TIGHT));

    let mut m = message_with(json!({ "one": 1, "xs": vec![1; N] }));
    let run = engine.run(&mut m).await;

    assert!(
        matches!(run.status(), RunStatus::CompletedWithErrors),
        "{run:?}"
    );
    let codes: Vec<&str> = m.errors().iter().map(|e| e.code.as_str()).collect();
    assert_eq!(codes, ["BUDGET_EXCEEDED"]);
    let data = data(&m);
    assert!(
        data.get("sum").is_none(),
        "the refused mapping wrote: {data}"
    );
    assert!(data.get("after_map").is_none(), "the call ended: {data}");

    // Under the default `on_failure`, it stops the run.
    let engine = engine_over(&with_default_policies(&wf), Some(TIGHT));
    let mut m = message_with(json!({ "one": 1, "xs": vec![1; N] }));
    assert_stopped_by_the_budget(&engine.run(&mut m).await);
}

#[tokio::test]
async fn a_filter_condition_over_the_ceiling_fails_its_task() {
    // 3.x read a runaway condition as unmet: a silent rejection.
    let wf = json!({
        "id": "w", "priority": 0,
        "tasks": [
            { "id": "gate", "function": { "name": "filter",
                "input": { "condition": { ">": [fold_expr(), 0] } } } },
            { "id": "after", "function": { "name": "map", "input": {
                "mappings": [{ "path": "data.went_on", "logic": true }] } } }
        ]
    });
    let engine = engine_over(&wf, Some(TIGHT));
    let mut m = message();
    let run = engine.run(&mut m).await;
    assert_stopped_by_the_budget(&run);
    assert_eq!(task_error(&m).code, "BUDGET_EXCEEDED");
    assert!(data(&m).get("went_on").is_none());
}

/// `wf` with every policy key removed, so its failures take the defaults.
fn with_default_policies(wf: &Value) -> Value {
    let mut wf = wf.clone();
    wf.as_object_mut().unwrap().remove("on_failure");
    for task in wf["tasks"].as_array_mut().unwrap() {
        task.as_object_mut().unwrap().remove("on_failure");
    }
    wf
}

// =============================================================================
// Carried state
// =============================================================================

#[tokio::test]
async fn the_ceiling_survives_a_hot_reload() {
    // A reload compiles against the program's registry, and the ceiling lives
    // on its `ExprEngine`. A budget that silently lifted itself there would be
    // worse than no budget at all: the bound would hold until the first
    // reload and then quietly stop.
    let engine = engine(json!({ "+": [1, 2] }), Some(TIGHT));

    let compiler = engine.compiler();
    let compiled = compiler
        .compile_one(&def(&sum_workflow(fold_expr())))
        .unwrap()
        .workflow;
    let reloaded = engine.with_program(compiler.link([compiled]));

    let mut m = message();
    let run = reloaded.run(&mut m).await;

    assert_stopped_by_the_budget(&run);
    assert_eq!(task_error(&m).code, "BUDGET_EXCEEDED");
}

// =============================================================================
// Classification
// =============================================================================

/// A budget error, from an evaluation over the ceiling.
fn budget_error() -> dataflow_rs::Error {
    let exprs = exprs(Some(TIGHT));
    let expr: Expr = exprs.compile(&fold_expr()).unwrap();
    let path: Path = "data.xs".parse().unwrap();
    let mut root = OwnedDataValue::from(&json!({ "data": {}, "metadata": {}, "temp_data": {} }));
    path.set(&mut root, OwnedDataValue::from(&json!(vec![1; N])))
        .unwrap();
    exprs
        .eval(&expr, &root)
        .expect_err("the fold crosses the ceiling")
}

#[test]
fn a_budget_error_is_not_retryable() {
    // Deterministic: the same rule over the same data spends the same ops, so
    // a retry would cross the same ceiling at the same node.
    let err = budget_error();
    assert_eq!(err.kind(), &ErrorKind::Budget);
    assert_eq!(err.code(), "BUDGET_EXCEEDED");
    assert!(!err.is_retryable());
}

#[test]
fn a_budget_error_round_trips_through_serde() {
    // Neither `ErrorKind::Budget` nor its code is `#[cfg(feature = "budget")]`:
    // errors travel, and a host built without the feature still has to read
    // one. What travels is the record.
    let record = ErrorRecord::from(&budget_error());
    assert_eq!(record.code, "BUDGET_EXCEEDED");
    let json = serde_json::to_string(&record).unwrap();
    let back: ErrorRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(back, record);
    assert!(
        back.message.contains("Operation budget exceeded"),
        "{}",
        back.message
    );
}

// =============================================================================
// `validation`
// =============================================================================

/// A workflow whose one `validation` task checks `rule`, then a `map` that
/// marks the workflow as having gone on; `policy` is the task's extra keys.
fn validation_workflow(rule: Value, policy: Value) -> Value {
    let mut task = json!({ "id": "check", "function": { "name": "validation",
        "input": { "rules": [rule] } } });
    for (k, v) in policy.as_object().unwrap() {
        task[k] = v.clone();
    }
    json!({
        "id": "w", "priority": 0,
        "tasks": [
            task,
            { "id": "after", "function": { "name": "map", "input": {
                "mappings": [{ "path": "data.went_on", "logic": true }] } } }
        ]
    })
}

#[tokio::test]
async fn a_validation_rule_over_the_ceiling_fails_the_task_rather_than_rejecting() {
    // 3.x made a runaway rule an `EVALUATION_ERROR` rejection (400), which
    // `on_rejection: continue` steps past: the ceiling would be silently
    // spent. It is the task's failure, under `on_failure`.
    let over = json!({ ">": [fold_expr(), 0] });
    for (rule, at) in [
        (json!({ "logic": over, "message": "never" }), "logic"),
        (
            json!({ "logic": false, "message": { "cat": ["n=", fold_expr()] } }),
            "message",
        ),
    ] {
        let engine = engine_over(&validation_workflow(rule, json!({})), Some(TIGHT));
        let mut m = message();
        let run = engine.run(&mut m).await;
        assert_stopped_by_the_budget(&run);
        assert_eq!(task_error(&m).code, "BUDGET_EXCEEDED", "{at}");
        assert!(data(&m).get("went_on").is_none(), "{at}");

        // `on_failure: continue` steps past it, as for any failure.
        let engine = engine_over(
            &validation_workflow(
                json!({ "logic": { ">": [fold_expr(), 0] }, "message": "never" }),
                json!({ "on_failure": "continue" }),
            ),
            Some(TIGHT),
        );
        let mut m = message();
        engine.run(&mut m).await;
        assert_eq!(task_error(&m).code, "BUDGET_EXCEEDED", "{at}");
        assert_eq!(data(&m)["went_on"], json!(true), "{at}");
    }
}

#[tokio::test]
async fn a_validation_rule_that_fails_to_evaluate_is_still_a_rejection() {
    // Any other evaluation error stays 3.x's `EVALUATION_ERROR` and 400.
    let rule = json!({ "logic": { "+": [{ "var": "data.word" }, "x"] }, "message": "m" });
    let engine = engine_over(&validation_workflow(rule, json!({})), Some(TIGHT));
    let mut m = message_with(json!({ "word": "abc" }));
    let run = engine.run(&mut m).await;
    assert!(!matches!(run.status(), RunStatus::Stopped(_)), "{run:?}");
    assert!(
        m.errors().iter().any(|e| e.code == "EVALUATION_ERROR"),
        "{:?}",
        m.errors()
    );
    assert_eq!(data(&m)["went_on"], json!(true));
}

#[tokio::test]
async fn condition_error_policy_stop_halts_budget_exceeded_across_all_condition_sites() {
    use dataflow_rs::RunOptions;
    use dataflow_rs::run::{ConditionErrorPolicy, SiteKind};

    let over = json!({ ">": [fold_expr(), 0] });
    let mark = json!({
        "id": "after",
        "function": {
            "name": "map",
            "input": { "mappings": [{ "path": "data.went_on", "logic": true }] }
        }
    });

    // 1. Workflow condition over ops budget
    let wf_cond = json!({
        "id": "w1",
        "condition": over.clone(),
        "on_failure": "continue",
        "tasks": [mark.clone()]
    });
    let e = engine_over(&wf_cond, Some(TIGHT));
    let mut m = message();
    let run = e
        .run_with(
            &mut m,
            &RunOptions::default().condition_error_policy(ConditionErrorPolicy::Stop),
        )
        .await;
    assert_stopped_by_the_budget(&run);
    if let RunStatus::Stopped(err) = run.status() {
        assert_eq!(
            err.site().map(|s| s.kind),
            Some(SiteKind::WorkflowCondition)
        );
        assert!(err.expr_cause().and_then(|c| c.budget.as_ref()).is_some());
    }
    assert!(data(&m).get("went_on").is_none());

    // 2. Group condition over ops budget
    let group_cond = json!({
        "id": "w1",
        "on_failure": "continue",
        "tasks": [
            {
                "id": "g1",
                "condition": over.clone(),
                "tasks": [{
                    "id": "in_group",
                    "function": {
                        "name": "map",
                        "input": { "mappings": [{ "path": "data.went_on", "logic": true }] }
                    }
                }]
            },
            mark.clone()
        ]
    });
    let e = engine_over(&group_cond, Some(TIGHT));
    let mut m = message();
    let run = e
        .run_with(
            &mut m,
            &RunOptions::default().condition_error_policy(ConditionErrorPolicy::Stop),
        )
        .await;
    assert_stopped_by_the_budget(&run);
    if let RunStatus::Stopped(err) = run.status() {
        assert_eq!(err.site().map(|s| s.kind), Some(SiteKind::GroupCondition));
    }
    assert!(data(&m).get("went_on").is_none());

    // 3. Task condition over ops budget
    let task_cond = json!({
        "id": "w1",
        "on_failure": "continue",
        "tasks": [
            {
                "id": "t1",
                "condition": over.clone(),
                "on_failure": "continue",
                "function": {
                    "name": "map",
                    "input": { "mappings": [{ "path": "data.t1", "logic": true }] }
                }
            },
            mark.clone()
        ]
    });
    let e = engine_over(&task_cond, Some(TIGHT));
    let mut m = message();
    let run = e
        .run_with(
            &mut m,
            &RunOptions::default().condition_error_policy(ConditionErrorPolicy::Stop),
        )
        .await;
    assert_stopped_by_the_budget(&run);
    if let RunStatus::Stopped(err) = run.status() {
        assert_eq!(err.site().map(|s| s.kind), Some(SiteKind::TaskCondition));
    }
    assert!(data(&m).get("went_on").is_none());

    // 4. Filter condition over ops budget even with `on_failure: continue` and `on_reject: skip`
    let filter_cond = json!({
        "id": "w1",
        "on_failure": "continue",
        "tasks": [
            {
                "id": "f1",
                "on_failure": "continue",
                "function": {
                    "name": "filter",
                    "input": {
                        "condition": over,
                        "on_reject": "skip"
                    }
                }
            },
            mark
        ]
    });
    let e = engine_over(&filter_cond, Some(TIGHT));
    let mut m = message();
    let run = e
        .run_with(
            &mut m,
            &RunOptions::default().condition_error_policy(ConditionErrorPolicy::Stop),
        )
        .await;
    assert_stopped_by_the_budget(&run);
    if let RunStatus::Stopped(err) = run.status() {
        assert_eq!(err.site().map(|s| s.kind), Some(SiteKind::FilterCondition));
    }
    assert!(data(&m).get("went_on").is_none());
}

#[tokio::test]
async fn cumulative_run_expr_ops_budget_and_accountant_track_partial_ops_on_ok_and_err() {
    use dataflow_rs::RunOptions;
    use dataflow_rs::run::RunAccountant;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Default)]
    struct OpsAccountant {
        total_ops: AtomicU64,
    }

    impl RunAccountant for OpsAccountant {
        fn on_expr_evaluated(&self, ops: u64) {
            self.total_ops.fetch_add(ops, Ordering::SeqCst);
        }
    }

    // Two medium-sized folds: each individually fits under the engine's per-eval
    // ceiling (or an unbounded engine), but together they exceed the run's
    // cumulative `with_expr_ops_budget`.
    let wf = json!({
        "id": "w",
        "on_failure": "continue",
        "tasks": [
            {
                "id": "first",
                "on_failure": "continue",
                "function": {
                    "name": "map",
                    "input": { "mappings": [{ "path": "data.a", "logic": fold_expr() }] }
                }
            },
            {
                "id": "second",
                "on_failure": "continue",
                "function": {
                    "name": "map",
                    "input": { "mappings": [{ "path": "data.b", "logic": fold_expr() }] }
                }
            }
        ]
    });
    let e = engine_over(&wf, None);
    let acc = OpsAccountant::default();
    let mut m = message_with(json!({ "xs": vec![1; 50] }));
    let run = e
        .run_with(
            &mut m,
            &RunOptions::default()
                .with_expr_ops_budget(300)
                .accountant(&acc),
        )
        .await;
    assert_stopped_by_the_budget(&run);
    assert_eq!(
        data(&m)["a"],
        json!(50),
        "first fold fits under 300 ops and completes"
    );
    assert!(
        data(&m).get("b").is_none(),
        "second fold crosses the remaining cumulative run ops budget and aborts"
    );
    assert!(
        acc.total_ops.load(Ordering::SeqCst) > 150,
        "accountant observed ops from both the completed first fold and the aborted second fold"
    );
}
