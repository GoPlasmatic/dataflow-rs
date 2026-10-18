//! # Loop scaling
//!
//! A row of the performance gate, its workload first written against 3.x: the
//! shape of #66 to #69, from Orion#371. A turn loop runs 1000 sweeps over a
//! large context and appends one entry to a log every sweep, so the context
//! grows as the loop runs. Each sweep observes (a `map`), decides, then steps
//! (a `map` that updates the score and appends to `data.log`).
//!
//! In 3.x every sweep converted the whole context into the arena again, once
//! per sync stretch, so a sweep paid for the context's bytes. The engine
//! keeps one view of the context for the whole run and splices each write
//! into it, so a sweep's cost must not grow with the log.
//!
//! The decide step is measured two ways:
//!
//! - **by map:** the whole sweep runs inline;
//! - **by handler:** a host function evaluates two `Expr` fields against the
//!   run's view and writes the decision through an `Output`, as Orion's
//!   `model_infer` does once per seat. The sweep then has an `.await` in it,
//!   which in 3.x split it into two stretches, each converting the context.
//!
//! Each runs at 250 and 1000 sweeps. The per-sweep figure at 1000 over the one
//! at 250 shows whether a sweep pays for the log: 1.0x means it does not.
//!
//! It also reports the run's arena: `arena.live_bytes` and
//! `arena.peak_bytes`, the largest view spine and the largest bytes in use
//! any run saw, and asserts `peak <= 2 * max(64 KiB, live) + 64 KiB` on each
//! run (the run arena's bound plus one op's garbage).
//!
//! Run with: `cargo run --example loop_scaling --release`

use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::Output;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::{Ctx, Engine, Expr, Function, Message, Outcome, Result};
use serde_json::{Value, json};
use std::time::Instant;

#[path = "shared/bench.rs"]
mod bench;
#[path = "shared/gate.rs"]
mod gate;

/// The context before the loop starts: carried by every sweep, read by none.
const CONTEXT_BYTES: usize = 256 * 1024;

/// (sweeps per message, timed messages).
const CONFIGS: &[(u64, usize)] = &[(250, 40), (1000, 10)];

/// The run arena's bound: twice the live view or 64 KiB, whichever is
/// larger, plus 64 KiB for one operation's garbage.
const FLOOR: usize = 64 * 1024;

/// The decide step's config, compiled once at build: two fields, and the
/// literal output folded to a path.
#[derive(FunctionConfig)]
struct DecideConfig {
    seat: Expr<u64>,
    turn: Expr<u64>,
    output: Output,
}

/// The decide step as a host function: two fields evaluated against the
/// run's view, one write.
struct Decide;

impl Function for Decide {
    type Config = DecideConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &DecideConfig) -> Result<Outcome> {
        let seat = ctx.eval(&cfg.seat)?;
        let turn = ctx.eval(&cfg.turn)?;
        let action = json!({ "seat": seat, "delta": delta(seat, turn) });
        ctx.emit(&cfg.output, OwnedDataValue::from(&action))?;
        Ok(Outcome::Ok)
    }
}

/// The decision both variants make, so both produce the same message.
fn delta(seat: u64, turn: u64) -> u64 {
    (seat * 7 + turn) % 5
}

fn workflow(sweeps: u64, by_handler: bool) -> Value {
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
    json!({
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
    })
}

fn engine(sweeps: u64, by_handler: bool) -> Engine {
    let builder = Engine::builder().workflow_json(&workflow(sweeps, by_handler));
    let builder = if by_handler {
        builder.register("decide", Decide)
    } else {
        builder
    };
    builder.build().expect("the engine builds")
}

fn message(base: &OwnedDataValue) -> Message {
    Message::from_parts(OwnedDataValue::Null, base.clone(), OwnedDataValue::Null)
}

/// The arena of one run, as the engine reports it: the largest view spine and the
/// largest bytes in use.
#[derive(Clone, Copy, Default)]
struct Arena {
    live: usize,
    peak: usize,
}

impl Arena {
    /// Takes the run's counters and holds them to the run arena's bound.
    fn of(run: &dataflow_rs::Run, sweeps: u64) -> Self {
        let stats = run.arena_stats();
        let arena = Self {
            live: stats.live_max,
            peak: stats.peak_arena,
        };
        let bound = 2 * FLOOR.max(arena.live) + FLOOR;
        assert!(
            arena.peak <= bound,
            "{sweeps} sweeps: the run arena peaked at {} bytes, over the bound {bound} \
             (live {} bytes, {} views, {} checkpoints)",
            arena.peak,
            arena.live,
            stats.views,
            stats.checkpoints
        );
        arena
    }

    fn max(self, other: Self) -> Self {
        Self {
            live: self.live.max(other.live),
            peak: self.peak.max(other.peak),
        }
    }
}

/// Runs one message and checks the loop did its work: one log entry and
/// three audit entries per sweep, and a score that adds every decision.
async fn check(engine: &Engine, base: &OwnedDataValue, sweeps: u64) -> Arena {
    let mut message = message(base);
    let run = engine.run(&mut message).await;
    let arena = Arena::of(&run, sweeps);
    run.into_result().expect("the loop runs");
    assert!(
        message.errors().is_empty(),
        "errors: {:?}",
        message.errors()
    );
    let data = Value::from(message.context().data());
    let log = data["log"].as_array().expect("data.log is an array");
    assert_eq!(log.len() as u64, sweeps, "one log entry per sweep");
    let expected: u64 = (0..sweeps).map(|turn| delta(turn % 8, turn)).sum();
    assert_eq!(
        data["state"]["score"].as_f64(),
        Some(expected as f64),
        "the score adds every decision"
    );
    assert_eq!(
        message.audit().len() as u64,
        3 * sweeps,
        "three tasks per sweep"
    );
    arena
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
    let mut arena = Arena::default();
    let mut gate = gate::Gate::new("loop_scaling");
    for &(sweeps, messages) in CONFIGS {
        let messages = gate::count(messages, 1);
        for by_handler in [false, true] {
            let engine = engine(sweeps, by_handler);
            arena = arena.max(check(&engine, &base, sweeps).await);

            let mut samples = bench::Samples::with_capacity(messages);
            for _ in 0..messages {
                let mut message = message(&base);
                let start = Instant::now();
                let run = engine.run(&mut message).await;
                samples.push(start.elapsed());
                arena = arena.max(Arena::of(&run, sweeps));
                run.into_result().expect("the loop runs");
            }
            let how = if by_handler { "handler" } else { "map" };
            let label = format!("{sweeps:>4} sweeps, decide by {how}");
            let row = samples.row(&label, sweeps as usize, "sweep");
            gate.row(&format!("sweeps={sweeps}.{how}"), row, "µs/sweep");
            means.push((sweeps, by_handler, row.0));
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
    println!(
        "Arena: live {} bytes, peak {} bytes (bound {} bytes)",
        arena.live,
        arena.peak,
        2 * FLOOR.max(arena.live) + FLOOR
    );
    gate.lower("arena.live_bytes", arena.live as f64, "bytes")
        .lower("arena.peak_bytes", arena.peak as f64, "bytes")
        .emit();
}
