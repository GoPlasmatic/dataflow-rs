extern crate dataflow_rs_v4 as dataflow_rs;

use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::{Evaluated, ExprEngine, escaped_keys, secret_refs};
use dataflow_rs::run::ErrorKind;
use dataflow_rs::{Expr, Path};
use serde_json::{Value, json};
use std::borrow::Cow;

fn root(v: Value) -> OwnedDataValue {
    OwnedDataValue::from(&v)
}

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn expressions_are_send_and_sync() {
    assert_send_sync::<Expr<OwnedDataValue>>();
    assert_send_sync::<Expr<String>>();
    assert_send_sync::<Expr<u64>>();
    assert_send_sync::<Expr<bool>>();
    assert_send_sync::<Expr<Path>>();
}

#[test]
fn a_literal_folds_and_is_lent() {
    let e = ExprEngine::default();
    let x: Expr<String> = e.compile(&json!("hello")).unwrap();
    assert_eq!(x.constant().map(String::as_str), Some("hello"));
    assert_eq!(x.raw(), &json!("hello"));
    let out = e.eval(&x, &root(json!({}))).unwrap();
    assert!(matches!(out.value, Cow::Borrowed("hello")));
    #[cfg(feature = "budget")]
    assert_eq!(out.ops, Some(0));
    #[cfg(not(feature = "budget"))]
    assert_eq!(out.ops, None);

    let o: Expr<OwnedDataValue> = e.compile(&json!([1, 2])).unwrap();
    let out = e.eval(&o, &root(json!({}))).unwrap();
    assert!(matches!(out.value, Cow::Borrowed(_)));
}

#[test]
fn a_var_does_not_fold_and_is_owned() {
    let e = ExprEngine::default();
    let x: Expr<OwnedDataValue> = e.compile(&json!({"var": "data.x"})).unwrap();
    assert!(x.constant().is_none());
    let out = e.eval(&x, &root(json!({"data": {"x": 7}}))).unwrap();
    assert!(matches!(out.value, Cow::Owned(_)));
    assert_eq!(out.value.into_owned(), root(json!(7)));
}

#[test]
fn division_by_zero_compiles_and_fails_at_eval() {
    let e = ExprEngine::default();
    let x: Expr<OwnedDataValue> = e.compile(&json!({"/": [1, 0]})).unwrap();
    assert!(x.constant().is_none());
    let err = e.eval(&x, &root(json!({}))).unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::Eval);
}

#[test]
fn a_datalogic_compile_failure_is_compile() {
    let e = ExprEngine::default();
    // Nesting past datalogic's compile depth limit is the refusal reachable
    // from pure data.
    let mut deep = json!(1);
    for _ in 0..300 {
        deep = json!({"!": [deep]});
    }
    let err = e.compile::<OwnedDataValue>(&deep).unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::Compile);
    assert_eq!(err.code(), "COMPILATION_ERROR");
}

#[test]
fn a_string_expression_renders_other_results_as_compact_json() {
    let e = ExprEngine::default();
    let x: Expr<String> = e.compile(&json!({"var": "data.n"})).unwrap();
    let out = e.eval(&x, &root(json!({"data": {"n": 5}}))).unwrap();
    assert_eq!(out.value, "5");
    let out = e
        .eval(&x, &root(json!({"data": {"n": {"a": [1]}}})))
        .unwrap();
    assert_eq!(out.value, r#"{"a":[1]}"#);
    let out = e.eval(&x, &root(json!({"data": {"n": "s"}}))).unwrap();
    assert_eq!(out.value, "s");
    let k: Expr<String> = e.compile(&json!(5)).unwrap();
    assert_eq!(k.constant().map(String::as_str), Some("5"));
}

#[test]
fn a_u64_expression() {
    let e = ExprEngine::default();
    let k: Expr<u64> = e.compile(&json!(30000)).unwrap();
    assert_eq!(e.eval(&k, &root(json!({}))).unwrap().value, 30000);
    let err = e.compile::<u64>(&json!(-1)).unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::Config);
    assert!(err.message().contains("u64"), "{}", err.message());
    let x: Expr<u64> = e.compile(&json!({"var": "data.n"})).unwrap();
    assert_eq!(
        e.eval(&x, &root(json!({"data": {"n": 3}}))).unwrap().value,
        3
    );
    let err = e.eval(&x, &root(json!({"data": {"n": "x"}}))).unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::Eval);
}

#[test]
fn a_bool_expression() {
    let e = ExprEngine::default();
    let k: Expr<bool> = e.compile(&json!(true)).unwrap();
    assert!(e.eval(&k, &root(json!({}))).unwrap().value);
    let err = e.compile::<bool>(&json!("yes")).unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::Config);
    let x: Expr<bool> = e.compile(&json!({"var": "data.b"})).unwrap();
    let err = e.eval(&x, &root(json!({"data": {"b": 1}}))).unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::Eval);
}

#[test]
fn a_path_expression() {
    let e = ExprEngine::default();
    let k: Expr<Path> = e.compile(&json!("data.a.b")).unwrap();
    assert_eq!(
        k.constant().map(ToString::to_string).as_deref(),
        Some("data.a.b")
    );
    let out = e.eval(&k, &root(json!({}))).unwrap();
    assert!(matches!(out.value, Cow::Borrowed(_)));
    let err = e.compile::<Path>(&json!("rows")).unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::Config);

    let x: Expr<Path> = e
        .compile(&json!({"cat": ["data.", {"var": "data.k"}]}))
        .unwrap();
    let out = e.eval(&x, &root(json!({"data": {"k": "z"}}))).unwrap();
    assert!(matches!(out.value, Cow::Owned(_)));
    assert_eq!(out.value.to_string(), "data.z");
    let out = e.eval(&x, &root(json!({"data": {"k": 1}}))).unwrap();
    assert_eq!(out.value.to_string(), "data.1");
}

#[test]
fn a_dynamic_path_that_is_not_a_path_is_eval() {
    let e = ExprEngine::default();
    let x: Expr<Path> = e.compile(&json!({"var": "data.p"})).unwrap();
    let err = e
        .eval(&x, &root(json!({"data": {"p": "rows"}})))
        .unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::Eval);
}

#[test]
fn ops_is_counted_only_under_budget() {
    let e = ExprEngine::default();
    let x: Expr<OwnedDataValue> = e
        .compile(&json!({"map": [{"var": "data.l"}, {"+": [{"var": ""}, 1]}]}))
        .unwrap();
    let out: Evaluated<_> = e
        .eval(&x, &root(json!({"data": {"l": [1, 2, 3]}})))
        .unwrap();
    #[cfg(feature = "budget")]
    assert!(out.ops.is_some_and(|n| n > 0), "{:?}", out.ops);
    #[cfg(not(feature = "budget"))]
    assert_eq!(out.ops, None);
}

#[cfg(feature = "budget")]
#[test]
fn a_budget_failure_is_budget() {
    let e = ExprEngine::builder().ops_budget(2).build().unwrap();
    let x: Expr<OwnedDataValue> = e
        .compile(&json!({"map": [{"var": "data.l"}, {"+": [{"var": ""}, 1]}]}))
        .unwrap();
    let err = e
        .eval(&x, &root(json!({"data": {"l": [1, 2, 3]}})))
        .unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::Budget);
    assert_eq!(err.code(), "BUDGET_EXCEEDED");
}

#[test]
fn debug_shows_the_raw_json_and_whether_it_folded() {
    let e = ExprEngine::default();
    let k: Expr<String> = e.compile(&json!("hi")).unwrap();
    let d: Expr<String> = e.compile(&json!({"var": "data.x"})).unwrap();
    let (k, d) = (format!("{k:?}"), format!("{d:?}"));
    assert!(k.contains("\"hi\"") && k.contains("folded: true"), "{k}");
    assert!(d.contains("data.x") && d.contains("folded: false"), "{d}");
}

#[test]
fn escaped_keys_walks_in_v3_order() {
    let raw = json!({"a": {"$cat": 1, "$$x": 2, "b": [{"$oid": 3}]}});
    let got: Vec<String> = escaped_keys(&raw).map(|p| p.to_string()).collect();
    assert_eq!(got, ["a.$cat", "a.b[0].$oid"]);
    assert_eq!(
        escaped_keys(&json!({"$a": 1})).next().unwrap().to_string(),
        "$a"
    );
    assert_eq!(escaped_keys(&json!(1)).count(), 0);
}

#[test]
fn secret_refs_finds_literal_and_dynamic_names() {
    let raw = json!({"x": {"secret": "k"}, "y": {"secret": {"var": "data.n"}}});
    let got: Vec<(Option<&str>, String)> =
        secret_refs(&raw).map(|(n, p)| (n, p.to_string())).collect();
    assert_eq!(got, [(Some("k"), "x".to_owned()), (None, "y".to_owned())]);

    let raw = json!([{"secret": ["a"]}, {"secret": {"cat": [{"secret": "inner"}]}}, {"secret": "a", "z": 1}]);
    let got: Vec<(Option<&str>, String)> =
        secret_refs(&raw).map(|(n, p)| (n, p.to_string())).collect();
    assert_eq!(
        got,
        [
            (Some("a"), "[0]".to_owned()),
            (None, "[1]".to_owned()),
            (Some("inner"), "[1].secret.cat[0]".to_owned()),
        ]
    );
}

#[test]
fn a_u64_never_saturates_or_truncates() {
    let e = ExprEngine::default();
    for lit in ["1.5", "-1.5", "1.8446744073709552e19", "1e20"] {
        let raw: Value = serde_json::from_str(lit).unwrap();
        let err = e.compile::<u64>(&raw).unwrap_err();
        assert_eq!(err.kind(), &ErrorKind::Config, "{lit}");
        assert!(
            err.message().contains("not a u64"),
            "{lit}: {}",
            err.message()
        );
    }
    let half: Expr<u64> = e.compile(&json!({"/": [1, {"var": "data.d"}]})).unwrap();
    let err = e.eval(&half, &root(json!({"data": {"d": 2}}))).unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::Eval);
    let x: Expr<u64> = e.compile(&json!({"*": [{"var": "data.n"}, 1]})).unwrap();
    let big: Value = serde_json::from_str(r#"{"data": {"n": 1e20}}"#).unwrap();
    let err = e.eval(&x, &root(big)).unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::Eval);
    assert!(err.message().contains("not a u64"), "{}", err.message());
}

#[test]
fn a_string_expression_renders_null_and_bool() {
    let e = ExprEngine::default();
    let x: Expr<String> = e.compile(&json!({"var": "data.v"})).unwrap();
    let out = e.eval(&x, &root(json!({"data": {"v": null}}))).unwrap();
    assert_eq!(out.value, "null");
    let out = e.eval(&x, &root(json!({"data": {"v": true}}))).unwrap();
    assert_eq!(out.value, "true");
}
