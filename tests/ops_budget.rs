//! `EngineBuilder::with_ops_budget` — the per-evaluation operation ceiling,
//! and the [`DataflowError::BudgetExceeded`] it raises.
//!
//! The whole file is gated: without the `budget` feature there is no
//! `with_ops_budget` to call, and datalogic charges nothing, so every
//! assertion here would be vacuous rather than merely absent.
#![cfg(feature = "budget")]

use async_trait::async_trait;
use dataflow_rs::engine::functions::AsyncFunctionHandler;
use dataflow_rs::engine::message::Message;
use dataflow_rs::{
    DataflowError, Engine, Result, TaskContext, TaskOutcome, Template, TemplateCompiler, Workflow,
};
use serde_json::{Value, json};

mod common;

use common::workflow;

/// How many elements the expensive expression folds over. Large enough that a
/// four-figure ceiling is crossed well before the fold finishes, small enough
/// that the unbudgeted run stays instant.
const N: usize = 20_000;

/// A ceiling far below the cost of folding [`N`] elements, but far above the
/// handful of operations a trivial expression spends — so the two tasks below
/// are separated by the budget, not by luck.
const TIGHT: u64 = 2_000;

// -----------------------------------------------------------------------------
// Fixtures
// -----------------------------------------------------------------------------

/// Sums `data.xs` through `ctx.eval` — the `TaskContext` path, which is where
/// a datalogic evaluation error becomes a `DataflowError` rather than being
/// logged and skipped.
struct SumViaEval;

#[async_trait]
impl AsyncFunctionHandler for SumViaEval {
    type Input = SumInput;

    fn compile_input(input: &mut Self::Input, c: &TemplateCompiler) -> Result<()> {
        input.expr.compile(c, "expr")
    }

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &Self::Input) -> Result<TaskOutcome> {
        let total = input.expr.eval(ctx)?;
        ctx.set("data.total", total);
        Ok(TaskOutcome::Success)
    }
}

#[derive(serde::Deserialize)]
struct SumInput {
    expr: Template,
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

fn sum_workflow(expr: Value) -> Workflow {
    workflow(json!({
        "id": "w", "name": "w", "priority": 0,
        "tasks": [
            { "id": "sum", "name": "sum", "function": {
                "name": "sum_via_eval",
                "input": { "expr": expr } } }
        ]
    }))
}

/// An engine over `expr`, optionally bounded.
fn engine(expr: Value, budget: Option<u64>) -> Engine {
    let builder = Engine::builder()
        .register("sum_via_eval", SumViaEval)
        .with_workflow(sum_workflow(expr));
    match budget {
        Some(b) => builder.with_ops_budget(b),
        None => builder,
    }
    .build()
    .unwrap()
}

/// A message carrying [`N`] elements at `data.xs`.
fn message() -> Message {
    Message::builder()
        .data_json(&json!({ "xs": vec![1; N] }))
        .build()
}

fn total(m: &Message) -> Value {
    Value::from(m.data().get("total").unwrap())
}

/// The task-level error recorded on `m`.
///
/// A task error that propagates records twice — once against the task and
/// again against the workflow, which is this engine's ordinary shape and not
/// something the budget changes. The task entry is the classified one.
fn task_error(m: &Message) -> &dataflow_rs::ErrorInfo {
    m.errors()
        .iter()
        .find(|e| e.task_id.is_some())
        .unwrap_or_else(|| panic!("no task-level error recorded, got {:?}", m.errors()))
}

// =============================================================================
// The ceiling
// =============================================================================

#[tokio::test]
async fn an_expression_over_the_ceiling_is_refused_with_its_own_error_code() {
    let engine = engine(fold_expr(), Some(TIGHT));

    let mut m = message();
    let outcome = engine.process_message(&mut m).await;

    assert!(
        outcome.is_err(),
        "a task whose only evaluation was refused should not report success"
    );
    let err = task_error(&m);
    assert_eq!(
        err.code, "BUDGET_EXCEEDED",
        "a budget abort must not collapse into LOGIC_ERROR: {err:?}"
    );
    assert!(
        m.data().get("total").is_none_or(|v| v.is_null()),
        "the write must not happen — the ceiling aborts *before* the work"
    );
    assert_eq!(
        err.message.matches("Operation budget exceeded").count(),
        1,
        "datalogic's message already names the condition; the variant must \
         not prefix it again: {}",
        err.message
    );
}

#[tokio::test]
async fn the_same_expression_is_fine_unbudgeted() {
    // Two properties at once. The control: nothing about this workflow is
    // inherently too expensive, so the previous test measures the ceiling and
    // not a broken fold. And the default: never calling `with_ops_budget`
    // leaves evaluation unbounded, so merely compiling with the feature is not
    // a breaking change for a host that never asked for a ceiling.
    let engine = engine(fold_expr(), None);

    let mut m = message();
    engine.process_message(&mut m).await.unwrap();

    assert!(m.errors().is_empty(), "unbudgeted run: {:?}", m.errors());
    assert_eq!(total(&m), json!(N), "the fold should have completed");
}

#[tokio::test]
async fn a_cheap_expression_passes_under_the_same_ceiling() {
    // The ceiling is per *evaluation*, and a small one is nowhere near it —
    // so `with_ops_budget` is not simply failing everything it touches.
    let engine = engine(json!({ "+": [1, 2] }), Some(TIGHT));

    // The expression never reads `data.xs`, so an empty message is the honest
    // input — it is the ceiling being tested, not the data.
    let mut m = Message::builder().build();
    engine.process_message(&mut m).await.unwrap();

    assert!(m.errors().is_empty(), "{:?}", m.errors());
    assert_eq!(total(&m), json!(3));
}

// =============================================================================
// Carried state
// =============================================================================

#[tokio::test]
async fn the_ceiling_survives_a_hot_reload() {
    // `with_new_workflows` builds a *fresh* datalogic engine. A budget that
    // silently lifted itself there would be worse than no budget at all: the
    // bound would hold until the first reload and then quietly stop.
    let engine = engine(json!({ "+": [1, 2] }), Some(TIGHT));

    let reloaded = engine
        .with_new_workflows(vec![sum_workflow(fold_expr())])
        .unwrap();

    let mut m = message();
    let outcome = reloaded.process_message(&mut m).await;

    assert!(outcome.is_err(), "the reloaded engine dropped its ceiling");
    assert_eq!(task_error(&m).code, "BUDGET_EXCEEDED");
}

// =============================================================================
// Classification
// =============================================================================

#[test]
fn a_budget_error_is_not_retryable() {
    // Deterministic: the same rule over the same data spends the same ops, so
    // a retry would cross the same ceiling at the same node.
    assert!(!DataflowError::BudgetExceeded("over".into()).retryable());
}

#[test]
fn a_budget_error_round_trips_through_serde() {
    // The variant is deliberately not `#[cfg(feature = "budget")]` — errors
    // travel, and a host built without the feature still has to read one.
    let err = DataflowError::BudgetExceeded("over at node 3".into());
    let json = serde_json::to_string(&err).unwrap();
    let back: DataflowError = serde_json::from_str(&json).unwrap();
    assert!(matches!(back, DataflowError::BudgetExceeded(m) if m == "over at node 3"));
}
