//! The one machine-readable line every gate benchmark prints, read by
//! `bench/gate.py`. The human-readable output above it is unchanged; a v4 port
//! of a benchmark keeps these metric names, so the 13-row comparison stays
//! mechanical.
//!
//! Each benchmark compiles this file as its own module and none uses every
//! method, hence the module-wide dead-code allowance.

#![allow(dead_code)]

use serde_json::{Value, json};

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

    /// Prints `GATE {...}` on its own line. Called once, last.
    pub fn emit(&self) {
        println!(
            "GATE {}",
            json!({ "bench": self.bench, "metrics": self.metrics })
        );
    }
}
