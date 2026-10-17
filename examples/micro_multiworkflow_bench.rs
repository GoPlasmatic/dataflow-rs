//! # Multi-workflow chained `Engine::run` microbenchmark
//!
//! Isolates the per-message cost of running a message through several chained
//! workflows with no tokio task-spawn overhead (`current_thread` runtime, tight
//! single-threaded loop — same methodology as `micro_cond_bench.rs`).
//!
//! ## What it measures and why
//!
//! The arena form of `message.context` is deep-walked (`ArenaContext::from_owned`
//! → `to_arena`) once per sync stretch. This bench was written when every
//! workflow opened its own `with_arena` scope, so a message through N
//! consecutive fully-sync workflows paid the heavy `data.input` deep-walk N
//! times over the same tree, and a real (non-`true`) workflow condition was
//! evaluated via the OWNED path (`eval_to_owned`), deep-walking the whole
//! context AGAIN per conditioned workflow.
//!
//! **Both are fixed.** `WorkflowExecutor::execute_sync_workflow_run` carries one
//! `ArenaContext` across a maximal run of consecutive fully-sync workflows, and
//! evaluates each workflow condition in-arena. So this is now a regression
//! guard, not a proposal: the three layouts below should sit close together, and
//! `split-*` drifting away from `grouped` means the shared-arena run stopped
//! applying — most likely because a workflow stopped qualifying for
//! `joins_sync_run`.
//!
//! It runs the SAME task work in three layouts:
//!
//! - `grouped`: ONE workflow whose tasks are `parse + N×(map+validate)`, all in a
//!   single sync stretch → **1** `from_owned`/msg. The lower bound.
//! - `split-true`: N workflows, one stage each, all `condition: true` (folds to
//!   `None`) → **1** `from_owned`/msg now that the run is shared; **N** before.
//! - `split-progress`: N workflows gated on `metadata.progress.workflow_id` (the
//!   documented chaining pattern) → also **1** `from_owned`/msg, with each
//!   condition evaluated against the carried arena view; **N** plus (N-1)
//!   owned-path condition deep-walks before.
//!
//! `grouped` and `split-*` perform identical map/validate evals, identical writes,
//! identical audit entries — they differ only in how many times the context is
//! deep-walked. `split-true − grouped` is therefore what the shared-arena run
//! recovers, and `split-progress − grouped` what it recovers plus the in-arena
//! condition eval. Both should now be small.
//!
//! This is v4's port of v3's `micro_multiworkflow_bench`: the same three
//! layouts, stage count and message counts, run through dataflow-rs 4.
//!
//! Run with: `cargo run --example micro_multiworkflow_bench --release`

use dataflow_rs::{Engine, Message};
use serde_json::{Value, json};
use std::time::Instant;

#[path = "shared/gate.rs"]
mod gate;

const N: usize = 500_000;
const WARMUP: usize = 50_000;
const NUM_STAGES: usize = 5;

/// JSON for a `map` + `validation` pair operating on stage `i`: reads the heavy
/// `data.input.*` (loaded once by the parse task) and writes a small
/// subtree `stage{i}` (a distinct `refresh_for_path` slot per stage).
///
/// v3's benchmark writes OUTSIDE `data`: its `map` roots an unprefixed path
/// `stage{i}.id` at the whole context, so the write lands at the top-level
/// key `context.stage{i}`, while its rules read `data.stage{i}.*` and so fail
/// on every message (5 tasks at status 400, 10 `VALIDATION_ERROR` records).
/// That was a defect in v3's benchmark (reported to the owner), but it is the
/// work this row has measured from the start. The engine refuses a path
/// outside the three roots, so the port reproduces that work through
/// `temp_data.stage{i}.*`, a root no rule reads, and keeps the rules reading
/// `data.stage{i}.*` so the same 10 fail.
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
                            {{ "path": "temp_data.stage{i}.id",     "logic": {{ "var": "data.input.id" }} }},
                            {{ "path": "temp_data.stage{i}.name",   "logic": {{ "var": "data.input.party.name" }} }},
                            {{ "path": "temp_data.stage{i}.amount", "logic": {{ "var": "data.input.amount.value" }} }},
                            {{ "path": "temp_data.stage{i}.ccy",    "logic": {{ "var": "data.input.amount.currency" }} }},
                            {{ "path": "temp_data.stage{i}.adj",    "logic": {{ "+": [ {{ "var": "data.input.amount.value" }}, {i} ] }} }},
                            {{ "path": "temp_data.stage{i}.flag",   "logic": {{ ">": [ {{ "var": "data.input.amount.value" }}, 0 ] }} }}
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
fn grouped_workflow() -> Value {
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
    serde_json::from_str(&json).unwrap()
}

/// `split` layout: N workflows, one stage each (workflow 0 also parses).
/// `conditioned` gates every workflow after the first on the previous workflow's
/// `metadata.progress.workflow_id` (a real compiled condition → owned-path eval).
fn split_workflows(conditioned: bool) -> Vec<Value> {
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
            serde_json::from_str(&json).unwrap()
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

/// Time `Engine::run` over an already-built engine. Returns ns/msg.
/// `expected_audit` asserts the full task chain executed (1 parse + N×2 tasks).
async fn time_engine(label: &str, engine: &Engine, payload: &Value, expected_audit: usize) -> f64 {
    {
        let mut m = Message::from_json(payload);
        engine.run(&mut m).await.into_result().unwrap();
        assert_eq!(
            m.audit().len(),
            expected_audit,
            "[{label}] expected {expected_audit} audit entries, got {} — chain did not fully execute",
            m.audit().len()
        );
        // What v3's benchmark produces: the maps write, the rules (which read
        // `data.stage{i}`) all fail.
        let temp = Value::from(m.context().temp_data());
        assert_eq!(
            temp[format!("stage{}", NUM_STAGES - 1)]["amount"],
            json!(250),
            "[{label}] the last stage's mappings did not write"
        );
        assert_eq!(
            m.errors().len(),
            2 * NUM_STAGES,
            "[{label}] expected one VALIDATION_ERROR per rule, got {:?}",
            m.errors()
        );
        assert!(
            m.errors().iter().all(|e| e.code == "VALIDATION_ERROR"),
            "[{label}] errors: {:?}",
            m.errors()
        );
        let rejected = m
            .audit()
            .iter()
            .filter(|a| a.task_id.starts_with("validate_") && a.status == 400)
            .count();
        assert_eq!(rejected, NUM_STAGES, "[{label}] validation tasks at 400");
    }

    for _ in 0..WARMUP {
        let mut m = Message::from_json(payload);
        engine.run(&mut m).await.into_result().unwrap();
    }

    let start = Instant::now();
    for _ in 0..N {
        let mut m = Message::from_json(payload);
        engine.run(&mut m).await.into_result().unwrap();
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

    let build = |workflows: Vec<Value>| {
        workflows
            .iter()
            .fold(Engine::builder(), |b, w| b.workflow_json(w))
            .build()
            .expect("the engine builds")
    };
    let grouped = build(vec![grouped_workflow()]);
    let split_true = build(split_workflows(false));
    let split_prog = build(split_workflows(true));

    println!(
        "Multi-workflow bench: {NUM_STAGES} stages (parse + {NUM_STAGES}x map+validate), heavy data.input\n"
    );

    let g = time_engine("grouped(1 wf)", &grouped, &payload, expected_audit).await;
    let st = time_engine("split-true(N wf)", &split_true, &payload, expected_audit).await;
    let sp = time_engine("split-progress(N)", &split_prog, &payload, expected_audit).await;

    println!(
        "\nSplit-vs-grouped gap (work is identical; only deep-walk count differs).\n\
         Both should stay near zero — a gap means the shared-arena run stopped \
         applying:\n  \
         shared arena       : split-true     - grouped = {:>7.1} ns/msg ({:+.1}%)\n  \
         + in-arena cond    : split-progress - grouped = {:>7.1} ns/msg ({:+.1}%)",
        st - g,
        (st - g) / g * 100.0,
        sp - g,
        (sp - g) / g * 100.0,
    );

    gate::Gate::new("micro_multiworkflow_bench")
        .lower("grouped.ns_per_msg", g, "ns/msg")
        .lower("split_true.ns_per_msg", st, "ns/msg")
        .lower("split_progress.ns_per_msg", sp, "ns/msg")
        .emit();
}
