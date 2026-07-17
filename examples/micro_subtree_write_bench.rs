//! # Same-subtree map-write scaling microbenchmark
//!
//! Isolates the write-path cost item #1 of the perf proposal attacks: k map
//! mappings all targeting the same depth-2 subtree (`data.DOC.f{i}`), the
//! canonical shape of document-assembly workloads (e.g. 25 mappings into
//! `data.MT103` in the realistic bench).
//!
//! Before the arena write-through, every mapping's cache refresh re-arena'd
//! the ENTIRE owned `data.DOC` subtree (`to_arena` deep clone, string copies
//! included), so mapping n re-walked all n-1 previously written fields —
//! O(k²) total deep-clone work per message. The write-through splices the
//! already-arena-resident eval result into the depth-2 cache and rebuilds
//! only the spine (shallow pair copies), leaving the per-message write cost
//! ~linear in k.
//!
//! The k-sweep makes the difference visible as a scaling slope: with each
//! mapping performing one identical JSONLogic eval (linear in k), any
//! superlinear growth in ns/mapping as k rises is write-path overhead.
//! Compare binaries built before/after the write-through commit to quantify
//! the win; on a fixed binary, watch that ns/mapping stays ~flat across k.
//!
//! Methodology matches `micro_multiworkflow_bench.rs`: `current_thread`
//! runtime, tight single-threaded loop, default `capture_changes = true`.
//!
//! Run with: `cargo run --example micro_subtree_write_bench --release`

use dataflow_rs::{Engine, Message, Workflow};
use serde_json::{Value, json};
use std::time::Instant;

/// (k mappings, messages to time). Message counts shrink as k grows to keep
/// each config's wall time comparable.
const CONFIGS: &[(usize, usize)] = &[(5, 150_000), (25, 60_000), (100, 15_000)];

/// One workflow: parse the payload into `data.input`, then a single map task
/// whose k mappings all write string values into `data.DOC.f{i}` — same
/// depth-2 subtree, accumulating k fields per message.
fn subtree_workflow(k: usize) -> Workflow {
    let mappings: Vec<String> = (0..k)
        .map(|i| {
            format!(
                r#"{{ "path": "data.DOC.f{i}",
                     "logic": {{ "cat": [ {{ "var": "data.input.party.name" }}, " :: ",
                                          {{ "var": "data.input.groupHeader.msgId" }}, " :: field-{i:04}" ] }} }}"#
            )
        })
        .collect();
    let json = format!(
        r#"{{
            "id": "wf_subtree",
            "name": "Subtree writes",
            "priority": 0,
            "condition": true,
            "tasks": [
                {{
                    "id": "parse",
                    "name": "Load payload",
                    "function": {{ "name": "parse_json", "input": {{ "source": "payload", "target": "input" }} }}
                }},
                {{
                    "id": "assemble",
                    "name": "Assemble DOC",
                    "function": {{ "name": "map", "input": {{ "mappings": [ {} ] }} }}
                }}
            ]
        }}"#,
        mappings.join(",")
    );
    Workflow::from_json(&json).unwrap()
}

/// Small payload: the measurement targets the write path, not the
/// payload-walk, so keep `data.input` light.
fn build_payload() -> Value {
    json!({
        "party": { "name": "Acme Global Payments GmbH", "country": "DE" },
        "groupHeader": { "msgId": "MSG-2026-000123", "creationDateTime": "2026-07-17T10:00:00Z" },
        "amount": { "value": 2500, "currency": "EUR" }
    })
}

async fn time_config(k: usize, n: usize, payload: &Value) -> (f64, f64) {
    let engine = Engine::builder()
        .with_workflow(subtree_workflow(k))
        .build()
        .unwrap();

    // Correctness gate: the full chain ran and all k fields landed.
    {
        let mut m = Message::from_value(payload);
        engine.process_message(&mut m).await.unwrap();
        assert_eq!(m.audit_trail().len(), 2, "parse + map must both run");
        let doc = &m.context["data"]["DOC"];
        for i in 0..k {
            let field = doc.get(format!("f{i}"));
            assert!(
                field.map(|v| v.as_str().is_some()).unwrap_or(false),
                "data.DOC.f{i} missing or not a string — mappings did not execute"
            );
        }
    }

    for _ in 0..n / 10 {
        let mut m = Message::from_value(payload);
        engine.process_message(&mut m).await.unwrap();
    }

    let start = Instant::now();
    for _ in 0..n {
        let mut m = Message::from_value(payload);
        engine.process_message(&mut m).await.unwrap();
    }
    let el = start.elapsed();
    let ns_msg = el.as_nanos() as f64 / n as f64;
    let ns_mapping = ns_msg / k as f64;
    println!(
        "  k={k:<4} {n:>7} msgs in {:>6.3}s => {ns_msg:>9.1} ns/msg => {ns_mapping:>7.1} ns/mapping",
        el.as_secs_f64()
    );
    (ns_msg, ns_mapping)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let payload = build_payload();
    println!(
        "Same-subtree write scaling: parse + k mappings into data.DOC.f*, capture_changes=on\n"
    );

    let mut per_mapping = Vec::new();
    for &(k, n) in CONFIGS {
        let (_, ns_mapping) = time_config(k, n, &payload).await;
        per_mapping.push((k, ns_mapping));
    }

    let (k0, m0) = per_mapping[0];
    let (kn, mn) = per_mapping[per_mapping.len() - 1];
    println!(
        "\nScaling check: ns/mapping at k={kn} vs k={k0} = {:.2}x \
         (1.0x = linear write cost; the pre-write-through engine grows with k)",
        mn / m0
    );
}
