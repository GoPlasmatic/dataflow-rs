extern crate dataflow_rs_v4 as dataflow_rs;

use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::{
    ConfigField, EvalRoot, Evaluated, ExprEngine, FieldKind, Secrets, ValueKind, escaped_keys,
    secret_refs,
};
use dataflow_rs::function::{FunctionConfig, testing};
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
    assert_send_sync::<Expr<i64>>();
    assert_send_sync::<Expr<f64>>();
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

#[test]
fn an_i64_expression() {
    let e = ExprEngine::default();
    // A negative literal folds, and is the value.
    let k: Expr<i64> = e.compile(&json!(-3)).unwrap();
    assert_eq!(k.constant(), Some(&-3));
    assert_eq!(e.eval(&k, &root(json!({}))).unwrap().value, -3);
    // An expression that reads the context does not fold.
    let x: Expr<i64> = e.compile(&json!({"-": [{"var": "data.n"}, 10]})).unwrap();
    assert!(x.constant().is_none());
    assert_eq!(
        e.eval(&x, &root(json!({"data": {"n": 3}}))).unwrap().value,
        -7
    );
    // A float with a fraction, a number out of range and a string are not
    // an i64: a constant at compile, a computed one at eval.
    for lit in ["2.5", "1e20", r#""3""#] {
        let raw: Value = serde_json::from_str(lit).unwrap();
        let err = e.compile::<i64>(&raw).unwrap_err();
        assert_eq!(err.kind(), &ErrorKind::Config, "{lit}");
        assert!(
            err.message().contains("not an i64"),
            "{lit}: {}",
            err.message()
        );
    }
    let v: Expr<i64> = e.compile(&json!({"var": "data.v"})).unwrap();
    for n in [json!(0.5), json!(1e20), json!("x")] {
        let err = e.eval(&v, &root(json!({"data": {"v": n}}))).unwrap_err();
        assert_eq!(err.kind(), &ErrorKind::Eval, "{n}");
        assert!(err.message().contains("an i64"), "{n}: {}", err.message());
    }
}

#[test]
fn an_f64_expression() {
    let e = ExprEngine::default();
    let k: Expr<f64> = e.compile(&json!(2.5)).unwrap();
    assert_eq!(k.constant(), Some(&2.5));
    assert!((e.eval(&k, &root(json!({}))).unwrap().value - 2.5).abs() < f64::EPSILON);
    // Any number: an integer is one too.
    let k: Expr<f64> = e.compile(&json!(-4)).unwrap();
    assert_eq!(k.constant(), Some(&-4.0));
    let x: Expr<f64> = e.compile(&json!({"*": [{"var": "data.n"}, 0.5]})).unwrap();
    assert!(x.constant().is_none());
    let out = e.eval(&x, &root(json!({"data": {"n": 5}}))).unwrap().value;
    assert!((out - 2.5).abs() < f64::EPSILON, "{out}");
    // A string is not a number, at compile and at eval.
    let err = e.compile::<f64>(&json!("2.5")).unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::Config);
    assert!(err.message().contains("not an f64"), "{}", err.message());
    let y: Expr<f64> = e.compile(&json!({"var": "data.s"})).unwrap();
    let err = e.eval(&y, &root(json!({"data": {"s": "x"}}))).unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::Eval);
    assert!(err.message().contains("an f64"), "{}", err.message());
}

/// A config with an `i64` and an `f64` field, as a host derives one.
#[derive(FunctionConfig)]
struct Incr {
    by: Expr<i64>,
    ratio: Option<Expr<f64>>,
}

#[test]
fn i64_and_f64_fields_are_numbers_in_a_derived_config() {
    assert!(matches!(
        <Expr<i64> as ConfigField>::kind(),
        FieldKind::Expr(ValueKind::Number)
    ));
    assert!(matches!(
        <Expr<f64> as ConfigField>::kind(),
        FieldKind::Expr(ValueKind::Number)
    ));
    let kinds: Vec<(&str, bool)> = Incr::fields()
        .iter()
        .map(|f| {
            (
                &*f.name,
                matches!(f.kind, FieldKind::Expr(ValueKind::Number)),
            )
        })
        .collect();
    assert_eq!(kinds, [("by", true), ("ratio", true)]);

    let Ok(cfg) = testing::config::<Incr>(json!({"by": -2, "ratio": 0.25})) else {
        panic!("the config compiles");
    };
    assert_eq!(cfg.by.constant(), Some(&-2));
    assert_eq!(cfg.ratio.as_ref().and_then(Expr::constant), Some(&0.25));
    // A literal that is not the field's type is refused at compile.
    let Err(issues) = testing::config::<Incr>(json!({"by": 1.5})) else {
        panic!("a fraction is not an i64");
    };
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].path.to_string(), "by");
}

/// Evaluates `x` against `root`, whichever kind of root it is: the value
/// and the operations spent.
fn eval_against<'r>(
    e: &ExprEngine,
    x: &Expr,
    root: impl EvalRoot<'r>,
) -> dataflow_rs::Result<(OwnedDataValue, Option<u64>)> {
    e.eval(x, root).map(|out| (out.value.into_owned(), out.ops))
}

#[test]
fn eval_over_a_json_root_gives_what_its_owned_value_gives() {
    let secrets = Secrets::new(root(json!({"api": {"key": "k-1"}}))).unwrap();
    let e = ExprEngine::builder().secrets(secrets).build().unwrap();
    let json_root = json!({
        "data": {"n": 4, "name": "ada", "which": "api.key"},
        "metadata": {"m": true},
        "temp_data": {}
    });
    let owned_root = root(json_root.clone());
    for raw in [
        json!({"var": "data.name"}),
        json!({"var": "metadata"}),
        json!({"+": [{"var": "data.n"}, 1]}),
        json!({"cat": [{"var": "data.name"}, "/", {"var": "data.n"}]}),
        json!({"secret": {"var": "data.which"}}),
        json!({"cat": [{"secret": "api.key"}, "@", {"var": "data.name"}]}),
    ] {
        let x: Expr = e.compile(&raw).unwrap();
        assert!(x.constant().is_none(), "{raw} reads the root");
        let from_json = eval_against(&e, &x, &json_root).unwrap();
        let from_owned = eval_against(&e, &x, &owned_root).unwrap();
        // The value and, under `budget`, the operations spent.
        assert_eq!(from_json, from_owned, "{raw}");
    }
    let x: Expr = e
        .compile(&json!({"secret": {"var": "data.which"}}))
        .unwrap();
    assert_eq!(
        eval_against(&e, &x, &json_root).unwrap().0,
        root(json!("k-1"))
    );
    // A typed expression, and a failure, are the same over either root.
    let n: Expr<u64> = e.compile(&json!({"var": "data.n"})).unwrap();
    assert_eq!(e.eval(&n, &json_root).unwrap().value, 4);
    let s: Expr<u64> = e.compile(&json!({"var": "data.name"})).unwrap();
    let (a, b) = (
        e.eval(&s, &json_root).unwrap_err(),
        e.eval(&s, &owned_root).unwrap_err(),
    );
    assert_eq!((a.kind(), a.message()), (b.kind(), b.message()));
    // A folded constant is lent over a JSON root too.
    let k: Expr<String> = e.compile(&json!("hi")).unwrap();
    assert!(matches!(
        e.eval(&k, &json_root).unwrap().value,
        Cow::Borrowed("hi")
    ));
}

#[cfg(feature = "budget")]
#[test]
fn a_budget_failure_over_a_json_root_is_budget() {
    let e = ExprEngine::builder().ops_budget(2).build().unwrap();
    let x: Expr<OwnedDataValue> = e
        .compile(&json!({"map": [{"var": "data.l"}, {"+": [{"var": ""}, 1]}]}))
        .unwrap();
    let err = e.eval(&x, &json!({"data": {"l": [1, 2, 3]}})).unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::Budget);
    assert_eq!(err.code(), "BUDGET_EXCEEDED");
}
