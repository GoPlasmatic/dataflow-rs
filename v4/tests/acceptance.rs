extern crate dataflow_rs_v4 as dataflow_rs;

// The acceptance configs: an Orion Mongo connector and an Orion plugin's
// `Templated`, each reporting every issue at its authored path.

use dataflow_rs::expr::{
    CompileCx, ExprEngine, ExprVisitor, FieldKind, FieldSpec, Interpolated, Output, ValueKind,
};
use dataflow_rs::function::{FunctionConfig, Templated};
use dataflow_rs::program::{AuthoredPath, IssueCode, Severity};
use dataflow_rs::{Expr, Issue};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code, reason = "the variants are read by serde only")]
enum MongoOp {
    InsertOne,
    InsertMany,
    UpdateOne,
    UpdateMany,
    ReplaceOne,
    DeleteOne,
    DeleteMany,
}

#[derive(FunctionConfig)]
#[allow(
    dead_code,
    reason = "the fields are read through the visitor and tests"
)]
struct MongoWrite {
    connector: String,
    database: String,
    collection: String,
    #[config(serde)]
    op: MongoOp,
    #[config(default_json = "{}")]
    filter: Interpolated,
    document: Option<Interpolated>,
    update: Option<Interpolated>,
    documents: Option<Interpolated>,
    array_filters: Option<Interpolated>,
    #[config(default = false)]
    upsert: Expr<bool>,
    #[config(default = true)]
    ordered: Expr<bool>,
    #[config(default = false)]
    all: bool,
    #[output(default = "data")]
    output: Output,
}

fn root() -> AuthoredPath {
    "tasks[0].function.input".parse().unwrap()
}

type Triple = (IssueCode, Severity, String);

fn triples(issues: &[Issue]) -> Vec<Triple> {
    issues
        .iter()
        .map(|i| (i.code, i.severity, i.path.to_string()))
        .collect()
}

fn at(code: IssueCode, severity: Severity, path: &str) -> Triple {
    (code, severity, format!("tasks[0].function.input{path}"))
}

fn compile<T: FunctionConfig>(
    input: &Value,
    fields: Option<&[FieldSpec]>,
) -> (Option<T>, Vec<Issue>) {
    let engine = ExprEngine::default();
    let mut issues = Vec::new();
    let out = {
        let mut cx = CompileCx::new(&engine, &mut issues).at_path(root());
        if let Some(fields) = fields {
            cx = cx.with_runtime_fields(fields);
        }
        T::compile(input, &mut cx)
    };
    (out, issues)
}

#[derive(Default)]
struct Seen {
    interpolated: Vec<String>,
    exprs: Vec<String>,
}

impl ExprVisitor for Seen {
    fn expr(&mut self, at: &AuthoredPath, _raw: &Value, _records: bool) {
        self.exprs.push(at.to_string());
    }
    fn interpolated(&mut self, at: &AuthoredPath, _doc: &Interpolated) {
        self.interpolated.push(at.to_string());
    }
}

// Orion 1.12.0, docs/src/reference/functions/mongo_write.md.
fn mongo_doc() -> Value {
    json!({"connector":"mongo","database":"shop","collection":"meetings","op":"update_one",
        "filter":{"_id":{"$oid":{"var":"data.payload.object.id"}}},
        "update":{"$set":{"payload":{"var":"data.payload"},
            "updated_at":{"$date":{"var":"metadata.timestamp"}},"deleted":false}},
        "upsert":true,"output":"temp_data.write_result"})
}

// Orion tests/integration/mongodb_test.rs:414-422.
fn mongo_update() -> Value {
    json!({"connector":"mw-crud","database":"orion_test","collection":"meetings",
        "op":"update_one","filter":{"_id":{"var":"data.req.id"}},
        "update":{"$set":{"deleted":true},
            "$push":{"audit":{"at":{"$date":"2026-01-16T00:00:00Z"},"who":"orion"}}},
        "output":"data.res"})
}

// Orion tests/integration/mongodb_test.rs:322-334.
fn mongo_insert() -> Value {
    json!({"connector":"mw-crud","database":"orion_test","collection":"meetings",
        "op":"insert_one","document":{"topic":{"var":"data.req.topic"},
            "payload":{"var":"data.req.payload"},
            "starts_at":{"$date":"2026-01-15T09:00:00Z"},"deleted":false},
        "output":"data.res"})
}

#[test]
fn the_three_orion_mongo_inputs_compile_with_no_issue() {
    for input in [mongo_doc(), mongo_update(), mongo_insert()] {
        let (out, issues) = compile::<MongoWrite>(&input, None);
        assert!(out.is_some(), "{input}");
        assert!(issues.is_empty(), "{issues:?}");
    }
}

#[test]
fn the_mongo_doc_input_is_walked_and_folds_its_literals() {
    let (out, _) = compile::<MongoWrite>(&mongo_doc(), None);
    let config = out.unwrap();
    assert_eq!(config.upsert.constant(), Some(&true));
    assert_eq!(config.ordered.constant(), Some(&true));
    assert!(!config.all);
    let mut seen = Seen::default();
    config.visit(&mut seen);
    assert_eq!(seen.interpolated, ["filter", "update"]);
    assert_eq!(seen.exprs, ["upsert", "ordered", "output"]);
}

#[test]
fn the_broken_mongo_input_reports_every_issue_at_its_path() {
    let input = json!({"connector": "c", "colection": "m", "op": "upsert_one",
        "filter": {"kind": {"cat": ["a", "b"]}},
        "update": {"$set": {"token": {"secret": "api"}}},
        "documents": [{"var": "payload.items"}],
        "upsert": "yes", "output": "results.x"});
    let (out, issues) = compile::<MongoWrite>(&input, None);
    assert!(out.is_none());
    use IssueCode::*;
    use Severity::*;
    assert_eq!(
        triples(&issues),
        [
            at(LikelyTypo, Rejected, ".colection"),
            at(InputParse, Rejected, ".database"),
            at(InputParse, Rejected, ".collection"),
            at(InputParse, Rejected, ".op"),
            at(OperatorInDocument, Advisory, ".filter.kind.cat"),
            at(SecretInDocument, Rejected, ".update.$set.token"),
            at(PayloadVar, Defect, ".documents[0]"),
            at(InputParse, Rejected, ".upsert"),
            at(InvalidPath, Rejected, ".output"),
        ]
    );
}

fn fixedwidth() -> Vec<FieldSpec> {
    vec![
        FieldSpec::runtime("record", FieldKind::Expr(ValueKind::String), true),
        FieldSpec::runtime("spec", FieldKind::Literal(ValueKind::Array), true),
        FieldSpec::runtime("output", FieldKind::Output, false).with_default("\"data\""),
    ]
}

fn identity() -> Vec<FieldSpec> {
    vec![
        FieldSpec::runtime("message", FieldKind::Interpolated, true),
        FieldSpec::runtime("output", FieldKind::Output, false).with_default("\"data\""),
    ]
}

#[test]
fn the_two_orion_plugin_inputs_compile_with_no_issue() {
    let parse = json!({"record":{"var":"data.input.record"},"spec":[
        {"name":"account","width":10,"type":"string"},
        {"name":"amount_minor","width":11,"type":"number"},
        {"name":"currency","width":3,"type":"string"},
        {"name":"value_date","width":8,"type":"date"}],"output":"data.statement"});
    let (out, issues) = compile::<Templated>(&parse, Some(&fixedwidth()));
    assert!(out.is_some() && issues.is_empty(), "{issues:?}");
    let mut seen = Seen::default();
    out.unwrap().visit(&mut seen);
    assert_eq!(seen.exprs, ["record", "output"]);

    let echo = json!({"message":{"var":"data.input"},"output":"data.echo"});
    let (out, issues) = compile::<Templated>(&echo, Some(&identity()));
    assert!(out.is_some() && issues.is_empty(), "{issues:?}");
    let mut seen = Seen::default();
    out.unwrap().visit(&mut seen);
    assert_eq!(seen.interpolated, ["message"]);
    assert_eq!(seen.exprs, ["output"]);
}

#[test]
fn an_absent_output_takes_its_default() {
    let (out, issues) = compile::<Templated>(&json!({"message": "hi"}), Some(&identity()));
    assert!(out.is_some() && issues.is_empty(), "{issues:?}");
    let mut seen = Seen::default();
    out.unwrap().visit(&mut seen);
    assert_eq!(seen.exprs, ["output"]);
}

#[test]
fn the_broken_plugin_input_reports_every_issue_at_its_path() {
    let input = json!({"reocrd": {"var": "data.r"}, "spec": "x", "output": 7, "extra": 1});
    let (out, issues) = compile::<Templated>(&input, Some(&fixedwidth()));
    assert!(out.is_none());
    use IssueCode::*;
    use Severity::*;
    // Unknown keys come first, in the input's key order, and a JSON object
    // iterates in sorted key order here: `extra` before `reocrd`. Then the
    // runtime fields, in declaration order.
    assert_eq!(
        triples(&issues),
        [
            at(UnknownKey, Rejected, ".extra"),
            at(LikelyTypo, Rejected, ".reocrd"),
            at(InputParse, Rejected, ".record"),
            at(InputParse, Rejected, ".spec"),
            at(InputParse, Rejected, ".output"),
        ]
    );
}

#[test]
fn no_runtime_fields_is_one_input_parse() {
    let (out, issues) = compile::<Templated>(&json!({"a": 1}), None);
    assert!(out.is_none());
    assert_eq!(
        triples(&issues),
        [at(IssueCode::InputParse, Severity::Rejected, "")]
    );
    assert!(issues[0].message.contains("no runtime fields are declared"));
}

#[test]
fn a_literal_is_checked_against_its_kind_and_a_folded_expr_too() {
    let fields = vec![
        FieldSpec::runtime("path", FieldKind::Literal(ValueKind::Path), true),
        FieldSpec::runtime("any", FieldKind::Literal(ValueKind::Any), false),
        FieldSpec::runtime("n", FieldKind::Expr(ValueKind::Number), false),
    ];
    let (out, issues) = compile::<Templated>(
        &json!({"path": "data.a", "any": null, "n": 3}),
        Some(&fields),
    );
    assert!(out.is_some() && issues.is_empty(), "{issues:?}");
    let (out, issues) = compile::<Templated>(&json!({"path": "rows", "n": "x"}), Some(&fields));
    assert!(out.is_none());
    assert_eq!(
        triples(&issues),
        [
            at(IssueCode::InvalidPath, Severity::Rejected, ".path"),
            at(IssueCode::InputParse, Severity::Rejected, ".n"),
        ]
    );
    let (out, issues) = compile::<Templated>(&json!([1]), Some(&fields));
    assert!(out.is_none());
    assert_eq!(
        triples(&issues),
        [at(IssueCode::InputParse, Severity::Rejected, "")]
    );
}

#[test]
fn a_field_whose_expression_does_not_compile_is_template_compile() {
    // Found by probing datalogic-rs 5.7.1: the only pure-data compile error is
    // nesting past its depth limit of 256, so 300 nested levels is refused.
    let mut rule = json!({"var": "data.x"});
    for _ in 0..300 {
        rule = json!({"!": rule});
    }
    let (out, issues) =
        compile::<Templated>(&json!({"record": rule, "spec": []}), Some(&fixedwidth()));
    assert!(out.is_none());
    assert_eq!(
        triples(&issues),
        [at(
            IssueCode::TemplateCompile,
            Severity::Rejected,
            ".record"
        )]
    );
}
