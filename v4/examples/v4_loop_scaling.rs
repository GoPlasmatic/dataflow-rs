//! # Loop scaling
//!
//! v4's port of v3's `loop_scaling`, a row of the v4 performance gate: the
//! shape of #66 to #69, from Orion#371. A turn loop runs 1000 sweeps over a
//! large context and appends one entry to a log every sweep, so the context
//! grows as the loop runs. Each sweep observes (a `map`), decides (a `map`),
//! then steps (a `map` that updates the score and appends to `data.log`).
//!
//! In 3.15 every sweep converts the whole context into the arena again, once
//! per sync stretch, so a sweep pays for the context's bytes. v4 keeps one
//! view of the context for the whole run and splices each write into it, so
//! the gate requires this row to be faster.
//!
//! This port runs the "by map" variant only; phase 5 adds "by handler", when
//! a host handler can be registered. It runs at 250 and 1000 sweeps, and the
//! per-sweep figure at 1000 over the one at 250 shows whether a sweep pays
//! for the log: 1.0x means it does not.
//!
//! It also reports the run's arena (I72): `arena.live_bytes` and
//! `arena.peak_bytes`, the largest view spine and the largest bytes in use
//! any run saw, and asserts `peak <= 2 * max(64 KiB, live) + 64 KiB` on each
//! run (05's bound plus one op's garbage).
//!
//! Run with: `cargo run -p dataflow-rs-v4 --example v4_loop_scaling --release`

use dataflow_rs_v4::datavalue::OwnedDataValue;
use dataflow_rs_v4::{Engine, Message};
use serde_json::{Value, json};
use std::time::Instant;

#[path = "shared/bench.rs"]
mod bench;
#[path = "../../examples/shared/gate.rs"]
mod gate;

/// The context before the loop starts: carried by every sweep, read by none.
const CONTEXT_BYTES: usize = 256 * 1024;

/// (sweeps per message, timed messages).
const CONFIGS: &[(u64, usize)] = &[(250, 40), (1000, 10)];

/// 05's bound on the run arena: twice the live view or 64 KiB, whichever is
/// larger, plus 64 KiB for one operation's garbage.
const FLOOR: usize = 64 * 1024;

/// The decision the decide step makes, so the check can recompute the score.
fn delta(seat: u64, turn: u64) -> u64 {
    (seat * 7 + turn) % 5
}

fn workflow(sweeps: u64) -> Value {
    let decide = json!({ "id": "decide", "name": "Decide", "function": { "name": "map", "input": {
        "mappings": [{ "path": "temp_data.action", "logic": {
            "seat": { "var": "temp_data.obs.seat" },
            "delta": { "%": [
                { "+": [{ "*": [{ "var": "temp_data.obs.seat" }, 7] }, { "var": "temp_data.turn" }] },
                5
            ]}
        }}]
    }}});
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

fn engine(sweeps: u64) -> Engine {
    Engine::builder()
        .workflow_json(&workflow(sweeps))
        .build()
        .expect("the engine builds")
}

fn message(base: &OwnedDataValue) -> Message {
    Message::from_parts(OwnedDataValue::Null, base.clone(), OwnedDataValue::Null)
}

/// The arena of one run, as I72 reports it: the largest view spine and the
/// largest bytes in use.
#[derive(Clone, Copy, Default)]
struct Arena {
    live: usize,
    peak: usize,
}

impl Arena {
    /// Takes the run's counters and holds them to 05's bound.
    fn of(run: &dataflow_rs_v4::Run, sweeps: u64) -> Self {
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
        let engine = engine(sweeps);
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
        let label = format!("{sweeps:>4} sweeps, decide by map");
        let row = samples.row(&label, sweeps as usize, "sweep");
        gate.row(&format!("sweeps={sweeps}.map"), row, "µs/sweep");
        means.push((sweeps, row.0));
    }

    println!();
    let mean_at = |sweeps: u64| {
        means
            .iter()
            .find(|row| row.0 == sweeps)
            .map(|row| row.1)
            .expect("both sizes ran")
    };
    println!(
        "Growth, decide by map    : µs/sweep at 1000 over 250 = {:.2}x",
        mean_at(1000) / mean_at(250)
    );
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
