//! Capturing what the `log` built-in emits. It logs under target
//! `dataflow::log` and asks `log_enabled!` first, so the level a case enables
//! decides whether its message is evaluated at all, which 02's `log` rows pin.
//!
//! One global logger, installed once. Outside a capture it enables nothing for
//! that target. Recording is single-threaded, so one capture at a time.

use dataflow_corpus::Json;
use log::{Level, LevelFilter, Log, Metadata, Record};
use std::sync::{Mutex, Once};

const TARGET: &str = "dataflow::log";

struct State {
    level: Option<Level>,
    lines: Vec<Json>,
}

static STATE: Mutex<State> = Mutex::new(State {
    level: None,
    lines: Vec::new(),
});
static INSTALL: Once = Once::new();
static CAPTURE: Capture = Capture;

struct Capture;

impl Log for Capture {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.target() == TARGET
            && STATE
                .lock()
                .expect("the log state lock")
                .level
                .is_some_and(|level| metadata.level() <= level)
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            let line = Json::Object(vec![
                ("level".to_owned(), Json::str(record.level().as_str())),
                ("message".to_owned(), Json::str(record.args().to_string())),
            ]);
            STATE.lock().expect("the log state lock").lines.push(line);
        }
    }

    fn flush(&self) {}
}

/// Start capturing target `dataflow::log` up to `level` (`error` to `trace`),
/// or enable nothing when `level` is `None`.
pub fn begin(level: Option<&str>) {
    INSTALL.call_once(|| {
        log::set_logger(&CAPTURE).expect("no other logger is installed");
        log::set_max_level(LevelFilter::Trace);
    });
    let level = level.map(|l| {
        l.parse()
            .unwrap_or_else(|_| panic!("unknown log level {l}"))
    });
    let mut state = STATE.lock().expect("the log state lock");
    state.level = level;
    state.lines.clear();
}

/// Stop capturing, and return the lines kept since `begin`.
pub fn end() -> Vec<Json> {
    let mut state = STATE.lock().expect("the log state lock");
    state.level = None;
    std::mem::take(&mut state.lines)
}
