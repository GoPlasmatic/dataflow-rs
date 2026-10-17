//! What the end-to-end run tests share: an engine built from JSON
//! definitions, one message run over it, the message's normalised wire text,
//! and the golden files those texts are held to.
//!
//! The text is normalised as the corpus normalises it: `processed_at`,
//! `engine_version` and the audit and error timestamps. A golden file,
//! `tests/golden/<dir>/<test>.json`, holds every run a test makes, in order,
//! as `{code, message}`, and is compared key order and all.
//! `DATAFLOW_GOLDEN_WRITE=1` rewrites it from the run.

// Each test binary that includes this file uses a part of it.
#![allow(dead_code)]

use dataflow_corpus::json::Json;
use dataflow_corpus::normalise;
use dataflow_rs::run::{AuditMode, EngineBuilder, RunStatus};
use dataflow_rs::{Engine, Message, RunOptions};
use datavalue::OwnedDataValue;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// One run: the message's `data` and the run's options.
#[derive(Clone, Default)]
pub struct Run {
    pub data: Value,
    pub audit: Option<usize>,
    /// `AuditMode::Off`, over `audit`.
    pub audit_off: bool,
    /// Change capture off.
    pub capture_off: bool,
}

/// A run over `data`.
pub fn data(data: Value) -> Run {
    Run {
        data,
        ..Run::default()
    }
}

/// The engine over `workflows`, or the issues it rejected them with.
pub fn build(workflows: &[Value]) -> Result<Engine, String> {
    build_with(workflows, |b| b)
}

/// [`build`] with `register` adding host functions to the builder.
pub fn build_with(
    workflows: &[Value],
    register: impl FnOnce(EngineBuilder) -> EngineBuilder,
) -> Result<Engine, String> {
    workflows
        .iter()
        .fold(register(Engine::builder()), |b, w| b.workflow_json(w))
        .build()
        .map_err(|issues| format!("{issues:?}"))
}

/// One finished run: the message's normalised wire text, the outcome's code
/// (`ok`, or the stopping error's), and the run's report.
pub struct Ran {
    pub text: String,
    pub code: String,
    pub report: dataflow_rs::Run,
}

impl Ran {
    /// The normalised message as JSON.
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.text).expect("a normalised message parses")
    }
}

/// `run` over `engine`, the message's id `m`.
pub async fn run_on(engine: &Engine, run: &Run) -> Ran {
    let mut message = Message::from_parts(
        OwnedDataValue::Null,
        OwnedDataValue::from(&run.data),
        OwnedDataValue::Null,
    )
    .with_id("m");
    let mut opts = RunOptions::default();
    if let Some(n) = run.audit {
        opts = opts.audit(AuditMode::Last(n));
    }
    if run.audit_off {
        opts = opts.audit(AuditMode::Off);
    }
    if run.capture_off {
        opts = opts.capture_changes(false);
    }
    let report = engine.run_with(&mut message, &opts).await;
    let code = match report.status() {
        RunStatus::Stopped(e) => e.code().to_owned(),
        _ => "ok".to_owned(),
    };
    Ran {
        text: normalised(&message),
        code,
        report,
    }
}

/// The message's wire text, normalised.
pub fn normalised(message: &Message) -> String {
    let text = serde_json::to_string(message).expect("a message serializes");
    let mut json = Json::from_text(&text).expect("a message parses");
    normalise::message(&mut json, None);
    json.to_text()
}

/// The runs one test makes, held to its golden file by [`Golden::check`].
pub struct Golden {
    file: PathBuf,
    runs: Vec<Json>,
}

impl Golden {
    /// The runs of test `name`, held to `tests/golden/<dir>/<name>.json`.
    pub fn new(dir: &str, name: &str) -> Self {
        Self {
            file: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/golden")
                .join(dir)
                .join(format!("{name}.json")),
            runs: Vec::new(),
        }
    }

    /// Adds `ran` as the next run.
    pub fn push(&mut self, ran: &Ran) {
        let mut run = Json::object();
        run.insert("code", Json::str(ran.code.as_str()));
        run.insert(
            "message",
            Json::from_text(&ran.text).expect("a normalised message parses"),
        );
        self.runs.push(run);
    }

    /// Compares the runs with the golden file, key order included, or
    /// rewrites it under `DATAFLOW_GOLDEN_WRITE=1`.
    pub fn check(self) {
        let got = Json::Array(self.runs);
        let file = self.file.display();
        if std::env::var_os("DATAFLOW_GOLDEN_WRITE").is_some_and(|v| v == "1") {
            std::fs::create_dir_all(self.file.parent().expect("a golden file has a directory"))
                .expect("the golden directory is writable");
            std::fs::write(&self.file, got.to_pretty()).expect("the golden file is writable");
            return;
        }
        let text = std::fs::read_to_string(&self.file)
            .unwrap_or_else(|e| panic!("{file}: {e}; DATAFLOW_GOLDEN_WRITE=1 writes it"));
        let want = Json::from_text(&text).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(
            got == want,
            "the runs differ from {file} (DATAFLOW_GOLDEN_WRITE=1 rewrites it):\n{}",
            got.to_pretty()
        );
    }
}
