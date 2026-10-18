//! # Observed run
//!
//! A row of the performance gate, its workload first written against 3.x:
//! `realistic_benchmark`'s workload with an `Observer` attached that counts
//! tasks and workflows and never reads a duration, as an always-on metrics
//! observer that counts only events does. In 3.x an attached observer cost
//! a clock read per task and per workflow whether it used the time or not;
//! the engine reads the clock only for an observer whose interest holds
//! `TIMINGS`. The gate judges the row as every other: not slower.
//!
//! The workload runs twice: unobserved, which is `realistic_benchmark`, and
//! observed. The unobserved row shows what observing costs on this line.
//!
//! Run with: `cargo run --example observed_run --release`

use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::observe::{Event, Interest, Observer};
use dataflow_rs::run::EngineConfig;
use dataflow_rs::{Engine, Message};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "shared/bench.rs"]
mod bench;
#[path = "shared/gate.rs"]
mod gate;
#[path = "shared/iso20022.rs"]
mod iso20022;

fn total_messages() -> usize {
    gate::count(500_000, 200)
}
fn warmup_messages() -> usize {
    gate::count(5_000, 20)
}

/// Counts events and never reads a duration.
#[derive(Default)]
struct Counts {
    tasks: AtomicU64,
    workflows: AtomicU64,
}

impl Observer for Counts {
    /// The point of the row: no `TIMINGS`, so v4 reads no clock for this
    /// observer.
    fn interest(&self) -> Interest {
        Interest::WORKFLOWS | Interest::TASKS
    }

    fn on(&self, event: &Event<'_>) {
        match event {
            Event::TaskFinished { .. } => {
                self.tasks.fetch_add(1, Ordering::Relaxed);
            }
            Event::WorkflowFinished { .. } => {
                self.workflows.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let payload = Arc::new(OwnedDataValue::from(&iso20022::payload()));
    let workflow = iso20022::workflow();
    let unobserved = Engine::builder()
        .workflow_json(&workflow)
        .build()
        .expect("the engine builds");
    let counts = Arc::new(Counts::default());
    let observed = Engine::builder()
        .workflow_json(&workflow)
        .config(EngineConfig::default().observer(counts.clone()))
        .build()
        .expect("the engine builds");

    // The check: the workload ran, and the observer saw every task and the
    // workflow.
    let mut m = Message::new(Arc::clone(&payload));
    observed
        .run(&mut m)
        .await
        .into_result()
        .expect("the workflow runs");
    iso20022::check(&m);
    assert_eq!(counts.tasks.load(Ordering::Relaxed), 3, "three task events");
    assert_eq!(
        counts.workflows.load(Ordering::Relaxed),
        1,
        "one workflow event"
    );

    println!("OBSERVED RUN: the ISO 20022 workload, unobserved and with a counting observer");
    println!(
        "Messages per row: {}, cores: {}\n",
        total_messages(),
        num_cpus::get()
    );
    let (mut plain, plain_tp) =
        bench::spawned(&unobserved, &payload, warmup_messages(), total_messages()).await;
    let (mut watched, watched_tp) =
        bench::spawned(&observed, &payload, warmup_messages(), total_messages()).await;
    let plain_row = plain.row("unobserved", 1, "message");
    let watched_row = watched.row("observed, counting only", 1, "message");
    println!();
    println!("Throughput: unobserved {plain_tp:.0} msg/s, observed {watched_tp:.0} msg/s");
    println!(
        "Observing costs {:+.1}% throughput",
        (watched_tp - plain_tp) / plain_tp * 100.0
    );
    gate::Gate::new("observed_run")
        .row("unobserved", plain_row, "µs/message")
        .higher("unobserved.throughput", plain_tp, "msg/s")
        .row("observed", watched_row, "µs/message")
        .higher("observed.throughput", watched_tp, "msg/s")
        .emit();
}
