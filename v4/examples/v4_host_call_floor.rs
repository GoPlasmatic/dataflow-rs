//! # Host call floor
//!
//! The gate's statistic for what one host call costs on its own. It runs
//! `async_handler_benchmark`'s workload: a sync stretch (`parse_json`, a
//! three-mapping `map`, a two-rule `validation`), then optionally one echo
//! host call that writes one string.
//!
//! - **Why it exists.** `async_handler_benchmark.marginal` cannot gate the
//!   floor. It is a difference of two latency means taken under a 500,000-task
//!   spawn burst, and its noise ran from 29% to 66% on the gate machine.
//! - **How it measures.** v3 and v4 run in one process, on one thread, so
//!   there is no scheduler, no spawn and no cross-thread traffic. The two
//!   engines alternate in rounds inside the process, as `bench/gate.py
//!   compare` alternates binaries. Round 0 is discarded as cold. Each
//!   statistic is the median of the per-round values.
//! - **What it reports.**
//!   - `sync` is ns per message without the call, `call` is ns per message
//!     with it, and `floor` is the call's own cost, `call - sync`.
//!   - Each `ratio` is v4 over v3: the median of the per-round ratios.
//!   - `call.ratio` carries the gate's verdict, "not slower". `sync.ratio`
//!     and `floor.ratio` are reported alongside it (`bench/gate.py`,
//!     `SELF_JUDGED`).
//!
//! Each engine's result is checked before anything is timed.
//!
//! Run with: `cargo run -p dataflow-rs-v4 --example v4_host_call_floor --release`

// The derive emits `::dataflow_rs::` paths.
extern crate dataflow_rs_v4 as dataflow_rs;

use dataflow_rs_v4::datavalue::OwnedDataValue;
use dataflow_rs_v4::expr::Output;
use dataflow_rs_v4::function::FunctionConfig;
use dataflow_rs_v4::{Ctx, Function, Outcome};
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

#[path = "../../examples/shared/gate.rs"]
mod gate;

/// Messages per engine and configuration in one round.
const BATCH: usize = 30_000;
/// Rounds, the first of which is discarded as cold.
const ROUNDS: usize = 11;

/// The workload's tasks: `async_handler_benchmark`'s, with or without the
/// echo call.
fn workflow(with_call: bool) -> Value {
    let mut tasks = vec![
        json!({"id": "load_payload", "name": "Parse Payload", "function": {
            "name": "parse_json", "input": {"source": "payload", "target": "input"}}}),
        json!({"id": "transform", "name": "Transform", "function": {"name": "map", "input": {
        "mappings": [
            {"path": "data.user_id", "logic": {"var": "data.input.user_id"}},
            {"path": "data.amount", "logic": {"var": "data.input.amount"}},
            {"path": "data.currency", "logic": {"var": "data.input.currency"}}
        ]}}}),
        json!({"id": "validate", "name": "Validate", "function": {"name": "validation", "input": {
        "rules": [
            {"logic": {"!!": {"var": "data.user_id"}}, "message": "user_id required"},
            {"logic": {">": [{"var": "data.amount"}, 0]}, "message": "amount must be positive"}
        ]}}}),
    ];
    if with_call {
        tasks.push(json!({"id": "echo", "name": "Echo", "function": {
            "name": "echo", "input": {"target": "data.echoed", "value": "ok"}}}));
    }
    let id = if with_call {
        "with_custom"
    } else {
        "sync_only"
    };
    json!({"id": id, "name": id, "tasks": tasks})
}

fn payload() -> OwnedDataValue {
    OwnedDataValue::from(&json!({
        "user_id": "user_42",
        "amount": 1500.50,
        "currency": "EUR",
        "metadata": { "source": "benchmark" }
    }))
}

/// v4's echo: one write through the call's overlay.
#[derive(FunctionConfig)]
struct EchoConfig {
    #[output(default = "data.echoed")]
    target: Output,
    value: String,
}

struct Echo;

impl Function for Echo {
    type Config = EchoConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &EchoConfig) -> dataflow_rs_v4::Result<Outcome> {
        ctx.emit(&cfg.target, OwnedDataValue::String(cfg.value.clone()))?;
        Ok(Outcome::Ok)
    }
}

/// v3's echo, `async_handler_benchmark`'s `EchoHandler`, written as
/// `async_trait` expands it, since the macro is not a dependency here.
#[derive(serde::Deserialize)]
struct EchoInput {
    target: String,
    value: String,
}

struct V3Echo;

impl dataflow_rs_v3::AsyncFunctionHandler for V3Echo {
    type Input = EchoInput;

    fn execute<'life0, 'life1, 'life2, 'life3, 'async_trait>(
        &'life0 self,
        ctx: &'life1 mut dataflow_rs_v3::TaskContext<'life2>,
        input: &'life3 EchoInput,
    ) -> Pin<
        Box<
            dyn Future<Output = dataflow_rs_v3::Result<dataflow_rs_v3::TaskOutcome>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        'life3: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            ctx.set(&input.target, OwnedDataValue::String(input.value.clone()));
            Ok(dataflow_rs_v3::TaskOutcome::Success)
        })
    }
}

/// One engine of either version, running one message the way the gate's
/// benchmarks do.
enum Engine {
    V3(dataflow_rs_v3::Engine),
    V4(dataflow_rs_v4::Engine),
}

impl Engine {
    fn v3(with_call: bool) -> Self {
        let raw = workflow(with_call).to_string();
        let mut builder = dataflow_rs_v3::Engine::builder()
            .with_workflow(dataflow_rs_v3::Workflow::from_json(&raw).expect("v3 parses"));
        if with_call {
            builder = builder.register("echo", V3Echo);
        }
        Self::V3(builder.build().expect("v3 builds"))
    }

    fn v4(with_call: bool) -> Self {
        let mut builder = dataflow_rs_v4::Engine::builder().workflow_json(&workflow(with_call));
        if with_call {
            builder = builder.register("echo", Echo);
        }
        Self::V4(builder.build().expect("v4 builds"))
    }

    /// One message: its `data` and its audit entries, checked for errors.
    async fn one(&self, payload: &Arc<OwnedDataValue>) -> (Value, usize) {
        match self {
            Self::V3(engine) => {
                let mut m = dataflow_rs_v3::Message::new(Arc::clone(payload));
                engine.process_message(&mut m).await.expect("v3 runs");
                assert!(m.errors().is_empty(), "v3 errors: {:?}", m.errors());
                (Value::from(&m.context["data"]), m.audit_trail().len())
            }
            Self::V4(engine) => {
                let mut m = dataflow_rs_v4::Message::new(Arc::clone(payload));
                engine.run(&mut m).await.into_result().expect("v4 runs");
                assert!(m.errors().is_empty(), "v4 errors: {:?}", m.errors());
                (Value::from(m.context().data()), m.audit().len())
            }
        }
    }

    /// `n` messages, in ns per message.
    async fn batch(&self, payload: &Arc<OwnedDataValue>, n: usize) -> f64 {
        let start = Instant::now();
        match self {
            Self::V3(engine) => {
                for _ in 0..n {
                    let mut m = dataflow_rs_v3::Message::new(Arc::clone(payload));
                    engine.process_message(&mut m).await.expect("v3 runs");
                }
            }
            Self::V4(engine) => {
                for _ in 0..n {
                    let mut m = dataflow_rs_v4::Message::new(Arc::clone(payload));
                    engine.run(&mut m).await.into_result().expect("v4 runs");
                }
            }
        }
        start.elapsed().as_secs_f64() * 1e9 / n as f64
    }
}

/// Checks one engine before timing: the mappings landed, both rules
/// passed, one audit entry per task, and the echo's write where it runs.
async fn check(name: &str, engine: &Engine, payload: &Arc<OwnedDataValue>, with_call: bool) {
    let (data, entries) = engine.one(payload).await;
    assert_eq!(
        data["user_id"],
        json!("user_42"),
        "{name}: the mappings landed"
    );
    assert_eq!(data["amount"], json!(1500.5), "{name}: the mappings landed");
    assert_eq!(
        entries,
        3 + usize::from(with_call),
        "{name}: one entry per task"
    );
    let echoed = with_call.then(|| json!("ok"));
    assert_eq!(
        data.get("echoed").cloned(),
        echoed,
        "{name}: the echo's write"
    );
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

/// Half the inter-quartile range of `values`, in percent of their median.
fn half_iqr_pct(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let at = |q: f64| {
        let i = q * (values.len() - 1) as f64;
        let (lo, hi) = (i.floor() as usize, i.ceil() as usize);
        values[lo] + (values[hi] - values[lo]) * (i - lo as f64)
    };
    (at(0.75) - at(0.25)) / 2.0 / at(0.5) * 100.0
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    println!(
        "HOST CALL FLOOR: async_handler_benchmark's workload, v3 and v4 in one process, one thread"
    );
    println!(
        "{BATCH} messages per engine and configuration per round, {ROUNDS} rounds (the first discarded), order alternating\n"
    );
    let payload = Arc::new(payload());
    // (version, with the call), in a round's order; odd rounds run it
    // backwards.
    let cells = [(3, false), (4, false), (3, true), (4, true)];
    let engines: Vec<Engine> = cells
        .iter()
        .map(|&(version, with_call)| match version {
            3 => Engine::v3(with_call),
            _ => Engine::v4(with_call),
        })
        .collect();
    for (&(version, with_call), engine) in cells.iter().zip(&engines) {
        check(&format!("v{version}"), engine, &payload, with_call).await;
    }

    // ns per message, per cell, per kept round.
    let mut ns: Vec<[f64; 4]> = Vec::with_capacity(ROUNDS - 1);
    for round in 0..ROUNDS {
        let mut got = [0.0; 4];
        let order: Vec<usize> = if round % 2 == 0 {
            (0..4).collect()
        } else {
            (0..4).rev().collect()
        };
        for i in order {
            got[i] = engines[i].batch(&payload, BATCH).await;
        }
        if round > 0 {
            ns.push(got);
        }
    }

    let cell = |i: usize| median(ns.iter().map(|r| r[i]).collect());
    let floor = |r: &[f64; 4], v: usize| r[2 + v] - r[v];
    let ratios = |f: &dyn Fn(&[f64; 4]) -> f64| ns.iter().map(f).collect::<Vec<_>>();
    let sync_ratio = ratios(&|r| r[1] / r[0]);
    let call_ratio = ratios(&|r| r[3] / r[2]);
    let floor_ratio = ratios(&|r| floor(r, 1) / floor(r, 0));
    let floor_ns = |v: usize| median(ns.iter().map(|r| floor(r, v)).collect());

    let rows = [
        ("sync only", cell(0), cell(1), &sync_ratio),
        ("with the call", cell(2), cell(3), &call_ratio),
        (
            "floor (call - sync)",
            floor_ns(0),
            floor_ns(1),
            &floor_ratio,
        ),
    ];
    println!(
        "{:<22} {:>12} {:>12} {:>10} {:>12}",
        "", "v3 ns/msg", "v4 ns/msg", "v4/v3", "±half IQR"
    );
    for (label, v3, v4, r) in rows {
        println!(
            "{label:<22} {v3:>12.1} {v4:>12.1} {:>10.3} {:>11.1}%",
            median(r.clone()),
            half_iqr_pct(r.clone())
        );
    }

    let mut gate = gate::Gate::new("host_call_floor");
    for (name, (_, v3, v4, r)) in ["sync", "call", "floor"].into_iter().zip(rows) {
        gate.lower(format!("{name}.v3_ns"), v3, "ns")
            .lower(format!("{name}.v4_ns"), v4, "ns")
            .lower(format!("{name}.ratio"), median(r.clone()), "ratio");
    }
    gate.emit();
}
