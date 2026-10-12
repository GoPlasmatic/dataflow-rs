//! # Observed run
//!
//! A row of the v4 performance gate: `realistic_benchmark`'s workload with an
//! `ExecutionObserver` attached that counts tasks and workflows and never
//! reads a duration, as an always-on metrics observer that counts only events
//! does. In 3.15 an attached observer costs a clock read per task and per
//! workflow whether it uses the time or not. v4 reads the clock only for an
//! observer that asks for timings, so the gate requires the observed row to be
//! faster.
//!
//! The workload runs twice: unobserved, which is `realistic_benchmark`, and
//! observed. The unobserved row shows what observing costs on this line.
//!
//! Run with: `cargo run --example observed_run --release`

use dataflow_rs::{Engine, ExecutionObserver, Message, TaskEvent, WorkflowFinished};
use datavalue::OwnedDataValue;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "shared/bench.rs"]
mod bench;
#[path = "shared/gate.rs"]
mod gate;
#[path = "shared/iso20022.rs"]
mod iso20022;

const TOTAL_MESSAGES: usize = 500_000;
const WARMUP_MESSAGES: usize = 5_000;

/// Counts events and never reads a duration.
#[derive(Default)]
struct Counts {
    tasks: AtomicU64,
    workflows: AtomicU64,
}

impl ExecutionObserver for Counts {
    fn task_finished(&self, _event: &TaskEvent<'_>) {
        self.tasks.fetch_add(1, Ordering::Relaxed);
    }

    fn workflow_finished(&self, _event: &WorkflowFinished<'_>) {
        self.workflows.fetch_add(1, Ordering::Relaxed);
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let payload = Arc::new(OwnedDataValue::from(&iso20022::payload()));
    let unobserved = Arc::new(
        Engine::builder()
            .with_workflow(iso20022::workflow())
            .build()
            .expect("the engine builds"),
    );
    let counts = Arc::new(Counts::default());
    let observed = Arc::new(
        Engine::builder()
            .with_workflow(iso20022::workflow())
            .with_observer(counts.clone())
            .build()
            .expect("the engine builds"),
    );

    // The check: the workload ran, and the observer saw every task and the
    // workflow.
    let mut m = Message::new(Arc::clone(&payload));
    observed
        .process_message(&mut m)
        .await
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
        "Messages per row: {TOTAL_MESSAGES}, cores: {}\n",
        num_cpus::get()
    );
    let (mut plain, plain_tp) =
        bench::spawned(&unobserved, &payload, WARMUP_MESSAGES, TOTAL_MESSAGES).await;
    let (mut watched, watched_tp) =
        bench::spawned(&observed, &payload, WARMUP_MESSAGES, TOTAL_MESSAGES).await;
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
