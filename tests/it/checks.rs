// The compile checks: secrets, template keys, `payload` vars and
// `UNGUARDED_VALIDATION`.
//
// Each fixture is parsed and checked, and the `(code, severity, path)` rows of
// the codes it is about are pinned, as a multiset, to the rows it must report,
// each path absolute from the workflow's root. An empty expectation is a
// fixture that must report nothing of those codes.

use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::def::{ParseOptions, WorkflowDef, parse};
use dataflow_rs::expr::{ExprEngine, Output, Secrets};
use dataflow_rs::function::{
    FieldKind, FieldSpec, FunctionConfig, FunctionSpec, Templated, ValueKind,
};
use dataflow_rs::program::{CompileOptions, Compiler, Issue, IssueCode as C, Severity, StepKind};
use dataflow_rs::{Ctx, Expr, Function, Outcome, Registry, Result};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// The compiler
// ---------------------------------------------------------------------------

/// The secrets the engine holds: `k` is declared, `nope` is not.
fn secrets() -> Value {
    json!({"k": "v"})
}

/// A host function that takes no input, for a fan-out.
struct Host;

impl Function for Host {
    type Config = ();

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

#[derive(FunctionConfig)]
struct FetchConfig {
    /// Not recorded: handed to the function, which may read a secret.
    token: Option<Expr<String>>,
    #[output(default = "data.response")]
    output: Output,
}

/// A host function with a destination.
struct Fetch;

impl Function for Fetch {
    type Config = FetchConfig;

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &FetchConfig) -> Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

/// A plugin whose destination its manifest declares at runtime.
struct Plugin;

impl Function for Plugin {
    type Config = Templated;

    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default().with_runtime_fields(vec![
            FieldSpec::runtime("out", FieldKind::Output, false),
            FieldSpec::runtime("token", FieldKind::Expr(ValueKind::String), false),
        ])
    }

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &Templated) -> Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

fn compiler() -> Compiler {
    let secrets = Secrets::new(OwnedDataValue::from(&secrets())).expect("an object");
    let exprs = ExprEngine::builder()
        .secrets(secrets)
        .build()
        .expect("the engine builds");
    let mut registry = Registry::new(exprs);
    registry.register("host", Host);
    registry.register("fetch", Fetch);
    registry.register("plugin", Plugin);
    registry.compiler(CompileOptions::default())
}

// ---------------------------------------------------------------------------
// Definitions and rows
// ---------------------------------------------------------------------------

/// A workflow `w` holding `tasks`.
fn wf(tasks: Value) -> Value {
    json!({"id": "w", "tasks": tasks})
}

/// A task `id` calling `name` with `input`.
fn task(id: &str, name: &str, input: Value) -> Value {
    json!({"id": id, "function": {"name": name, "input": input}})
}

/// A `map` task `id` writing `logic` to `data.x`.
fn map(id: &str, logic: Value) -> Value {
    task(
        id,
        "map",
        json!({"mappings": [{"path": "data.x", "logic": logic}]}),
    )
}

/// A `validation` task `id` with one rule.
fn validation(id: &str) -> Value {
    task(
        id,
        "validation",
        json!({"rules": [{"logic": {"var": "data.ok"}, "message": "not ok"}]}),
    )
}

/// `step` with `key` set to `value`.
fn with(mut step: Value, key: &str, value: Value) -> Value {
    step[key] = value;
    step
}

/// The definition, which must parse.
fn def(json: &Value) -> WorkflowDef {
    let parsed = parse(json, ParseOptions::default());
    parsed
        .def
        .unwrap_or_else(|| panic!("the definition parses: {:?}", parsed.issues))
}

/// One issue: code, severity and absolute path.
type Row = (String, String, String);

fn row(code: C, severity: Severity, path: &str) -> Row {
    (code.as_str().into(), severity.as_str().into(), path.into())
}

fn sorted(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort();
    rows
}

/// The issues for `json`, of the given codes, in order.
fn issues_of(json: &Value, codes: &[C]) -> Vec<Issue> {
    compiler()
        .check(&def(json))
        .into_iter()
        .filter(|i| codes.contains(&i.code))
        .collect()
}

fn rows_of(json: &Value, codes: &[C]) -> Vec<Row> {
    issues_of(json, codes)
        .iter()
        .map(|i| row(i.code, i.severity, &i.path.to_string()))
        .collect()
}

/// The four codes the expression checks report.
const EXPR_CODES: &[C] = &[
    C::SecretInMessageWrite,
    C::UnknownSecret,
    C::EscapedTemplateKey,
    C::DuplicateTemplateKey,
];

/// `json`'s rows of `codes` are `expected`, in any order.
fn rows_are(json: &Value, codes: &[C], expected: &[Row]) {
    assert_eq!(
        sorted(rows_of(json, codes)),
        sorted(expected.to_vec()),
        "the rows for {json}"
    );
}

// ---------------------------------------------------------------------------
// Secrets
// ---------------------------------------------------------------------------

/// Every parameter that holds an expression, with `e` placed in it: the
/// parameter's absolute path, whether the engine records its result, and the
/// workflow.
fn sinks(e: &Value) -> Vec<(&'static str, bool, Value)> {
    let one = |name: &str, input: Value| wf(json!([task("t", name, input)]));
    vec![
        // The `Message` sinks: the engine records each one's result.
        (
            "tasks[0].function.input.mappings[0].logic",
            true,
            one("map", json!({"mappings": [{"path": "data.x", "logic": e}]})),
        ),
        (
            "tasks[0].function.input.mappings[0].path",
            true,
            one("map", json!({"mappings": [{"path": e, "logic": 1}]})),
        ),
        (
            "tasks[0].function.input.rules[0].message",
            true,
            one(
                "validation",
                json!({"rules": [{"logic": true, "message": e}]}),
            ),
        ),
        (
            "tasks[0].function.input.message",
            true,
            one("log", json!({"message": e})),
        ),
        (
            "tasks[0].function.input.fields.f",
            true,
            one("log", json!({"message": "m", "fields": {"f": e}})),
        ),
        (
            "tasks[0].function.input.source",
            true,
            one("parse_json", json!({"source": e, "target": "in"})),
        ),
        (
            "tasks[0].function.input.target",
            true,
            one("parse_xml", json!({"source": "payload", "target": e})),
        ),
        (
            "tasks[0].function.input.source",
            true,
            one("publish_json", json!({"source": e, "target": "out"})),
        ),
        (
            "tasks[0].function.input.target",
            true,
            one("publish_json", json!({"source": "in", "target": e})),
        ),
        (
            "tasks[0].function.input.root_element",
            true,
            one(
                "publish_xml",
                json!({"source": "in", "target": "out", "root_element": e}),
            ),
        ),
        (
            "loop.over",
            true,
            json!({"id": "w", "loop": {"max": 2, "over": e, "as": "item"},
                     "tasks": [map("t", json!(1))]}),
        ),
        (
            "tasks[0].for_each.over",
            true,
            wf(json!([{"id": "t", "for_each": {"over": e, "as": "item"},
                       "function": {"name": "host", "input": {}}}])),
        ),
        // The `Bool` sinks: nothing of the value survives.
        (
            "condition",
            false,
            json!({"id": "w", "condition": e, "tasks": [map("t", json!(1))]}),
        ),
        (
            "tasks[0].condition",
            false,
            wf(json!([{"id": "g", "condition": e, "tasks": [map("t", json!(1))]}])),
        ),
        (
            "tasks[0].condition",
            false,
            wf(json!([with(map("t", json!(1)), "condition", e.clone())])),
        ),
        (
            "tasks[0].function.input.rules[0].logic",
            false,
            one(
                "validation",
                json!({"rules": [{"logic": e, "message": "m"}]}),
            ),
        ),
        (
            "tasks[0].function.input.condition",
            false,
            one("filter", json!({"condition": e})),
        ),
    ]
}

#[test]
fn a_secret_in_every_parameter_is_reported() {
    let mut checked = 0;
    for name in ["k", "nope"] {
        let e = json!({"cat": ["data.", {"secret": name}]});
        for (path, records, json) in sinks(&e) {
            let expected = if records {
                vec![row(C::SecretInMessageWrite, Severity::Rejected, path)]
            } else if name == "nope" {
                vec![row(C::UnknownSecret, Severity::Rejected, path)]
            } else {
                vec![]
            };
            rows_are(&json, EXPR_CODES, &expected);
            checked += 1;
        }
    }
    assert_eq!(checked, 34);
}

#[test]
fn a_recorded_expression_reports_one_secret_issue_and_no_unknown_name() {
    let e = json!({"cat": [{"secret": "nope"}, {"secret": "k"}, {"secret": {"var": "data.n"}}]});
    let json = wf(json!([map("t", e)]));
    rows_are(
        &json,
        EXPR_CODES,
        &[row(
            C::SecretInMessageWrite,
            Severity::Rejected,
            "tasks[0].function.input.mappings[0].logic",
        )],
    );
}

#[test]
fn every_undeclared_literal_name_is_reported_and_a_dynamic_one_is_not() {
    let e = json!({"and": [{"secret": "nope"}, {"secret": ["gone"]}, {"secret": "k"},
                           {"secret": {"var": "data.n"}}]});
    let json = wf(json!([with(map("t", json!(1)), "condition", e)]));
    let at = "tasks[0].condition";
    rows_are(
        &json,
        EXPR_CODES,
        &[
            row(C::UnknownSecret, Severity::Rejected, at),
            row(C::UnknownSecret, Severity::Rejected, at),
        ],
    );
}

#[test]
fn a_secret_issue_is_stamped_with_its_step_and_field() {
    let json = wf(json!([
        {"id": "g", "condition": {"secret": "nope"}, "tasks": [
            task("t", "log", json!({"message": {"secret": "k"}}))
        ]}
    ]));
    let issues = issues_of(&json, EXPR_CODES);
    let stamps: Vec<_> = issues
        .iter()
        .map(|i| {
            (
                i.code,
                i.step_kind,
                i.step_id.as_deref(),
                i.field.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        stamps,
        [
            (C::UnknownSecret, Some(StepKind::Group), Some("g"), None),
            (
                C::SecretInMessageWrite,
                Some(StepKind::Task),
                Some("t"),
                Some("message")
            ),
        ]
    );
}

// ---------------------------------------------------------------------------
// Template keys
// ---------------------------------------------------------------------------

#[test]
fn an_escaped_key_is_reported_at_its_deep_path() {
    let logic = "tasks[0].function.input.mappings[0].logic";
    rows_are(
        &wf(json!([map("t", json!({"$cat": ["a", "b"]}))])),
        EXPR_CODES,
        &[row(
            C::EscapedTemplateKey,
            Severity::Advisory,
            &format!("{logic}.$cat"),
        )],
    );
    // Inside an array, below another key.
    rows_are(
        &wf(json!([map(
            "t",
            json!({"cat": ["a", {"$oid": {"var": "data.id"}}]})
        )])),
        EXPR_CODES,
        &[row(
            C::EscapedTemplateKey,
            Severity::Advisory,
            &format!("{logic}.cat[1].$oid"),
        )],
    );
}

#[test]
fn two_keys_emitting_one_name_are_a_duplicate_at_the_later_key() {
    let logic = "tasks[0].function.input.mappings[0].logic";
    rows_are(
        &wf(json!([map("t", json!({"$a": 1, "a": 2}))])),
        EXPR_CODES,
        &[
            row(
                C::EscapedTemplateKey,
                Severity::Advisory,
                &format!("{logic}.$a"),
            ),
            row(
                C::DuplicateTemplateKey,
                Severity::Rejected,
                &format!("{logic}.a"),
            ),
        ],
    );
    // In an array element, nested below an escaped key.
    rows_are(
        &wf(json!([map(
            "t",
            json!({"cat": [{"$k": {"$b": 1, "b": 2}}]})
        )])),
        EXPR_CODES,
        &[
            row(
                C::EscapedTemplateKey,
                Severity::Advisory,
                &format!("{logic}.cat[0].$k"),
            ),
            row(
                C::EscapedTemplateKey,
                Severity::Advisory,
                &format!("{logic}.cat[0].$k.$b"),
            ),
            row(
                C::DuplicateTemplateKey,
                Severity::Rejected,
                &format!("{logic}.cat[0].$k.b"),
            ),
        ],
    );
}

#[test]
fn a_doubled_escape_is_not_reported_as_escaped_break_14() {
    let logic = "tasks[0].function.input.mappings[0].logic";
    // `$$x` emits `$x`, a key that still starts with `$`: not escaped.
    rows_are(&wf(json!([map("t", json!({"$$x": 1}))])), EXPR_CODES, &[]);
    // `$$a` emits `$a` and `$a` emits `a`: no two keys emit one name.
    rows_are(
        &wf(json!([map("t", json!({"$$a": 1, "$a": 2}))])),
        EXPR_CODES,
        &[row(
            C::EscapedTemplateKey,
            Severity::Advisory,
            &format!("{logic}.$a"),
        )],
    );
    // `$$a` and `$a` both beside `a`: `$a` collides with `a`.
    rows_are(
        &wf(json!([map("t", json!({"$$a": 0, "$a": 1, "a": 2}))])),
        EXPR_CODES,
        &[
            row(
                C::EscapedTemplateKey,
                Severity::Advisory,
                &format!("{logic}.$a"),
            ),
            row(
                C::DuplicateTemplateKey,
                Severity::Rejected,
                &format!("{logic}.a"),
            ),
        ],
    );
}

#[test]
fn an_escaped_key_inside_a_condition_is_reported() {
    let cond = json!({"==": [{"var": "data.k"}, {"$a": 1}]});
    rows_are(
        &wf(json!([with(map("t", json!(1)), "condition", cond.clone())])),
        EXPR_CODES,
        &[row(
            C::EscapedTemplateKey,
            Severity::Advisory,
            "tasks[0].condition.==[1].$a",
        )],
    );
    // The workflow's own condition, a group's, and `loop.over`.
    rows_are(
        &json!({"id": "w", "condition": cond.clone(), "tasks": [map("t", json!(1))]}),
        EXPR_CODES,
        &[row(
            C::EscapedTemplateKey,
            Severity::Advisory,
            "condition.==[1].$a",
        )],
    );
    rows_are(
        &wf(json!([{"id": "g", "condition": cond, "tasks": [map("t", json!(1))]}])),
        EXPR_CODES,
        &[row(
            C::EscapedTemplateKey,
            Severity::Advisory,
            "tasks[0].condition.==[1].$a",
        )],
    );
    rows_are(
        &json!({"id": "w", "loop": {"max": 2, "over": [{"$a": 1, "a": 2}]},
                  "tasks": [map("t", json!(1))]}),
        EXPR_CODES,
        &[
            row(C::EscapedTemplateKey, Severity::Advisory, "loop.over[0].$a"),
            row(
                C::DuplicateTemplateKey,
                Severity::Rejected,
                "loop.over[0].a",
            ),
        ],
    );
}

#[test]
fn a_duplicate_key_rejects_the_workflow() {
    let json = wf(json!([map("t", json!({"$a": 1, "a": 2}))]));
    assert!(compiler().compile_one(&def(&json)).is_err());
    let json = wf(json!([map("t", json!({"$a": 1}))]));
    let compiled = compiler()
        .compile_one(&def(&json))
        .expect("an escaped key is an Advisory");
    let codes: Vec<C> = compiled.issues.iter().map(|i| i.code).collect();
    assert_eq!(codes, [C::EscapedTemplateKey]);
}

// ---------------------------------------------------------------------------
// UNGUARDED_VALIDATION
// ---------------------------------------------------------------------------

const UNGUARDED: &[C] = &[C::UnguardedValidation];

fn unguarded(path: &str) -> Row {
    row(C::UnguardedValidation, Severity::Advisory, path)
}

#[test]
fn an_unguarded_validation_is_reported_at_on_rejection() {
    let next = || map("m", json!(1));
    // Followed by an unguarded task.
    rows_are(
        &wf(json!([validation("v"), next()])),
        UNGUARDED,
        &[unguarded("tasks[0].on_rejection")],
    );
    // A condition of literal `true` guards nothing.
    rows_are(
        &wf(json!([
            validation("v"),
            with(next(), "condition", json!(true))
        ])),
        UNGUARDED,
        &[unguarded("tasks[0].on_rejection")],
    );
    // Nor does a group whose condition is literal `true`, or that has none.
    rows_are(
        &wf(json!([validation("v"), {"id": "g", "condition": true, "tasks": [next()]}])),
        UNGUARDED,
        &[unguarded("tasks[0].on_rejection")],
    );
    rows_are(
        &wf(json!([validation("v"), {"id": "a", "tasks": [{"id": "b", "tasks": [next()]}]}])),
        UNGUARDED,
        &[unguarded("tasks[0].on_rejection")],
    );
    // Each of two validations is unguarded: a validation guards nothing.
    rows_are(
        &wf(json!([validation("v1"), validation("v2"), next()])),
        UNGUARDED,
        &[
            unguarded("tasks[0].on_rejection"),
            unguarded("tasks[1].on_rejection"),
        ],
    );
    // In a group, followed by a task outside it.
    rows_are(
        &wf(json!([{"id": "g", "tasks": [validation("v")]}, next()])),
        UNGUARDED,
        &[unguarded("tasks[0].tasks[0].on_rejection")],
    );
}

#[test]
fn a_guarded_or_stopping_validation_is_not_reported() {
    let next = || map("m", json!(1));
    // The last task: nothing follows it.
    rows_are(&wf(json!([next(), validation("v")])), UNGUARDED, &[]);
    // A conditional task follows it, at once or later.
    let cond = json!({"var": "data.ok"});
    rows_are(
        &wf(json!([
            validation("v"),
            with(next(), "condition", cond.clone())
        ])),
        UNGUARDED,
        &[],
    );
    rows_are(
        &wf(json!([
            validation("v"),
            map("a", json!(1)),
            with(next(), "condition", cond.clone())
        ])),
        UNGUARDED,
        &[],
    );
    // A filter follows it.
    rows_are(
        &wf(json!([
            validation("v"),
            task("f", "filter", json!({"condition": {"var": "data.ok"}})),
            next()
        ])),
        UNGUARDED,
        &[],
    );
    // A group with a condition opens at the next task.
    rows_are(
        &wf(json!([validation("v"), {"id": "g", "condition": cond.clone(), "tasks": [next()]}])),
        UNGUARDED,
        &[],
    );
    // A nested group opens at the next task: the inner one is conditional.
    rows_are(
        &wf(json!([validation("v"),
                   {"id": "a", "tasks": [{"id": "b", "condition": cond, "tasks": [next()]}]}])),
        UNGUARDED,
        &[],
    );
    // It stops the workflow itself.
    rows_are(
        &wf(json!([
            with(validation("v"), "terminal", json!(true)),
            next()
        ])),
        UNGUARDED,
        &[],
    );
    rows_are(
        &wf(json!([
            with(validation("v"), "on_rejection", json!("halt")),
            next()
        ])),
        UNGUARDED,
        &[],
    );
    // And in 3.x's spelling, which the compat reader rewrites.
    rows_are(
        &wf(json!([
            with(validation("v"), "halt_on", json!("failure")),
            next()
        ])),
        UNGUARDED,
        &[],
    );
}

#[test]
fn a_validation_in_setup_is_followed_by_the_body() {
    let json = json!({"id": "w",
        "loop": {"max": 2, "setup": [validation("v")]},
        "tasks": [map("m", json!(1))]});
    rows_are(&json, UNGUARDED, &[unguarded("loop.setup[0].on_rejection")]);
    // A guarded body guards it.
    let json = json!({"id": "w",
        "loop": {"max": 2, "setup": [validation("v")]},
        "tasks": [with(map("m", json!(1)), "condition", json!({"var": "data.ok"}))]});
    rows_are(&json, UNGUARDED, &[]);
}

#[test]
fn the_unguarded_issue_names_the_next_task_and_on_rejection_halt() {
    let json = json!({"id": "w",
        "loop": {"max": 2, "setup": [validation("v")]},
        "tasks": [map("m", json!(1))]});
    let issues = issues_of(&json, UNGUARDED);
    let [issue] = &issues[..] else {
        panic!("one issue: {issues:?}");
    };
    assert_eq!(issue.step_kind, Some(StepKind::Setup));
    assert_eq!(issue.step_id.as_deref(), Some("v"));
    assert_eq!(issue.field, None);
    assert!(issue.message.contains("`m`"), "{}", issue.message);
    assert!(
        issue.message.contains(r#""on_rejection": "halt""#),
        "{}",
        issue.message
    );
    // An Advisory: the workflow compiles.
    assert!(compiler().compile_one(&def(&json)).is_ok());
}

#[test]
fn a_fragment_has_no_workflow_lint() {
    // The expression checks run, so the escaped key is reported; the
    // unguarded validation is not.
    let steps = json!([validation("v"), map("m", json!({"$a": 1}))]);
    let issues = compiler().check_steps(&steps, "tasks");
    let rows: Vec<Row> = issues
        .iter()
        .map(|i| row(i.code, i.severity, &i.path.to_string()))
        .collect();
    assert_eq!(
        rows,
        [row(
            C::EscapedTemplateKey,
            Severity::Advisory,
            "tasks[1].function.input.mappings[0].logic.$a"
        )]
    );
}

// ---------------------------------------------------------------------------
// PAYLOAD_VAR
// ---------------------------------------------------------------------------

/// The `payload` reads, each inside a `cat` at index 0.
fn payload_reads() -> [Value; 5] {
    [
        json!({"var": "payload"}),
        json!({"var": "payload.a"}),
        json!({"var": ["payload.a", 1]}),
        json!({"val": "payload"}),
        json!({"val": ["payload", "a"]}),
    ]
}

/// `read` inside a `cat`, in a `map` `logic` and in a task condition: the
/// two definitions, and where the `cat`'s element sits in each.
fn in_logic_and_condition(read: &Value) -> [(Value, &'static str); 2] {
    let e = json!({"cat": [read, "x"]});
    [
        (
            wf(json!([map("t", e.clone())])),
            "tasks[0].function.input.mappings[0].logic.cat[0]",
        ),
        (
            wf(json!([with(map("t", json!(1)), "condition", e)])),
            "tasks[0].condition.cat[0]",
        ),
    ]
}

#[test]
fn a_literal_var_or_val_rooted_at_payload_is_a_defect_at_the_object() {
    for read in payload_reads() {
        for (json, at) in in_logic_and_condition(&read) {
            let issues = issues_of(&json, &[C::PayloadVar]);
            assert_eq!(
                issues
                    .iter()
                    .map(|i| row(i.code, i.severity, &i.path.to_string()))
                    .collect::<Vec<_>>(),
                [row(C::PayloadVar, Severity::Defect, at)],
                "{read}"
            );
            let message = &issues[0].message;
            assert!(message.contains("payload"), "{message}");
            assert!(message.contains("context"), "{message}");
            // A Defect: the workflow still compiles.
            assert!(compiler().compile_one(&def(&json)).is_ok());
        }
    }
}

#[test]
fn the_payload_message_names_the_path_read() {
    let json = wf(json!([map("t", json!({"var": "payload.order.id"}))]));
    let issues = issues_of(&json, &[C::PayloadVar]);
    assert_eq!(issues.len(), 1);
    assert!(
        issues[0].message.contains("payload.order.id"),
        "{}",
        issues[0].message
    );
    assert_eq!(
        issues[0].path.to_string(),
        "tasks[0].function.input.mappings[0].logic"
    );
}

#[test]
fn a_var_not_rooted_at_payload_or_computed_is_not_reported() {
    for read in [
        json!({"var": "data.payload"}),
        json!({"var": {"cat": ["payload", ".a"]}}),
        json!({"var": "payloads"}),
        json!({"val": "payload.a"}),
        json!({"val": ["data", "payload"]}),
        // Escaped, so not an operator: a literal object.
        json!({"$var": "payload"}),
    ] {
        for (json, _) in in_logic_and_condition(&read) {
            assert_eq!(issues_of(&json, &[C::PayloadVar]), [], "{read}");
        }
    }
}

#[test]
fn a_var_read_per_element_by_a_scoping_operator_is_not_reported() {
    // Inside `map`'s body `var` reads the element, which may have a
    // `payload` key; the array it walks is read from the context.
    let e = json!({"map": [{"var": "payload.items"}, {"var": "payload"}]});
    let json = wf(json!([map("t", e)]));
    let paths: Vec<String> = issues_of(&json, &[C::PayloadVar])
        .iter()
        .map(|i| i.path.to_string())
        .collect();
    assert_eq!(paths, ["tasks[0].function.input.mappings[0].logic.map[0]"]);
}

#[test]
fn reduces_initial_value_is_read_from_the_context_and_reported() {
    // `reduce`'s body reads the element and the accumulator; its initial
    // value, the third argument, is evaluated once, in the context.
    let body = json!({"+": [{"var": "accumulator"}, {"var": "payload"}]});
    let e = json!({"reduce": [{"var": "data.a"}, body, {"var": "payload.init"}]});
    let json = wf(json!([map("t", e)]));
    assert_eq!(
        rows_of(&json, &[C::PayloadVar]),
        [row(
            C::PayloadVar,
            Severity::Defect,
            "tasks[0].function.input.mappings[0].logic.reduce[2]"
        )]
    );
}

// ---------------------------------------------------------------------------
// Break 16: a destination is recorded
// ---------------------------------------------------------------------------

#[test]
fn a_secret_in_a_host_output_is_a_message_write_break_16() {
    let computed = json!({"cat": ["data.", {"secret": "k"}]});
    let json = wf(json!([task(
        "t",
        "fetch",
        json!({"output": computed.clone()})
    )]));
    assert_eq!(
        rows_of(&json, EXPR_CODES),
        [row(
            C::SecretInMessageWrite,
            Severity::Rejected,
            "tasks[0].function.input.output"
        )]
    );
    // A runtime `Output` field is visited the same way.
    let json = wf(json!([task("t", "plugin", json!({"out": computed}))]));
    assert_eq!(
        rows_of(&json, EXPR_CODES),
        [row(
            C::SecretInMessageWrite,
            Severity::Rejected,
            "tasks[0].function.input.out"
        )]
    );
}

#[test]
fn a_secret_in_a_host_field_that_is_not_recorded_is_clean() {
    let json = wf(json!([task(
        "t",
        "fetch",
        json!({"token": {"secret": "k"}})
    )]));
    assert_eq!(compiler().check(&def(&json)), []);
    let json = wf(json!([task(
        "t",
        "plugin",
        json!({"token": {"secret": "k"}})
    )]));
    assert_eq!(compiler().check(&def(&json)), []);
    // An undeclared one is unknown.
    let json = wf(json!([task(
        "t",
        "fetch",
        json!({"token": {"secret": "nope"}})
    )]));
    assert_eq!(
        rows_of(&json, EXPR_CODES),
        [row(
            C::UnknownSecret,
            Severity::Rejected,
            "tasks[0].function.input.token"
        )]
    );
}

// ---------------------------------------------------------------------------
// Break 2b: a host's expression fields are checked as a built-in's are
// ---------------------------------------------------------------------------

#[test]
fn a_host_expression_field_is_checked_as_a_builtins_is_break_2b() {
    const CODES: &[C] = &[
        C::EscapedTemplateKey,
        C::DuplicateTemplateKey,
        C::PayloadVar,
    ];
    for (function, field) in [("fetch", "token"), ("plugin", "token")] {
        let at = format!("tasks[0].function.input.{field}");
        // Template keys and `payload` reads, beside the secret checks.
        let json = wf(json!([task(
            "t",
            function,
            json!({field: {"$a": 1, "a": 2}})
        )]));
        assert_eq!(
            rows_of(&json, CODES),
            [
                row(
                    C::EscapedTemplateKey,
                    Severity::Advisory,
                    &format!("{at}.$a")
                ),
                row(
                    C::DuplicateTemplateKey,
                    Severity::Rejected,
                    &format!("{at}.a")
                ),
            ],
            "{function}"
        );
        assert!(compiler().compile_one(&def(&json)).is_err(), "{function}");
        // A literal `payload` read is a Defect (break 9): it still compiles.
        let json = wf(json!([task(
            "t",
            function,
            json!({field: {"var": "payload.id"}})
        )]));
        assert_eq!(
            rows_of(&json, CODES),
            [row(C::PayloadVar, Severity::Defect, &at)],
            "{function}"
        );
        assert!(compiler().compile_one(&def(&json)).is_ok(), "{function}");
    }
}

// ---------------------------------------------------------------------------
// The order
// ---------------------------------------------------------------------------

#[test]
fn one_expression_reports_secrets_then_template_keys_then_payload_vars() {
    // Written in the opposite order, so the rules' order shows.
    let cond = json!({"and": [{"var": "payload.a"}, {"$k": 1}, {"secret": "nope"}]});
    let json = wf(json!([with(map("t", json!(1)), "condition", cond)]));
    let all = compiler().check(&def(&json));
    let rows: Vec<Row> = all
        .iter()
        .map(|i| row(i.code, i.severity, &i.path.to_string()))
        .collect();
    assert_eq!(
        rows,
        [
            row(C::UnknownSecret, Severity::Rejected, "tasks[0].condition"),
            row(
                C::EscapedTemplateKey,
                Severity::Advisory,
                "tasks[0].condition.and[1].$k"
            ),
            row(C::PayloadVar, Severity::Defect, "tasks[0].condition.and[0]"),
        ]
    );
}
