//! # Loop scaling
//!
//! A row of the v4 performance gate: the shape of #66 to #69, from Orion#371.
//! A turn loop runs 1000 sweeps over a large context and appends one entry to
//! a log every sweep, so the context grows as the loop runs. Each sweep
//! observes (a `map`), decides, then steps (a `map` that updates the score and
//! appends to `data.log`).
//!
//! In 3.15 every sweep converts the whole context into the arena again, once
//! per sync stretch, so a sweep pays for the context's bytes, and a late sweep
//! pays more than an early one because the log has grown. v4 keeps one view of
//! the context for the whole run and splices each write into it, so the gate
//! requires this row to be faster.
//!
//! The decide step is measured two ways:
//!
//! - **by map:** the whole sweep is one sync stretch;
//! - **by handler:** a host handler resolves two `Template` fields in one
//!   `with_view` and writes the decision, as Orion's `model_infer` does once
//!   per seat. The sweep is then two stretches around an `.await`.
//!
//! Each runs at 250 and 1000 sweeps. The per-sweep figure at 1000 over the one
//! at 250 shows whether a sweep pays for the log: 1.0x means it does not.
//!
//! Run with: `cargo run --example loop_scaling --release`

use async_trait::async_trait;
use dataflow_rs::{
    AsyncFunctionHandler, DataflowError, Engine, Message, Result, TaskContext, TaskOutcome,
    Template, TemplateCompiler, Workflow,
};
use datavalue::OwnedDataValue;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Instant;

#[path = "shared/bench.rs"]
mod bench;

/// The context before the loop starts: carried by every sweep, read by none.
const CONTEXT_BYTES: usize = 256 * 1024;

/// (sweeps per message, timed messages).
const CONFIGS: &[(u64, usize)] = &[(250, 40), (1000, 10)];

#[derive(Deserialize)]
struct DecideInput {
    seat: Template,
    turn: Template,
    output: String,
}

/// The decide step as a host handler: two fields resolved in one view, one
/// write.
struct Decide;

#[async_trait]
impl AsyncFunctionHandler for Decide {
    type Input = DecideInput;

    fn compile_input(input: &mut DecideInput, c: &TemplateCompiler) -> Result<()> {
        input.seat.compile(c, "seat")?;
        input.turn.compile(c, "turn")
    }

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &DecideInput) -> Result<TaskOutcome> {
        let (seat, turn) = ctx.with_view(|view| {
            Ok::<_, DataflowError>((
                input.seat.resolve_u64_in(view, "seat")?,
                input.turn.resolve_u64_in(view, "turn")?,
            ))
        })?;
        let action = json!({ "seat": seat, "delta": delta(seat, turn) });
        ctx.set(&input.output, OwnedDataValue::from(&action));
        Ok(TaskOutcome::Success)
    }
}

/// The decision both variants make, so both produce the same message.
fn delta(seat: u64, turn: u64) -> u64 {
    (seat * 7 + turn) % 5
}

fn workflow(sweeps: u64, by_handler: bool) -> Workflow {
    let decide = if by_handler {
        json!({ "id": "decide", "name": "Decide", "function": { "name": "decide", "input": {
            "seat": { "var": "temp_data.obs.seat" },
            "turn": { "var": "temp_data.turn" },
            "output": "temp_data.action"
        }}})
    } else {
        json!({ "id": "decide", "name": "Decide", "function": { "name": "map", "input": {
            "mappings": [{ "path": "temp_data.action", "logic": {
                "seat": { "var": "temp_data.obs.seat" },
                "delta": { "%": [
                    { "+": [{ "*": [{ "var": "temp_data.obs.seat" }, 7] }, { "var": "temp_data.turn" }] },
                    5
                ]}
            }}]
        }}})
    };
    let workflow = json!({
        "id": "turns",
        "name": "Turn loop",
        "loop": { "counter": "turn", "max": sweeps },
        "tasks": [
            { "id": "observe", "name": "Observe", "function": { "name": "map", "input": {
                "mappings": [
                    { "path": "temp_data.obs.turn", "logic": { "var": "temp_data.turn" } },
                    { "path": "temp_data.obs.seat", "logic": { "%": [{ "var": "temp_data.turn" }, 8] } },
                    { "path": "temp_data.obs.score", "logic": { "var": "data.state.score" } }
                ]
            }}},
            decide,
            { "id": "step", "name": "Step", "function": { "name": "map", "input": {
                "mappings": [
                    { "path": "data.state.score", "logic":
                        { "+": [{ "var": "data.state.score" }, { "var": "temp_data.action.delta" }] } },
                    { "path": "data.log", "mode": "append", "logic": {
                        "turn": { "var": "temp_data.turn" },
                        "seat": { "var": "temp_data.action.seat" },
                        "delta": { "var": "temp_data.action.delta" },
                        "score": { "var": "data.state.score" },
                        "phase": "play",
                        "flags": [true, false, { "var": "temp_data.obs.seat" }]
                    }}
                ]
            }}}
        ]
    });
    Workflow::from_json(&workflow.to_string()).expect("the loop workflow parses")
}

fn engine(sweeps: u64, by_handler: bool) -> Engine {
    let builder = Engine::builder().with_workflow(workflow(sweeps, by_handler));
    let builder = if by_handler {
        builder.register("decide", Decide)
    } else {
        builder
    };
    builder.build().expect("the engine builds")
}

/// Runs one message and checks the loop did its work: one log entry and
/// three audit entries per sweep, and a score that adds every decision.
async fn check(engine: &Engine, base: &OwnedDataValue, sweeps: u64) {
    let mut message = Message::builder().data(base.clone()).build();
    engine
        .process_message(&mut message)
        .await
        .expect("the loop runs");
    assert!(
        message.errors().is_empty(),
        "errors: {:?}",
        message.errors()
    );
    let data = Value::from(&message.context["data"]);
    let log = data["log"].as_array().expect("data.log is an array");
    assert_eq!(log.len() as u64, sweeps, "one log entry per sweep");
    let expected: u64 = (0..sweeps).map(|turn| delta(turn % 8, turn)).sum();
    assert_eq!(
        data["state"]["score"].as_f64(),
        Some(expected as f64),
        "the score adds every decision"
    );
    assert_eq!(
        message.audit_trail().len() as u64,
        3 * sweeps,
        "three tasks per sweep"
    );
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let input = bench::large_document(CONTEXT_BYTES);
    println!("LOOP SCALING: observe -> decide -> step, appending to data.log every sweep");
    println!(
        "Context before the loop: {} at data.input, read by no step\n",
        bench::describe(&input)
    );
    let base = OwnedDataValue::from(&json!({ "input": input, "state": { "score": 0 } }));

    let mut means = Vec::new();
    for &(sweeps, messages) in CONFIGS {
        for by_handler in [false, true] {
            let engine = engine(sweeps, by_handler);
            check(&engine, &base, sweeps).await;

            let mut samples = bench::Samples::with_capacity(messages);
            for _ in 0..messages {
                let mut message = Message::builder().data(base.clone()).build();
                let start = Instant::now();
                engine
                    .process_message(&mut message)
                    .await
                    .expect("the loop runs");
                samples.push(start.elapsed());
            }
            let how = if by_handler { "handler" } else { "map" };
            let label = format!("{sweeps:>4} sweeps, decide by {how}");
            means.push((
                sweeps,
                by_handler,
                samples.row(&label, sweeps as usize, "sweep"),
            ));
        }
    }

    println!();
    for by_handler in [false, true] {
        let mean_at = |sweeps: u64| {
            means
                .iter()
                .find(|row| row.0 == sweeps && row.1 == by_handler)
                .map(|row| row.2)
                .expect("both sizes ran")
        };
        let how = if by_handler { "handler" } else { "map" };
        println!(
            "Growth, decide by {how:<7}: µs/sweep at 1000 over 250 = {:.2}x",
            mean_at(1000) / mean_at(250)
        );
    }
}
