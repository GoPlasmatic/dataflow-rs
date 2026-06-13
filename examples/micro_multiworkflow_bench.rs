//! # Multi-workflow chained `process_message` microbenchmark
//!
//! Isolates the per-message cost of running a message through several chained
//! workflows with no tokio task-spawn overhead (`current_thread` runtime, tight
//! single-threaded loop — same methodology as `micro_cond_bench.rs`).
//!
//! ## What it measures and why
//!
//! The arena form of `message.context` is deep-walked (`ArenaContext::from_owned`
//! → `to_arena`) once per sync stretch, and today every workflow opens its own
//! `with_arena` scope. So a message through N consecutive fully-sync workflows
//! pays the heavy `data.input` deep-walk N times even though the context is the
//! same tree (just mutated between workflows). On top of that, a real (non-`true`)
//! workflow condition is evaluated via the OWNED path (`eval_to_owned`), which
//! deep-walks the whole context into the arena AGAIN per conditioned workflow.
//!
//! To quantify the headroom **without implementing the hoist**, this bench runs
//! the SAME task work in three layouts:
//!
//! - `grouped`: ONE workflow whose tasks are `parse + N×(map+validate)`, all in a
//!   single sync stretch → **1** `from_owned`/msg. The lower bound a per-message
//!   arena hoist targets.
//! - `split-true`: N workflows, one stage each, all `condition: true` (folds to
//!   `None`) → **N** `from_owned`/msg. Today's cost.
//! - `split-progress`: N workflows gated on `metadata.progress.workflow_id` (the
//!   documented chaining pattern) → N `from_owned` PLUS (N-1) owned-path condition
//!   deep-walks/msg.
//!
//! `grouped` and `split-*` perform identical map/validate evals, identical writes,
//! identical audit entries — they differ only in how many times the context is
//! deep-walked. So `headroom(arena hoist) ≈ split-true − grouped` and
//! `headroom(+ in-arena condition) ≈ split-progress − grouped`.
//!
//! `micro_cond_bench.rs` (single workflow) and `benchmark.rs` are the regression
//! guard — they must not move when the hoist lands; only this bench should.
//!
//! Run with: `cargo run --example micro_multiworkflow_bench --release`

use dataflow_rs::{Engine, Message, Workflow};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Instant;

const N: usize = 500_000;
const WARMUP: usize = 50_000;
const NUM_STAGES: usize = 5;

/// JSON for a `map` + `validation` pair operating on stage `i`: reads the heavy
/// `data.input.*` (loaded once by the parse task) and writes a small
/// `data.stage{i}.*` subtree (a distinct `refresh_for_path` slot per stage).
fn stage_tasks_json(i: usize) -> String {
    format!(
        r#"
            {{
                "id": "map_{i}",
                "name": "Transform {i}",
                "function": {{
                    "name": "map",
                    "input": {{
                        "mappings": [
                            {{ "path": "stage{i}.id",     "logic": {{ "var": "data.input.id" }} }},
                            {{ "path": "stage{i}.name",   "logic": {{ "var": "data.input.party.name" }} }},
                            {{ "path": "stage{i}.amount", "logic": {{ "var": "data.input.amount.value" }} }},
                            {{ "path": "stage{i}.ccy",    "logic": {{ "var": "data.input.amount.currency" }} }},
                            {{ "path": "stage{i}.adj",    "logic": {{ "+": [ {{ "var": "data.input.amount.value" }}, {i} ] }} }},
                            {{ "path": "stage{i}.flag",   "logic": {{ ">": [ {{ "var": "data.input.amount.value" }}, 0 ] }} }}
                        ]
                    }}
                }}
            }},
            {{
                "id": "validate_{i}",
                "name": "Validate {i}",
                "function": {{
                    "name": "validation",
                    "input": {{
                        "rules": [
                            {{ "path": "stage{i}.id",     "logic": {{ "!!": {{ "var": "data.stage{i}.id" }} }},          "message": "id required" }},
                            {{ "path": "stage{i}.amount", "logic": {{ ">": [ {{ "var": "data.stage{i}.amount" }}, 0 ] }}, "message": "amount must be positive" }}
                        ]
                    }}
                }}
            }}"#
    )
}

/// The `parse_json` task that loads the whole payload into `data.input` so every
/// subsequent `from_owned` deep-walk includes the heavy tree.
fn parse_task_json() -> &'static str {
    r#"
            {
                "id": "parse",
                "name": "Load payload",
                "function": { "name": "parse_json", "input": { "source": "payload", "target": "input" } }
            }"#
}

/// `grouped` layout: one workflow, `parse + N×(map+validate)` in a single sync
/// stretch → one `from_owned` per message.
fn grouped_workflow() -> Workflow {
    let stages: Vec<String> = (0..NUM_STAGES).map(stage_tasks_json).collect();
    let json = format!(
        r#"{{
            "id": "wf_grouped",
            "name": "Grouped",
            "priority": 0,
            "condition": true,
            "tasks": [ {},{} ]
        }}"#,
        parse_task_json(),
        stages.join(",")
    );
    Workflow::from_json(&json).unwrap()
}

/// `split` layout: N workflows, one stage each (workflow 0 also parses).
/// `conditioned` gates every workflow after the first on the previous workflow's
/// `metadata.progress.workflow_id` (a real compiled condition → owned-path eval).
fn split_workflows(conditioned: bool) -> Vec<Workflow> {
    (0..NUM_STAGES)
        .map(|i| {
            let condition = if conditioned && i > 0 {
                format!(
                    r#"{{ "==": [ {{ "var": "metadata.progress.workflow_id" }}, "wf_{}" ] }}"#,
                    i - 1
                )
            } else {
                "true".to_string()
            };
            let tasks = if i == 0 {
                format!("{},{}", parse_task_json(), stage_tasks_json(i))
            } else {
                stage_tasks_json(i)
            };
            let json = format!(
                r#"{{
                    "id": "wf_{i}",
                    "name": "Workflow {i}",
                    "priority": {i},
                    "condition": {condition},
                    "tasks": [ {tasks} ]
                }}"#
            );
            Workflow::from_json(&json).unwrap()
        })
        .collect()
}

/// A few-KB, nested ISO-20022-shaped payload so the per-message `to_arena`
/// deep-walk of `data.input` is heavy enough to dominate the measurement.
fn build_payload() -> Value {
    let transactions: Vec<Value> = (0..20)
        .map(|k| {
            json!({
                "seq": k,
                "ref": format!("REF-{:06}", k),
                "debtor":   { "name": format!("Debtor {}", k),   "account": format!("DE89{:018}", k), "bic": "DEUTDEFF" },
                "creditor": { "name": format!("Creditor {}", k), "account": format!("FR14{:018}", k), "bic": "BNPAFRPP" },
                "amount":   { "value": 1000 + k, "currency": "EUR" },
                "remittance": format!("Invoice {} payment for services rendered in batch run", k)
            })
        })
        .collect();

    json!({
        "id": 12345,
        "party": { "name": "John Doe", "country": "DE", "id": "PARTY-001" },
        "amount": { "value": 250, "currency": "EUR" },
        "groupHeader": {
            "msgId": "MSG-2024-0001",
            "creationDateTime": "2024-06-13T10:00:00Z",
            "numberOfTxns": 20
        },
        "transactions": transactions
    })
}

/// Time `process_message` over an already-built engine. Returns ns/msg.
/// `expected_audit` asserts the full task chain executed (1 parse + N×2 tasks).
async fn time_engine(label: &str, engine: &Engine, payload: &Value, expected_audit: usize) -> f64 {
    {
        let mut m = Message::from_value(payload);
        engine.process_message(&mut m).await.unwrap();
        assert_eq!(
            m.audit_trail().len(),
            expected_audit,
            "[{label}] expected {expected_audit} audit entries, got {} — chain did not fully execute",
            m.audit_trail().len()
        );
    }

    for _ in 0..WARMUP {
        let mut m = Message::from_value(payload);
        engine.process_message(&mut m).await.unwrap();
    }

    let start = Instant::now();
    for _ in 0..N {
        let mut m = Message::from_value(payload);
        engine.process_message(&mut m).await.unwrap();
    }
    let el = start.elapsed();
    let ns = el.as_nanos() as f64 / N as f64;
    println!(
        "  {label:<16} {N} msgs in {:.3}s => {ns:>8.1} ns/msg => {:.0} msg/s",
        el.as_secs_f64(),
        N as f64 / el.as_secs_f64()
    );
    ns
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let payload = build_payload();
    let expected_audit = 1 + NUM_STAGES * 2; // 1 parse + N×(map+validate)

    let grouped = Arc::new(
        Engine::builder()
            .with_workflow(grouped_workflow())
            .build()
            .unwrap(),
    );
    let split_true = Arc::new(
        Engine::builder()
            .with_workflows(split_workflows(false))
            .build()
            .unwrap(),
    );
    let split_prog = Arc::new(
        Engine::builder()
            .with_workflows(split_workflows(true))
            .build()
            .unwrap(),
    );

    println!(
        "Multi-workflow bench: {NUM_STAGES} stages (parse + {NUM_STAGES}x map+validate), heavy data.input\n"
    );

    let g = time_engine("grouped(1 wf)", &grouped, &payload, expected_audit).await;
    let st = time_engine("split-true(N wf)", &split_true, &payload, expected_audit).await;
    let sp = time_engine("split-progress(N)", &split_prog, &payload, expected_audit).await;

    println!(
        "\nHeadroom estimate (work is identical; only deep-walk count differs):\n  \
         arena hoist        : split-true     - grouped = {:>7.1} ns/msg ({:+.1}%)\n  \
         + in-arena cond    : split-progress - grouped = {:>7.1} ns/msg ({:+.1}%)",
        st - g,
        (st - g) / g * 100.0,
        sp - g,
        (sp - g) / g * 100.0,
    );
}
