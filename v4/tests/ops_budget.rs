#![cfg(feature = "budget")]
extern crate dataflow_rs_v4 as dataflow_rs;

// `ExprEngineBuilder::ops_budget`: the per-evaluation operation ceiling, and
// the `ErrorKind::Budget` (`BUDGET_EXCEEDED`) it raises. v3's
// `tests/ops_budget.rs` ported (I111), with the reload through `compiler()`,
// `link` and `with_program`, plus the `map` mapping over the ceiling that
// `corpus/cases/expressions/budget` records.
//
// The whole file is gated: without the `budget` feature there is no
// `ops_budget` to call, and datalogic charges nothing, so every assertion
// here would be vacuous rather than merely absent.

use dataflow_rs::def::{ParseOptions, WorkflowDef, parse};
use dataflow_rs::expr::ExprEngine;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::run::{ErrorKind, ErrorRecord, RunStatus};
use dataflow_rs::{Ctx, Engine, Expr, Function, Message, Outcome, Path, Registry, Result, Run};
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
    Engine::builder()
        .registry(registry)
        .workflow_json(workflow)
        .build()
        .unwrap_or_else(|issues| panic!("the engine builds: {issues:?}"))
}

/// An engine over `expr`, optionally bounded.
fn engine(expr: Value, budget: Option<u64>) -> Engine {
    engine_over(&sum_workflow(expr), budget)
}

/// A message whose `data` is `data`.
fn message_with(data: Value) -> Message {
    Message::from_parts(
        OwnedDataValue::Null,
        OwnedDataValue::from(&data),
        OwnedDataValue::Null,
    )
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
async fn a_map_mapping_over_the_ceiling_fails_its_task_with_a_500() {
    // A built-in's evaluation is bounded too, but `map` reports it as v3 does
    // (`expressions/budget`): the mapping is logged and skipped, the later
    // mappings still run, and the task returns 500, recorded as
    // `TASK_STATUS_ERROR`. Under `on_failure: continue` the run completes,
    // with errors.
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
    let audit = m.audit();
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0].status, 500);
    let codes: Vec<&str> = m.errors().iter().map(|e| e.code.as_str()).collect();
    assert_eq!(codes, ["TASK_STATUS_ERROR"]);
    assert_eq!(m.errors()[0].message, "Task t returned status 500");
    let data = data(&m);
    assert!(
        data.get("sum").is_none(),
        "the refused mapping wrote: {data}"
    );
    assert_eq!(data["after_map"], json!(2), "the later mapping still ran");
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
