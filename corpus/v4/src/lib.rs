//! v4's golden-corpus driver. In phase 2 it checks one thing: that v4 parses
//! every corpus definition, or refuses it with v3's code at v3's path. Later
//! phases extend it to builds and runs. See `corpus/README.md`.
//!
//! This crate holds the reusable pieces (reading a case's definitions,
//! normalising what v3 recorded, collecting what v4 reports);
//! `tests/definitions.rs` is the check.

use dataflow_corpus::{Case, Json};
use dataflow_rs::def::{ParseOptions, StepDef, WorkflowDef, parse};
use dataflow_rs::expr::{CompileCx, ExprEngine};
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::function::builtins::MapConfig;
use dataflow_rs::program::{AuthoredPath, IssueCode, Severity};
use serde_json::Value;

/// One issue as the corpus compares it: code, severity and absolute authored
/// path. `path` is `None` only for v3's `PARSE_FAILED`, which had no path.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Row {
    pub code: String,
    pub severity: String,
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

/// The case's raw definitions as `serde_json` values.
pub fn definitions(case: &Case) -> Vec<Value> {
    case.workflows
        .iter()
        .map(|w| serde_json::from_str(&w.to_text()).expect("a Json is valid JSON"))
        .collect()
}

/// The codes whose issues are compared even when they are only Advisory:
/// v3 reported them from `check_workflow` and v4 reports them from `parse`
/// and the `map` config.
fn always_compared(code: IssueCode) -> bool {
    matches!(
        code,
        IssueCode::GroupContinueOnError | IssueCode::NullMapping
    )
}

/// What v4 reports for one definition: the parse's issues, plus the issues of
/// compiling every `map` task's config. Only Rejected, Defect and the two
/// always-compared codes are kept.
pub fn actual(definition: &Value) -> (Option<WorkflowDef>, Vec<Row>) {
    let parsed = parse(definition, ParseOptions::default());
    let mut issues = parsed.issues.clone();
    if let Some(def) = &parsed.def {
        let engine = ExprEngine::default();
        for (path, kind, step) in def.walk() {
            let StepDef::Task(task) = step else { continue };
            if task.function.name != "map" {
                continue;
            }
            let at = path.join("function").join("input");
            let mut cx = CompileCx::new(&engine, &mut issues)
                .at_path(at)
                .for_step(kind, task.id.clone());
            MapConfig::compile(&task.function.input, &mut cx);
        }
    }
    let rows = issues
        .iter()
        .filter(|i| {
            matches!(i.severity, Severity::Rejected | Severity::Defect) || always_compared(i.code)
        })
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

/// The `checked` codes whose v4 rule comes in phase 3, with the built-in
/// configs and the lints over a whole workflow. Their issues are not compared
/// yet; the driver lists each as skipped.
pub const PHASE_3_CODES: &[&str] = &[
    "UNGUARDED_VALIDATION",
    "ESCAPED_TEMPLATE_KEY",
    "DUPLICATE_TEMPLATE_KEY",
    "SECRET_IN_MESSAGE_WRITE",
];

/// What v3 recorded for one definition.
#[derive(Debug, Default)]
pub struct Expected {
    /// The rows v4 is compared against.
    pub rows: Vec<Row>,
    /// The `checked` issues of a phase 3 code, not compared.
    pub phase_3: Vec<Row>,
    /// Any other `checked` issue: neither compared nor deferred, which the
    /// driver fails on rather than drop.
    pub uncovered: Vec<Row>,
}

/// What v3 recorded for one definition. Every `authored` issue is a row, and
/// every `checked` issue coded `GROUP_CONTINUE_ON_ERROR` or `NULL_MAPPING`,
/// its path made absolute with the authored path of its task, taken from v4's
/// walk of `def`. The other `checked` issues are sorted into `phase_3` and
/// `uncovered`, their paths made absolute the same way when `def` is `Some`.
pub fn expected(record: &Json, def: Option<&WorkflowDef>) -> Expected {
    let mut out = Expected::default();
    for issue in list(record, "authored") {
        out.rows.push(Row::new(
            text(issue, "code").expect("an issue has a code"),
            text(issue, "severity").expect("an issue has a severity"),
            text(issue, "path").map(str::to_owned),
        ));
    }
    for issue in list(record, "checked") {
        let code = text(issue, "code").expect("an issue has a code");
        let compared = code == "GROUP_CONTINUE_ON_ERROR" || code == "NULL_MAPPING";
        let task_id = text(issue, "task_id").expect("a checked issue names its task");
        let relative = text(issue, "path").expect("a checked issue has a path");
        let path = match def {
            Some(def) => format!("{}.{relative}", step_path(def, task_id)),
            None if compared => panic!("a checked issue needs a parsed definition"),
            None => format!("{relative} on step `{task_id}`"),
        };
        let row = Row::new(
            code,
            text(issue, "severity").expect("an issue has a severity"),
            Some(path),
        );
        if compared {
            out.rows.push(row);
        } else if PHASE_3_CODES.contains(&code) {
            out.phase_3.push(row);
        } else {
            out.uncovered.push(row);
        }
    }
    out
}

/// The authored path of the step with this id.
pub fn step_path(def: &WorkflowDef, id: &str) -> AuthoredPath {
    def.walk()
        .find(|(_, _, step)| step.id() == id)
        .map(|(path, _, _)| path)
        .unwrap_or_else(|| panic!("no step `{id}` in the definition"))
}
