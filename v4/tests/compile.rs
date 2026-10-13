extern crate dataflow_rs_v4 as dataflow_rs;

// `Compiler`: `check`, `check_steps` and `compile_one`. Each definition goes
// through `parse`, then a compiler made from a registry holding the host
// functions below. The rows are `(code, severity, path)`, in walk order.

use dataflow_rs::def::{ParseOptions, WorkflowDef, parse};
use dataflow_rs::expr::{CompileCx, Output};
use dataflow_rs::function::{
    ExprVisitor, FieldKind, FieldSpec, FunctionConfig, FunctionSpec, Prepare, Reference, Templated,
    ValueKind,
};
use dataflow_rs::program::{
    CompileOptions, Compiler, Issue, IssueCode as C, ResourceOracle, Severity, StepKind,
};
use dataflow_rs::{Ctx, Expr, Function, Outcome, Registry, Result};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const R: Severity = Severity::Rejected;
const D: Severity = Severity::Defect;

#[derive(FunctionConfig)]
struct FetchConfig {
    #[config(records)]
    url: Expr<String>,
    #[output(default = "data.response")]
    output: Output,
}

/// A host function with a derived config.
struct Fetch;

impl Function for Fetch {
    type Config = FetchConfig;

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &FetchConfig) -> Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

#[derive(FunctionConfig)]
struct QueryConfig {
    connector: Option<Expr<String>>,
    limit: Option<u32>,
}

/// A host function whose `connector` names a connector.
struct Query;

impl Function for Query {
    type Config = QueryConfig;

    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default().with_references(vec![Reference::new("connector", "connector")])
    }

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &QueryConfig) -> Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

#[derive(FunctionConfig)]
struct AuthConfig {
    connector: String,
}

#[derive(FunctionConfig)]
struct LoginConfig {
    auth: AuthConfig,
}

/// A host function whose reference sits in a nested config.
struct Login;

impl Function for Login {
    type Config = LoginConfig;

    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default().with_references(vec![Reference::new("auth.connector", "connector")])
    }

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &LoginConfig) -> Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

/// Refuses every connector in `prepare`, and counts its calls.
struct Strict {
    prepared: Arc<AtomicUsize>,
}

impl Function for Strict {
    type Config = QueryConfig;

    fn prepare(
        &self,
        _cfg: &mut QueryConfig,
        p: &Prepare<'_>,
    ) -> std::result::Result<(), Vec<Issue>> {
        self.prepared.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            (p.name(), p.workflow_id(), p.task_id()),
            ("strict", "w", "t")
        );
        Err(vec![Issue::new(
            C::InputParse,
            "connector".parse().expect("an authored path"),
            "no connector by that name",
        )])
    }

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &QueryConfig) -> Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

/// A plugin whose fields its manifest declares at runtime.
struct Plugin;

impl Function for Plugin {
    type Config = Templated;

    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default().with_runtime_fields(vec![
            FieldSpec::runtime("record", FieldKind::Interpolated, true),
            FieldSpec::runtime("label", FieldKind::Literal(ValueKind::String), true),
        ])
    }

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &Templated) -> Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

/// A hand-written config that breaks its contract: it refuses every input
/// and records no issue.
struct Refuses;

impl FunctionConfig for Refuses {
    fn compile(_input: &Value, _cx: &mut CompileCx<'_>) -> Option<Self> {
        None
    }

    fn visit(&self, _v: &mut dyn ExprVisitor) {}

    fn fields() -> &'static [FieldSpec] {
        &[]
    }
}

/// A host function whose config refuses without saying why.
struct Silent;

impl Function for Silent {
    type Config = Refuses;

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &Refuses) -> Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

/// Knows only the connectors it was given.
struct Known(&'static [&'static str]);

impl ResourceOracle for Known {
    fn resolve(&self, kind: &str, name: &str) -> bool {
        kind == "connector" && self.0.contains(&name)
    }
}

fn registry() -> Registry {
    let mut registry = Registry::default();
    registry.register("fetch", Fetch);
    registry.register("query", Query);
    registry.register("login", Login);
    registry.register("plugin", Plugin);
    registry.register("silent", Silent);
    registry
}

fn compiler() -> Compiler {
    registry().compiler(CompileOptions::default())
}

/// A compiler whose oracle knows only `db1`.
fn with_oracle() -> Compiler {
    registry().compiler(CompileOptions::default().resources(Arc::new(Known(&["db1"]))))
}

/// A workflow `w` holding `tasks`.
fn wf(tasks: Value) -> Value {
    json!({"id": "w", "tasks": tasks})
}

/// A workflow `w` whose one task `t` calls `name` with `input`.
fn one(name: &str, input: Value) -> Value {
    wf(json!([{"id": "t", "function": {"name": name, "input": input}}]))
}

/// The definition, which must parse.
fn def(json: &Value) -> WorkflowDef {
    let parsed = parse(json, ParseOptions::default());
    parsed
        .def
        .unwrap_or_else(|| panic!("the definition parses: {:?}", parsed.issues))
}

type Row = (C, Severity, String);

fn rows(issues: &[Issue]) -> Vec<Row> {
    issues
        .iter()
        .map(|i| (i.code, i.severity, i.path.to_string()))
        .collect()
}

fn row(code: C, severity: Severity, path: &str) -> Row {
    (code, severity, path.to_string())
}

/// An issue's step stamp and field.
fn stamp(issue: &Issue) -> (Option<StepKind>, Option<&str>, Option<&str>) {
    (
        issue.step_kind,
        issue.step_id.as_deref(),
        issue.field.as_deref(),
    )
}

/// `compile_one`'s issues, which must be an `Err`; `check` returns the same.
fn rejected(compiler: &Compiler, json: &Value) -> Vec<Issue> {
    let def = def(json);
    let issues = compiler
        .compile_one(&def)
        .expect_err("the workflow is rejected");
    assert_eq!(compiler.check(&def), issues);
    issues
}

/// `compile_one`'s issues, which must be `Ok`; `check` returns the same.
fn compiled(compiler: &Compiler, json: &Value) -> Vec<Issue> {
    let def = def(json);
    let compiled = compiler.compile_one(&def).expect("the workflow compiles");
    assert_eq!(compiled.workflow.id(), "w");
    assert_eq!(compiler.check(&def), compiled.issues);
    compiled.issues
}

/// `n` nested `!`s around a `var`: past datalogic's depth limit of 256, the
/// one pure-data compile error (phase 2's finding).
fn too_deep() -> Value {
    let mut rule = json!({"var": "data.x"});
    for _ in 0..300 {
        rule = json!({"!": rule});
    }
    rule
}

#[test]
fn a_clean_workflow_compiles_with_no_issue() {
    let json = wf(json!([
        {"id": "a", "function": {"name": "map", "input": {"mappings": [
            {"path": "data.x", "logic": {"var": "data.y"}}
        ]}}},
        {"id": "b", "function": {"name": "log", "input": {"message": "hi"}}},
        {"id": "c", "function": {"name": "fetch", "input": {"url": "https://x"}}},
    ]));
    assert!(compiled(&compiler(), &json).is_empty());
}

#[test]
fn an_unregistered_function_is_unknown_on_its_own_task() {
    let json = wf(json!([
        {"id": "t1", "function": {"name": "fetch", "input": {"url": "https://x"}}},
        {"id": "t2", "function": {"name": "http_call", "input": {"url": "https://x"}}},
    ]));
    let issues = rejected(&compiler(), &json);
    assert_eq!(
        rows(&issues),
        [row(C::UnknownFunction, R, "tasks[1].function.name")]
    );
    assert_eq!(stamp(&issues[0]), (Some(StepKind::Task), Some("t2"), None));
}

#[test]
fn a_host_config_reports_every_issue_at_its_field() {
    let issues = rejected(&compiler(), &one("fetch", json!({"uri": "x"})));
    // A typo inside a config is Rejected (I23).
    assert_eq!(
        rows(&issues),
        [
            row(C::LikelyTypo, R, "tasks[0].function.input.uri"),
            row(C::InputParse, R, "tasks[0].function.input.url"),
        ]
    );
    assert_eq!(
        stamp(&issues[0]),
        (Some(StepKind::Task), Some("t"), Some("uri"))
    );
    assert_eq!(
        stamp(&issues[1]),
        (Some(StepKind::Task), Some("t"), Some("url"))
    );
}

#[test]
fn a_builtin_config_reports_at_its_field() {
    let issues = rejected(&compiler(), &one("log", json!({"level": "loud"})));
    assert_eq!(
        rows(&issues),
        [
            row(C::InputParse, R, "tasks[0].function.input.level"),
            row(C::InputParse, R, "tasks[0].function.input.message"),
        ]
    );
    assert_eq!(
        stamp(&issues[1]),
        (Some(StepKind::Task), Some("t"), Some("message"))
    );
}

#[test]
fn a_setup_task_is_stamped_setup() {
    let json = json!({"id": "w",
        "loop": {"max": 3, "setup": [
            {"id": "s", "function": {"name": "fetch", "input": {}}}
        ]},
        "tasks": [{"id": "t", "function": {"name": "fetch", "input": {"url": "https://x"}}}],
    });
    let issues = rejected(&compiler(), &json);
    assert_eq!(
        rows(&issues),
        [row(C::InputParse, R, "loop.setup[0].function.input.url")]
    );
    assert_eq!(
        stamp(&issues[0]),
        (Some(StepKind::Setup), Some("s"), Some("url"))
    );
}

#[test]
fn prepare_reports_below_the_input_and_runs_only_when_the_config_compiled() {
    let prepared = Arc::new(AtomicUsize::new(0));
    let mut registry = Registry::default();
    registry.register(
        "strict",
        Strict {
            prepared: Arc::clone(&prepared),
        },
    );
    let compiler = registry.compiler(CompileOptions::default());

    let issues = rejected(&compiler, &one("strict", json!({"connector": "db1"})));
    assert_eq!(
        rows(&issues),
        [row(C::InputParse, R, "tasks[0].function.input.connector")]
    );
    assert_eq!(
        stamp(&issues[0]),
        (Some(StepKind::Task), Some("t"), Some("connector"))
    );
    // Once for `compile_one`, once for `check`.
    assert_eq!(prepared.load(Ordering::SeqCst), 2);

    let issues = rejected(&compiler, &one("strict", json!({"limit": "many"})));
    assert_eq!(
        rows(&issues),
        [row(C::InputParse, R, "tasks[0].function.input.limit")]
    );
    assert_eq!(prepared.load(Ordering::SeqCst), 2);
}

#[test]
fn an_unresolved_literal_reference_is_a_defect() {
    let issues = compiled(&with_oracle(), &one("query", json!({"connector": "db2"})));
    assert_eq!(
        rows(&issues),
        [row(
            C::UnresolvedReference,
            D,
            "tasks[0].function.input.connector"
        )]
    );
    assert_eq!(
        stamp(&issues[0]),
        (Some(StepKind::Task), Some("t"), Some("connector"))
    );
    assert!(
        issues[0].message.contains("connector"),
        "{}",
        issues[0].message
    );
    assert!(issues[0].message.contains("db2"), "{}", issues[0].message);

    assert!(compiled(&with_oracle(), &one("query", json!({"connector": "db1"}))).is_empty());
}

#[test]
fn a_reference_in_a_nested_field_is_checked_at_its_path() {
    let issues = compiled(
        &with_oracle(),
        &one("login", json!({"auth": {"connector": "db2"}})),
    );
    assert_eq!(
        rows(&issues),
        [row(
            C::UnresolvedReference,
            D,
            "tasks[0].function.input.auth.connector"
        )]
    );
    assert_eq!(
        stamp(&issues[0]),
        (Some(StepKind::Task), Some("t"), Some("auth"))
    );
}

#[test]
fn only_a_literal_reference_in_a_compiled_config_under_an_oracle_is_checked() {
    let oracle = with_oracle();
    // Computed, or absent: nothing to check at compile.
    assert!(
        compiled(
            &oracle,
            &one("query", json!({"connector": {"var": "data.c"}}))
        )
        .is_empty()
    );
    assert!(compiled(&oracle, &one("query", json!({}))).is_empty());
    // A config that failed is not checked.
    let issues = rejected(
        &oracle,
        &one("query", json!({"connector": "db2", "limit": "many"})),
    );
    assert_eq!(
        rows(&issues),
        [row(C::InputParse, R, "tasks[0].function.input.limit")]
    );
    // With no oracle, nothing is checked.
    assert!(compiled(&compiler(), &one("query", json!({"connector": "db2"}))).is_empty());
}

#[test]
fn a_condition_that_does_not_compile_is_template_compile() {
    let json = wf(json!([
        {"id": "t", "condition": too_deep(), "function": {"name": "log", "input": {"message": "m"}}},
    ]));
    let issues = rejected(&compiler(), &json);
    assert_eq!(
        rows(&issues),
        [row(C::TemplateCompile, R, "tasks[0].condition")]
    );
    assert_eq!(stamp(&issues[0]), (Some(StepKind::Task), Some("t"), None));
}

#[test]
fn every_expression_site_reports_a_compile_failure_at_its_path() {
    let json = json!({"id": "w",
        "condition": too_deep(),
        "loop": {"max": 3, "over": too_deep()},
        "tasks": [
            {"id": "g", "condition": too_deep(), "tasks": [
                {"id": "f", "for_each": {"over": too_deep(), "as": "item"},
                 "function": {"name": "fetch", "input": {"url": "https://x"}}},
            ]},
        ],
    });
    let issues = rejected(&compiler(), &json);
    assert_eq!(
        rows(&issues),
        [
            row(C::TemplateCompile, R, "condition"),
            row(C::TemplateCompile, R, "loop.over"),
            row(C::TemplateCompile, R, "tasks[0].condition"),
            row(C::TemplateCompile, R, "tasks[0].tasks[0].for_each.over"),
        ]
    );
    assert_eq!(stamp(&issues[0]), (None, None, None));
    assert_eq!(stamp(&issues[1]), (None, None, None));
    assert_eq!(stamp(&issues[2]), (Some(StepKind::Group), Some("g"), None));
    assert_eq!(stamp(&issues[3]), (Some(StepKind::Task), Some("f"), None));
}

#[test]
fn a_templated_plugin_compiles_against_its_runtime_fields() {
    let clean = one("plugin", json!({"record": {"a": 1}, "label": "x"}));
    assert!(compiled(&compiler(), &clean).is_empty());

    let issues = rejected(&compiler(), &one("plugin", json!({"record": {"a": 1}})));
    assert_eq!(
        rows(&issues),
        [row(C::InputParse, R, "tasks[0].function.input.label")]
    );
}

/// A task with a near-miss key `parse` reports, and a config `fetch` refuses.
fn bad_task() -> Value {
    json!([{"id": "a", "conditon": true, "function": {"name": "fetch", "input": {"uri": "x"}}}])
}

#[test]
fn check_steps_reports_a_fragment_under_its_root() {
    let issues = compiler().check_steps(&bad_task(), "tasks[2].tasks");
    // The parse issues, then the compile issues.
    assert_eq!(
        rows(&issues),
        [
            row(C::LikelyTypo, D, "tasks[2].tasks[0].conditon"),
            row(C::LikelyTypo, R, "tasks[2].tasks[0].function.input.uri"),
            row(C::InputParse, R, "tasks[2].tasks[0].function.input.url"),
        ]
    );
    for issue in &issues {
        assert_eq!(issue.step_kind, Some(StepKind::Task), "{issue}");
        assert_eq!(issue.step_id.as_deref(), Some("a"), "{issue}");
    }
}

#[test]
fn check_steps_under_loop_setup_makes_setup_steps() {
    let issues = compiler().check_steps(&bad_task(), "loop.setup");
    assert_eq!(
        rows(&issues),
        [
            row(C::LikelyTypo, D, "loop.setup[0].conditon"),
            row(C::LikelyTypo, R, "loop.setup[0].function.input.uri"),
            row(C::InputParse, R, "loop.setup[0].function.input.url"),
        ]
    );
    assert!(
        issues.iter().all(|i| i.step_kind == Some(StepKind::Setup)),
        "{issues:?}"
    );
}

#[test]
fn check_steps_compiles_nothing_when_the_fragment_does_not_parse() {
    let steps = json!([
        {"function": {"name": "fetch", "input": {"uri": "x"}}},
    ]);
    assert_eq!(
        rows(&compiler().check_steps(&steps, "tasks")),
        [row(C::MissingStepId, R, "tasks[0].id")]
    );
}

#[test]
fn check_steps_refuses_a_fragment_that_is_not_an_array() {
    assert_eq!(
        rows(&compiler().check_steps(&json!({"id": "a"}), "tasks[2].tasks")),
        [row(C::ParseFailed, R, "tasks[2].tasks")]
    );
}

#[test]
fn check_steps_refuses_a_root_that_is_not_an_authored_path() {
    assert_eq!(
        rows(&compiler().check_steps(&bad_task(), "tasks[x")),
        [row(C::ParseFailed, R, "")]
    );
}

#[test]
fn a_config_that_refuses_without_an_issue_is_reported_not_dropped() {
    let issues = rejected(&compiler(), &one("silent", json!({"a": 1})));
    assert_eq!(
        rows(&issues),
        [row(C::InputParse, R, "tasks[0].function.input")]
    );
    assert_eq!(stamp(&issues[0]), (Some(StepKind::Task), Some("t"), None));
}

#[test]
fn a_config_that_reports_its_refusal_gets_no_further_issue() {
    // A derived host config, a built-in's and a runtime-declared one each
    // report their own issue, and nothing is added for the refusal.
    for (name, input, field) in [
        ("fetch", json!({}), "url"),
        ("log", json!({}), "message"),
        ("plugin", json!({"label": "x"}), "record"),
    ] {
        let issues = rejected(&compiler(), &one(name, input));
        assert_eq!(
            rows(&issues),
            [row(
                C::InputParse,
                R,
                &format!("tasks[0].function.input.{field}")
            )],
            "{name}"
        );
    }
}
