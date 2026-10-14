//! v4's golden-corpus driver. It checks two things. Phase 3's: v4 compiles
//! every corpus definition as v3 built it, reporting v3's issues at v3's
//! paths, and, for a definition a deviation covers, exactly what
//! [`expect::DEVIATED`] says. Phase 4's: v4 runs every case that needs only
//! the built-ins as v3 recorded it, and, at a deviated path, exactly what
//! [`expect::RUN_DEVIATED`] says. Phase 5's: the same for every case calling
//! `script` or fanning out, reloads and the ops budget included. See
//! `corpus/README.md`.
//!
//! This crate holds the reusable pieces (the case's compiler, reading a
//! case's definitions, normalising what v3 recorded, collecting what v4
//! reports, running a case's steps); `tests/definitions.rs` and
//! `tests/runs.rs` are the checks.

pub mod expect;
pub mod logs;
pub mod operators;
pub mod run;
pub mod script;

use dataflow_corpus::{Case, Json};
use dataflow_rs::Registry;
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::def::{ParseOptions, WorkflowDef, parse};
use dataflow_rs::expr::{ExprEngine, Secrets};
use dataflow_rs::program::{AuthoredPath, CompileOptions, Compiler};
use serde_json::Value;

/// One issue as the corpus compares it: code, severity and absolute authored
/// path. `path` is `None` only for v3's `PARSE_FAILED`, which had no path.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Row {
    /// The issue's code.
    pub code: String,
    /// The issue's severity.
    pub severity: String,
    /// The issue's absolute authored path, as it displays.
    pub path: Option<String>,
}

impl Row {
    fn new(code: &str, severity: &str, path: Option<String>) -> Self {
        Self {
            code: code.to_owned(),
            severity: severity.to_owned(),
            path,
        }
    }
}

impl From<&(&str, &str, &str)> for Row {
    /// A row of [`expect::DEVIATED`]: `(code, severity, path)`.
    fn from(&(code, severity, path): &(&str, &str, &str)) -> Self {
        Self::new(code, severity, Some(path.to_owned()))
    }
}

/// The case's raw definitions as `serde_json` values.
pub fn definitions(case: &Case) -> Vec<Value> {
    case.workflows
        .iter()
        .map(|w| serde_json::from_str(&w.to_text()).expect("a Json is valid JSON"))
        .collect()
}

/// The compiler a case's definitions compile with: an [`ExprEngine`] with the
/// case's `operators`, `secrets` and, on a `budget` build, `ops_budget`, and
/// a [`Registry`] holding `script`. A build without `budget` ignores the
/// ceiling: such a build compiles a budget case's definitions, and never
/// runs its steps, which `requires` the feature (`run::run_case` holds it
/// to that). The case's other engine options change runs, not compiles.
pub fn compiler(case: &Case) -> Compiler {
    let mut exprs = ExprEngine::builder();
    #[cfg(feature = "budget")]
    if let Some(ops) = case.engine.ops_budget {
        exprs = exprs.ops_budget(ops);
    }
    for name in &case.engine.operators {
        exprs = match name.as_str() {
            "shout" => exprs.operator("shout", operators::Shout),
            other => panic!("unknown corpus operator {other}"),
        };
    }
    if let Some(secrets) = &case.engine.secrets {
        let root: Value = serde_json::from_str(&secrets.to_text()).expect("a Json is valid JSON");
        let secrets = Secrets::new(OwnedDataValue::from(&root))
            .unwrap_or_else(|e| panic!("a case's secrets are an object: {e}"));
        exprs = exprs.secrets(secrets);
    }
    let exprs = exprs
        .build()
        .unwrap_or_else(|e| panic!("the case's engine builds: {e}"));
    let mut registry = Registry::new(exprs);
    registry.register("script", script::Script);
    registry.compiler(CompileOptions::default())
}

/// What v4 reports for one definition, unfiltered: the parse's issues, then,
/// when it parsed, the compiler's.
pub fn actual(compiler: &Compiler, definition: &Value) -> (Option<WorkflowDef>, Vec<Row>) {
    let parsed = parse(definition, ParseOptions::default());
    let mut issues = parsed.issues;
    if let Some(def) = &parsed.def {
        issues.extend(compiler.check(def));
    }
    let rows = issues
        .iter()
        .map(|i| {
            Row::new(
                i.code.as_str(),
                i.severity.as_str(),
                Some(i.path.to_string()),
            )
        })
        .collect();
    (parsed.def, rows)
}

fn text<'a>(json: &'a Json, key: &str) -> Option<&'a str> {
    match json.get(key) {
        Some(Json::String(s)) => Some(s),
        _ => None,
    }
}

fn list<'a>(json: &'a Json, key: &str) -> &'a [Json] {
    match json.get(key) {
        Some(Json::Array(items)) => items,
        _ => &[],
    }
}

/// What v3 recorded for one definition: every `authored` issue, then every
/// `checked` issue, its path made absolute with the authored path of its
/// task, taken from v4's walk of `def`. When v4 refused the definition, a
/// `checked` path names its step instead, and matches no v4 row.
pub fn expected(record: &Json, def: Option<&WorkflowDef>) -> Vec<Row> {
    let mut rows = Vec::new();
    for issue in list(record, "authored") {
        rows.push(Row::new(
            text(issue, "code").expect("an issue has a code"),
            text(issue, "severity").expect("an issue has a severity"),
            text(issue, "path").map(str::to_owned),
        ));
    }
    for issue in list(record, "checked") {
        let task_id = text(issue, "task_id").expect("a checked issue names its task");
        let relative = text(issue, "path").expect("a checked issue has a path");
        let path = match def {
            Some(def) => format!("{}.{relative}", step_path(def, task_id)),
            None => format!("{relative} on step `{task_id}`"),
        };
        rows.push(Row::new(
            text(issue, "code").expect("an issue has a code"),
            text(issue, "severity").expect("an issue has a severity"),
            Some(path),
        ));
    }
    rows
}

/// The authored path of the step with this id.
pub fn step_path(def: &WorkflowDef, id: &str) -> AuthoredPath {
    def.walk()
        .find(|(_, _, step)| step.id() == id)
        .map(|(path, _, _)| path)
        .unwrap_or_else(|| panic!("no step `{id}` in the definition"))
}
