//! The one machine-readable line every gate benchmark prints, read by
//! `bench/gate.py`. The human-readable output above it is unchanged; a
//! benchmark keeps these metric names from build to build, so the 13-row A/B
//! comparison stays mechanical.
//!
//! Under `DATAFLOW_BENCH_SMOKE=1` a benchmark runs smoke: its startup check
//! in full, then a few iterations ([`count`]), then its `GATE` line marked
//! `"smoke": true`. CI runs every example so (`bench/gate.py smoke`), which
//! is what keeps each benchmark's startup check, its guard against a workload
//! that silently does less than it says, from going unrun. `gate.py` refuses
//! a smoke line as a measurement.
//!
//! Each benchmark compiles this file as its own module and none uses every
//! method, hence the module-wide dead-code allowance.

#![allow(dead_code)]

use serde_json::{Value, json};

/// The variable that turns smoke mode on: `1`, and no other value.
pub const SMOKE: &str = "DATAFLOW_BENCH_SMOKE";

/// Whether this run is a smoke run.
pub fn smoke() -> bool {
    std::env::var_os(SMOKE).is_some_and(|v| v == "1")
}

/// An iteration count: `full`, or `smoke` in a smoke run. Only counts of
/// repetitions shrink; a shape a startup check depends on never does.
pub fn count(full: usize, smoke: usize) -> usize {
    if self::smoke() { smoke } else { full }
}

pub struct Gate {
    bench: &'static str,
    metrics: Vec<Value>,
}

impl Gate {
    pub fn new(bench: &'static str) -> Self {
        Self {
            bench,
            metrics: Vec::new(),
        }
    }

    /// A latency or a cost per unit: lower is better.
    pub fn lower(&mut self, name: impl Into<String>, value: f64, unit: &str) -> &mut Self {
        self.push(name.into(), value, unit, "lower")
    }

    /// A throughput: higher is better.
    pub fn higher(&mut self, name: impl Into<String>, value: f64, unit: &str) -> &mut Self {
        self.push(name.into(), value, unit, "higher")
    }

    /// One `Samples` row as `<label>.mean` and `<label>.p99`, the two figures
    /// the gate judges.
    pub fn row(
        &mut self,
        label: &str,
        (mean, _p50, p99): (f64, f64, f64),
        unit: &str,
    ) -> &mut Self {
        self.lower(format!("{label}.mean"), mean, unit);
        self.lower(format!("{label}.p99"), p99, unit)
    }

    fn push(&mut self, name: String, value: f64, unit: &str, better: &str) -> &mut Self {
        assert!(value.is_finite(), "{}: {name} is {value}", self.bench);
        self.metrics
            .push(json!({ "name": name, "value": value, "unit": unit, "better": better }));
        self
    }

    /// Prints `GATE {...}` on its own line. Called once, last. A smoke run's
    /// line says `"smoke": true`; a full run's is unchanged.
    pub fn emit(&self) {
        let mut line = json!({ "bench": self.bench, "metrics": self.metrics });
        if smoke() {
            line["smoke"] = json!(true);
        }
        println!("GATE {line}");
    }
}
