extern crate dataflow_rs_v4 as dataflow_rs;

use dataflow_rs::expr::{CompileCx, ConfigField, ExprEngine, ExprVisitor, FieldKind, Visit};
use dataflow_rs::expr::{Interpolated, Output};
use dataflow_rs::program::{AuthoredPath, IssueCode, Severity};
use dataflow_rs::{Issue, Path};
use serde_json::{Value, json};

fn mongo_write() -> Value {
    json!({
        "connector": "mongo", "database": "shop", "collection": "meetings", "op": "update_one",
        "filter": { "_id": { "$oid": { "var": "data.payload.object.id" } } },
        "update": { "$set": {
            "payload": { "var": "data.payload" },
            "updated_at": { "$date": { "var": "metadata.timestamp" } },
            "deleted": false
        } },
        "upsert": true,
        "output": "temp_data.write_result"
    })
}

fn compile<T: ConfigField>(raw: &Value) -> (Option<T>, Vec<Issue>) {
    let engine = ExprEngine::default();
    let mut issues = Vec::new();
    let out = {
        let mut cx = CompileCx::new(&engine, &mut issues);
        T::compile(raw, &mut cx)
    };
    (out, issues)
}

fn holes(doc: &Interpolated) -> Vec<String> {
    doc.holes().map(Path::to_string).collect()
}

#[test]
fn the_mongo_write_document_compiles_with_its_holes_in_order() {
    let engine = ExprEngine::default();
    let doc = engine.interpolated(&mongo_write()).unwrap();
    assert_eq!(
        holes(&doc),
        [
            "data.payload.object.id",
            "data.payload",
            "metadata.timestamp"
        ]
    );
    assert_eq!(doc.raw(), &mongo_write());
    let (out, issues) = compile::<Interpolated>(&mongo_write());
    assert!(out.is_some());
    assert!(issues.is_empty());
}

#[test]
fn a_non_string_var_is_invalid_path_at_its_node() {
    let (out, issues) = compile::<Interpolated>(&json!({"_id": {"var": 5}}));
    assert!(out.is_none());
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code, IssueCode::InvalidPath);
    assert_eq!(issues[0].path.to_string(), "_id");
}

#[test]
fn bad_var_arguments_are_invalid_path() {
    for bad in [
        json!(""),
        json!("results.x"),
        json!([]),
        json!([5]),
        json!(["data.a", 1, 2]),
    ] {
        let (out, issues) = compile::<Interpolated>(&json!({"a": {"var": bad}}));
        assert!(out.is_none(), "{bad}");
        assert_eq!(issues[0].code, IssueCode::InvalidPath, "{bad}");
    }
}

#[test]
fn a_payload_var_is_a_defect_and_the_document_still_compiles() {
    let (out, issues) = compile::<Interpolated>(&json!({"a": {"var": "payload.x"}}));
    assert!(out.is_some());
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code, IssueCode::PayloadVar);
    assert_eq!(issues[0].severity, Severity::Defect);
    assert_eq!(issues[0].path.to_string(), "a");
    assert_eq!(holes(&out.unwrap()), Vec::<String>::new());
}

#[test]
fn an_operator_in_a_document_is_advisory_and_stays_data() {
    let (out, issues) = compile::<Interpolated>(&json!({"k": {"cat": ["a", {"var": "data.x"}]}}));
    let doc = out.unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code, IssueCode::OperatorInDocument);
    assert_eq!(issues[0].severity, Severity::Advisory);
    assert_eq!(issues[0].path.to_string(), "k.cat");
    assert_eq!(holes(&doc), ["data.x"]);
}

#[test]
fn a_val_in_a_document_is_an_operator_that_stays_data() {
    // Only `var` is a hole: `val` reads the context in an expression, but in
    // a document it is data, and said so like any other operator.
    let (out, issues) = compile::<Interpolated>(&json!({"k": {"val": ["data", "x"]}}));
    let doc = out.unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code, IssueCode::OperatorInDocument);
    assert_eq!(issues[0].severity, Severity::Advisory);
    assert_eq!(issues[0].path.to_string(), "k.val");
    assert_eq!(holes(&doc), Vec::<String>::new());
}

#[test]
fn a_holes_default_is_data_and_is_walked() {
    // A secret as a hole's default would be handed on as the literal object.
    let (out, issues) =
        compile::<Interpolated>(&json!({"a": {"var": ["data.x", {"secret": "k"}]}}));
    assert!(out.is_none());
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].code, IssueCode::SecretInDocument);
    assert_eq!(issues[0].path.to_string(), "a.var[1]");

    // An operator-shaped default, `var` included, stays data with the
    // Advisory, and opens no hole.
    let (out, issues) = compile::<Interpolated>(
        &json!({"a": {"var": ["data.x", {"cat": ["p", {"var": "data.y"}]}]}}),
    );
    let doc = out.unwrap();
    let at: Vec<(IssueCode, String)> = issues
        .iter()
        .map(|i| (i.code, i.path.to_string()))
        .collect();
    assert_eq!(
        at,
        [
            (IssueCode::OperatorInDocument, "a.var[1].cat".to_string()),
            (
                IssueCode::OperatorInDocument,
                "a.var[1].cat[1].var".to_string()
            ),
        ]
    );
    assert!(issues.iter().all(|i| i.severity == Severity::Advisory));
    assert_eq!(holes(&doc), ["data.x"]);

    // A default beside a bad path is walked too: every issue is reported.
    let (_, issues) =
        compile::<Interpolated>(&json!({"a": {"var": ["results.x", {"secret": "k"}]}}));
    let codes: Vec<IssueCode> = issues.iter().map(|i| i.code).collect();
    assert_eq!(codes, [IssueCode::InvalidPath, IssueCode::SecretInDocument]);
}

#[test]
fn a_secret_in_a_document_is_rejected() {
    let (out, issues) = compile::<Interpolated>(&json!({"t": {"secret": "api"}}));
    assert!(out.is_none());
    assert_eq!(issues[0].code, IssueCode::SecretInDocument);
    assert_eq!(issues[0].path.to_string(), "t");
    let engine = ExprEngine::default();
    let err = engine
        .interpolated(&json!({"t": {"secret": "api"}}))
        .unwrap_err();
    assert_eq!(err[0].code, IssueCode::SecretInDocument);
}

#[test]
fn interpolated_drops_non_rejecting_issues_on_ok() {
    let engine = ExprEngine::default();
    assert!(
        engine
            .interpolated(&json!({"a": {"var": "payload.x"}}))
            .is_ok()
    );
}

#[test]
fn every_issue_is_reported_in_document_order() {
    let (out, issues) =
        compile::<Interpolated>(&json!({"a": {"var": 1}, "b": [{"secret": "s"}, {"var": ""}]}));
    assert!(out.is_none());
    let at: Vec<String> = issues.iter().map(|i| i.path.to_string()).collect();
    assert_eq!(at, ["a", "b[0]", "b[1]"]);
}

#[test]
fn output_folds_a_literal_path() {
    let (out, issues) = compile::<Output>(&json!("data.res"));
    assert!(issues.is_empty());
    let out = out.unwrap();
    assert_eq!(out.constant().unwrap().to_string(), "data.res");
    assert!(out.path().constant().is_some());
}

#[test]
fn output_refuses_a_bad_root() {
    let (out, issues) = compile::<Output>(&json!("results.x"));
    assert!(out.is_none());
    assert_eq!(issues[0].code, IssueCode::InvalidPath);
}

#[test]
fn a_computed_output_does_not_fold() {
    let (out, issues) = compile::<Output>(&json!({"cat": ["data.", {"var": "data.k"}]}));
    assert!(issues.is_empty());
    assert!(out.unwrap().constant().is_none());
}

#[test]
fn kinds_and_absence() {
    assert!(matches!(Interpolated::kind(), FieldKind::Interpolated));
    assert!(matches!(Output::kind(), FieldKind::Output));
    assert!(Output::absent().is_none());
}

#[derive(Default)]
struct Seen(Vec<String>);
impl ExprVisitor for Seen {
    fn expr(&mut self, at: &AuthoredPath, _raw: &Value, _records: bool) {
        self.0.push(format!("expr {at}"));
    }
    fn interpolated(&mut self, at: &AuthoredPath, doc: &Interpolated) {
        self.0.push(format!("doc {at} {}", doc.holes().count()));
    }
}

#[test]
fn visiting_reports_a_document_and_an_output() {
    let (doc, _) = compile::<Interpolated>(&mongo_write());
    let (out, _) = compile::<Output>(&json!("data.res"));
    let mut seen = Seen::default();
    let mut v = Visit::new(&mut seen, AuthoredPath::root());
    v.at("filter", |v| doc.unwrap().visit(v));
    v.at("output", |v| out.unwrap().visit(v));
    assert_eq!(seen.0, ["doc filter 3", "expr output"]);
}

#[test]
fn interpolated_is_send_and_sync() {
    fn assert<T: Send + Sync + std::fmt::Debug>() {}
    assert::<Interpolated>();
    assert::<Output>();
}
