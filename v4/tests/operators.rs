extern crate dataflow_rs_v4 as dataflow_rs;

use dataflow_rs::datalogic_rs::{self, CustomOperator};
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::{ExprEngine, Family, Secrets};
use dataflow_rs::{Error, Result};

struct Nop;

impl CustomOperator for Nop {
    fn evaluate<'a>(
        &self,
        _args: &[&'a datalogic_rs::DataValue<'a>],
        _ctx: &mut datalogic_rs::operator::EvalContext<'_, 'a>,
        arena: &'a datalogic_rs::bumpalo::Bump,
    ) -> datalogic_rs::Result<&'a datalogic_rs::DataValue<'a>> {
        Ok(arena.alloc(datalogic_rs::DataValue::Null))
    }
}

fn config_error(result: Result<ExprEngine>) -> bool {
    matches!(result, Err(e) if matches!(e.kind(), dataflow_rs::ErrorKind::Config))
}

#[test]
fn the_table_is_complete_and_operators_lists_builtins_customs_then_secret() {
    let engine = ExprEngine::builder()
        .operator("alpha", Nop)
        .operator("beta", Nop)
        .build()
        .unwrap();
    let live: Vec<&str> = datalogic_rs::Engine::new()
        .builtin_operator_names()
        .collect();
    let ops = engine.operators();
    let names: Vec<&str> = ops.iter().map(|o| &*o.name).collect();
    let mut expected: Vec<&str> = live.clone();
    expected.extend(["alpha", "beta", "secret"]);
    let mut sorted_names = names.clone();
    sorted_names.sort_unstable();
    let mut sorted_expected = expected.clone();
    sorted_expected.sort_unstable();
    assert_eq!(sorted_names, sorted_expected);
    // Builtins keep datalogic's order, customs registration order, secret last.
    assert_eq!(&names[names.len() - 3..], ["alpha", "beta", "secret"]);
    for o in ops.iter().filter(|o| live.contains(&&*o.name)) {
        assert_ne!(o.family, Family::Custom, "{}", o.name);
        assert_ne!(o.family, Family::Secret, "{}", o.name);
    }
    let secret = ops.last().unwrap();
    assert_eq!(secret.family, Family::Secret);
    assert!(secret.deterministic && !secret.scoping);
    let alpha = &ops[ops.len() - 3];
    assert_eq!(alpha.family, Family::Custom);
    assert!(!alpha.scoping && !alpha.deterministic);
}

#[cfg(feature = "datetime")]
#[test]
fn now_is_the_only_non_deterministic_builtin() {
    let engine = ExprEngine::default();
    let non: Vec<&str> = engine
        .operators()
        .iter()
        .filter(|o| !o.deterministic)
        .map(|o| &*o.name)
        .collect();
    assert_eq!(non, ["now"]);
}

#[test]
fn the_builder_refuses_secret_a_builtin_and_a_repeat() {
    assert!(config_error(
        ExprEngine::builder().operator("secret", Nop).build()
    ));
    assert!(config_error(
        ExprEngine::builder().operator("+", Nop).build()
    ));
    assert!(config_error(
        ExprEngine::builder()
            .operator("twice", Nop)
            .operator("twice", Nop)
            .build()
    ));
    assert!(ExprEngine::builder().operator("once", Nop).build().is_ok());
}

#[test]
fn secrets_mask_their_values_and_reject_non_objects() {
    let root = OwnedDataValue::from(&serde_json::json!({"a": {"b": "hunter2"}, "k": "v"}));
    let secrets = Secrets::new(root).unwrap();
    assert_eq!(
        secrets.get("a.b").unwrap(),
        &OwnedDataValue::from(&serde_json::json!("hunter2"))
    );
    assert!(secrets.get("").is_none());
    let shown = format!("{secrets:?}");
    assert!(
        !shown.contains("hunter2") && shown.contains("******"),
        "{shown}"
    );
    let engine = ExprEngine::builder().secrets(secrets).build().unwrap();
    assert_eq!(engine.secrets().names().collect::<Vec<_>>(), ["a", "k"]);
    assert!(Secrets::new(OwnedDataValue::from(&serde_json::json!([1]))).is_err());
    let _: fn(Error) = drop;
}
