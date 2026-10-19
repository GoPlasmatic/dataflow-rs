use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::{
    CompileCx, ConfigField, ExprEngine, ExprVisitor, FieldKind, FieldSpec, Output, ValueKind, Visit,
};
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::program::{AuthoredPath, IssueCode, Severity, StepKind};
use dataflow_rs::{Expr, Issue, Path};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn root() -> AuthoredPath {
    "tasks[2].function.input.limits".parse().unwrap()
}

fn compile<T: ConfigField>(raw: &Value, at: AuthoredPath) -> (Option<T>, Vec<Issue>) {
    let engine = ExprEngine::default();
    let mut issues = Vec::new();
    let out = {
        let mut cx = CompileCx::new(&engine, &mut issues).at_path(at);
        T::compile(raw, &mut cx)
    };
    (out, issues)
}

#[test]
fn a_vec_reports_every_bad_element_at_its_index() {
    let (out, issues) =
        compile::<Vec<Expr<u64>>>(&json!([1, "x", {"var": "data.n"}, true]), root());
    assert!(out.is_none());
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].code, IssueCode::InputParse);
    assert_eq!(
        issues[0].path.to_string(),
        "tasks[2].function.input.limits[1]"
    );
    assert_eq!(
        issues[1].path.to_string(),
        "tasks[2].function.input.limits[3]"
    );
}

#[test]
fn a_vec_of_good_elements_compiles() {
    let (out, issues) = compile::<Vec<Expr<u64>>>(&json!([1, {"var": "data.n"}]), root());
    assert!(issues.is_empty());
    assert_eq!(out.unwrap().len(), 2);
}

#[test]
fn a_non_array_is_input_parse() {
    let (out, issues) = compile::<Vec<bool>>(&json!({}), root());
    assert!(out.is_none());
    assert_eq!(issues[0].code, IssueCode::InputParse);
    assert!(issues[0].message.contains("expected an array"));
}

#[test]
fn a_map_reports_at_the_key() {
    let (out, issues) = compile::<BTreeMap<String, bool>>(&json!({"a": true, "b": 1}), root());
    assert!(out.is_none());
    assert_eq!(issues.len(), 1);
    assert_eq!(
        issues[0].path.to_string(),
        "tasks[2].function.input.limits.b"
    );
    assert!(issues[0].message.contains("expected a boolean"));
    let (_, issues) = compile::<BTreeMap<String, bool>>(&json!([]), root());
    assert!(issues[0].message.contains("expected an object"));
}

#[test]
fn scalars_say_what_they_expected() {
    let (_, i) = compile::<String>(&json!(1), root());
    assert!(i[0].message.contains("expected a string"));
    let (_, i) = compile::<u8>(&json!(300), root());
    assert!(
        i[0].message
            .contains("expected an unsigned integer that fits u8")
    );
    let (v, i) = compile::<u8>(&json!(200), root());
    assert!(i.is_empty());
    assert_eq!(v, Some(200));
    let (v, _) = compile::<f64>(&json!(1.5), root());
    assert_eq!(v, Some(1.5));
    let (v, _) = compile::<serde_json::Value>(&json!({"a": 1}), root());
    assert_eq!(v, Some(json!({"a": 1})));
    let (v, _) = compile::<OwnedDataValue>(&json!([1]), root());
    assert_eq!(v, Some(OwnedDataValue::from(&json!([1]))));
}

#[test]
fn option_is_absent_ok_and_compiles_t_when_present() {
    assert!(matches!(<Option<bool>>::absent(), Some(None)));
    assert!(<bool>::absent().is_none());
    let (v, i) = compile::<Option<bool>>(&json!(true), root());
    assert!(i.is_empty());
    assert_eq!(v, Some(Some(true)));
    let (v, i) = compile::<Option<bool>>(&json!("yes"), root());
    assert!(v.is_none());
    assert_eq!(i[0].code, IssueCode::InputParse);
}

#[test]
fn an_explicit_null_is_none_unless_t_takes_null_as_a_value() {
    // `T` has no null: an explicit `null` says "none", with no issue.
    let (v, i) = compile::<Option<bool>>(&Value::Null, root());
    assert!(i.is_empty(), "{i:?}");
    assert_eq!(v, Some(None));
    let (v, i) = compile::<Option<String>>(&Value::Null, root());
    assert!(i.is_empty(), "{i:?}");
    assert_eq!(v, Some(None));
    let (v, i) = compile::<Option<u64>>(&Value::Null, root());
    assert!(i.is_empty(), "{i:?}");
    assert_eq!(v, Some(None));
    let (v, i) = compile::<Option<Vec<bool>>>(&Value::Null, root());
    assert!(i.is_empty(), "{i:?}");
    assert_eq!(v, Some(None));

    // `T` takes `null` as a value: the `null` is that value.
    let (v, i) = compile::<Option<Expr>>(&Value::Null, root());
    assert!(i.is_empty(), "{i:?}");
    let expr = v.unwrap().expect("a constant null expression");
    assert_eq!(expr.constant(), Some(&OwnedDataValue::Null));
    let (v, _) = compile::<Option<OwnedDataValue>>(&Value::Null, root());
    assert_eq!(v, Some(Some(OwnedDataValue::Null)));
    let (v, _) = compile::<Option<Value>>(&Value::Null, root());
    assert_eq!(v, Some(Some(Value::Null)));
    let (v, _) = compile::<Option<dataflow_rs::expr::Interpolated>>(&Value::Null, root());
    assert!(matches!(v, Some(Some(_))));
    const { assert!(<Expr>::NULL_IS_VALUE && !<String>::NULL_IS_VALUE) };
}

#[test]
fn an_expr_classifies_its_errors() {
    let (_, i) = compile::<Expr<Path>>(&json!("extra.x"), root());
    assert_eq!(i[0].code, IssueCode::InvalidPath);
    let (v, i) = compile::<Expr<Path>>(&json!("data.x"), root());
    assert!(i.is_empty() && v.is_some());
    let (_, i) = compile::<Expr<bool>>(&json!("yes"), root());
    assert_eq!(i[0].code, IssueCode::InputParse);
    let (_, i) = compile::<Expr<bool>>(&crate::support::too_deep(), root());
    assert_eq!(i[0].code, IssueCode::TemplateCompile);
    assert!(!i[0].message.is_empty());
}

#[test]
fn issue_stamps_field_step_and_severity() {
    let engine = ExprEngine::default();
    let mut issues = Vec::new();
    let mut cx = CompileCx::new(&engine, &mut issues)
        .at_path("tasks[0].function.input".parse().unwrap())
        .for_step(StepKind::Task, "t");
    assert_eq!(cx.rejected(), 0);
    cx.at("limits", |cx| {
        cx.at(0usize, |cx| cx.issue(IssueCode::InputParse, "bad"));
        cx.issue(IssueCode::UnknownKey, "advisory");
    });
    cx.issue(IssueCode::InputParse, "at root");
    cx.issue_as(IssueCode::UnknownKey, Severity::Rejected, "forced");
    assert_eq!(cx.rejected(), 3);
    assert_eq!(cx.path().to_string(), "tasks[0].function.input");
    drop(cx);
    assert_eq!(issues[0].field.as_deref(), Some("limits"));
    assert_eq!(
        issues[0].path.to_string(),
        "tasks[0].function.input.limits[0]"
    );
    assert_eq!(issues[0].step_kind, Some(StepKind::Task));
    assert_eq!(issues[0].step_id.as_deref(), Some("t"));
    assert_eq!(issues[1].severity, Severity::Advisory);
    assert_eq!(issues[2].field, None);
    assert_eq!(issues[3].severity, Severity::Rejected);
}

#[test]
fn rejected_counts_only_rejected() {
    let engine = ExprEngine::default();
    let mut issues = Vec::new();
    let mut cx = CompileCx::new(&engine, &mut issues);
    cx.issue(IssueCode::UnknownKey, "advisory");
    cx.issue(IssueCode::PayloadVar, "defect");
    assert_eq!(cx.rejected(), 0);
    cx.issue(IssueCode::InvalidPath, "rejected");
    assert_eq!(cx.rejected(), 1);
}

#[test]
fn at_pops_even_with_an_early_return() {
    let engine = ExprEngine::default();
    let mut issues = Vec::new();
    let mut cx = CompileCx::new(&engine, &mut issues);
    let r: Option<()> = cx.at("a", |_| None);
    assert!(r.is_none());
    assert!(cx.path().is_root());
}

#[test]
fn unknown_keys_are_typo_or_unknown() {
    let engine = ExprEngine::default();
    let mut issues = Vec::new();
    let mut cx =
        CompileCx::new(&engine, &mut issues).at_path("tasks[0].function.input".parse().unwrap());
    let known = ["connector", "database", "collection"];
    cx.at("colection", |cx| cx.unknown_key("colection", &known));
    cx.at("zzz", |cx| cx.unknown_key("zzz", &known));
    drop(cx);
    assert_eq!(issues[0].field.as_deref(), Some("colection"));
    assert_eq!(issues[1].field.as_deref(), Some("zzz"));
    assert_eq!(issues[0].code, IssueCode::LikelyTypo);
    assert_eq!(issues[0].severity, Severity::Rejected);
    assert!(issues[0].message.contains("collection"));
    assert_eq!(
        issues[0].path.to_string(),
        "tasks[0].function.input.colection"
    );
    assert_eq!(issues[1].code, IssueCode::UnknownKey);
    assert_eq!(issues[1].severity, Severity::Rejected);
}

#[derive(Default)]
struct Collect(Vec<(String, Value, bool)>);

impl ExprVisitor for Collect {
    fn expr(&mut self, at: &AuthoredPath, raw: &Value, records: bool) {
        self.0.push((at.to_string(), raw.clone(), records));
    }
}

#[test]
fn visit_reports_each_expr_at_its_path() {
    let (v, _) = compile::<Vec<Expr<u64>>>(&json!([1, {"var": "data.n"}]), AuthoredPath::root());
    let v = v.unwrap();
    let mut c = Collect::default();
    {
        let mut visit = Visit::new(&mut c, "limits".parse().unwrap());
        v.visit(&mut visit);
        visit.recording(true, |vi| v[0].visit(vi));
        visit.recording(false, |vi| v[1].visit(vi));
    }
    assert_eq!(c.0[0], ("limits[0]".into(), json!(1), false));
    assert_eq!(
        c.0[1],
        ("limits[1]".into(), json!({"var": "data.n"}), false)
    );
    // recording ORs the flag in and restores it
    assert!(c.0[2].2);
    assert!(!c.0[3].2);
}

#[test]
fn visit_walks_maps_and_options() {
    let (m, _) = compile::<BTreeMap<String, Option<Expr<bool>>>>(
        &json!({"a": true, "b": {"var": "data.x"}}),
        AuthoredPath::root(),
    );
    let mut c = Collect::default();
    m.unwrap()
        .visit(&mut Visit::new(&mut c, AuthoredPath::root()));
    let paths: Vec<_> = c.0.iter().map(|e| e.0.as_str()).collect();
    assert_eq!(paths, ["a", "b"]);
}

#[test]
fn kinds_follow_the_type() {
    assert!(matches!(
        <bool>::kind(),
        FieldKind::Literal(ValueKind::Bool)
    ));
    assert!(matches!(
        <String>::kind(),
        FieldKind::Literal(ValueKind::String)
    ));
    assert!(matches!(
        <u16>::kind(),
        FieldKind::Literal(ValueKind::Number)
    ));
    assert!(matches!(
        <f64>::kind(),
        FieldKind::Literal(ValueKind::Number)
    ));
    assert!(matches!(
        <OwnedDataValue>::kind(),
        FieldKind::Literal(ValueKind::Any)
    ));
    assert!(matches!(
        <Expr<Path>>::kind(),
        FieldKind::Expr(ValueKind::Path)
    ));
    assert!(matches!(
        <Expr<String>>::kind(),
        FieldKind::Expr(ValueKind::String)
    ));
    assert!(matches!(
        <Expr<u64>>::kind(),
        FieldKind::Expr(ValueKind::Number)
    ));
    assert!(matches!(
        <Expr<bool>>::kind(),
        FieldKind::Expr(ValueKind::Bool)
    ));
    assert!(matches!(<Expr>::kind(), FieldKind::Expr(ValueKind::Any)));
    assert!(matches!(
        <Option<bool>>::kind(),
        FieldKind::Literal(ValueKind::Bool)
    ));
    match <Vec<bool>>::kind() {
        FieldKind::List(f) => assert!(matches!(f(), FieldKind::Literal(ValueKind::Bool))),
        _ => panic!("a list"),
    }
    match <BTreeMap<String, Expr<u64>>>::kind() {
        FieldKind::Map(f) => assert!(matches!(f(), FieldKind::Expr(ValueKind::Number))),
        _ => panic!("a map"),
    }
}

#[test]
fn a_field_spec_builds() {
    let s = FieldSpec::runtime("limit", <u64>::kind(), false).with_default("5");
    assert_eq!(s.name, "limit");
    assert!(!s.required && !s.records && !s.primary);
    assert_eq!(s.default.as_deref(), Some("5"));
    // An output records, at any depth.
    assert!(FieldSpec::runtime("out", FieldKind::Output, false).records);
    assert!(FieldSpec::runtime("outs", <Vec<Output>>::kind(), false).records);
}

#[test]
fn unit_config_accepts_empty_and_null() {
    for raw in [json!({}), Value::Null] {
        let (out, issues) = compile_cfg::<()>(&raw);
        assert!(out.is_some() && issues.is_empty());
    }
}

fn compile_cfg<C: FunctionConfig>(raw: &Value) -> (Option<C>, Vec<Issue>) {
    let engine = ExprEngine::default();
    let mut issues = Vec::new();
    let out = {
        let mut cx = CompileCx::new(&engine, &mut issues)
            .at_path("tasks[0].function.input".parse().unwrap());
        C::compile(raw, &mut cx)
    };
    (out, issues)
}

#[test]
fn unit_config_refuses_keys_and_non_objects() {
    let (out, issues) = compile_cfg::<()>(&json!({"a": 1}));
    assert!(out.is_none());
    assert_eq!(issues[0].code, IssueCode::UnknownKey);
    assert_eq!(issues[0].severity, Severity::Rejected);
    assert_eq!(issues[0].path.to_string(), "tasks[0].function.input.a");
    let (out, issues) = compile_cfg::<()>(&json!(3));
    assert!(out.is_none());
    assert_eq!(issues[0].code, IssueCode::InputParse);
    assert!(<() as FunctionConfig>::fields().is_empty());
    <() as FunctionConfig>::visit(&(), &mut Collect::default());
}

#[test]
fn an_index_below_the_root_carries_no_field() {
    let engine = ExprEngine::default();
    let mut issues = Vec::new();
    let mut cx =
        CompileCx::new(&engine, &mut issues).at_path("tasks[0].function.input".parse().unwrap());
    cx.at(0usize, |cx| cx.issue(IssueCode::InputParse, "x"));
    drop(cx);
    assert_eq!(issues[0].field, None);
}

#[test]
fn a_path_expr_over_a_non_string_is_input_parse_and_over_a_bad_string_invalid_path() {
    let (out, issues) = compile::<Expr<Path>>(&json!(7), AuthoredPath::root());
    assert!(out.is_none());
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code, IssueCode::InputParse);

    let (out, issues) = compile::<Expr<Path>>(&json!("rows"), AuthoredPath::root());
    assert!(out.is_none());
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code, IssueCode::InvalidPath);
}
