use dataflow_rs::Result;
use dataflow_rs::datalogic_rs::{self, CustomOperator};
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::{ExprEngine, OperatorFamily, Secrets};

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
fn the_operator_types_do_not_collide_with_datalogics() {
    use dataflow_rs::datalogic_rs::{Family, OperatorInfo};
    use dataflow_rs::expr::{Operator, OperatorFamily};

    let _: Option<Operator> = None;
    let _: Option<OperatorFamily> = None;
    let _: Option<OperatorInfo> = None;
    let _: Option<Family> = None;
}

#[test]
fn the_table_is_complete_and_operators_lists_builtins_customs_then_secret() {
    let engine = ExprEngine::builder()
        .operator("alpha", Nop)
        .operator("beta", Nop)
        .build()
        .unwrap();
    let live: Vec<&str> = datalogic_rs::Engine::builder()
        .with_families([])
        .build()
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
        assert_eq!(o.family, OperatorFamily::Core, "{}", o.name);
    }
    let secret = ops.last().unwrap();
    assert_eq!(secret.family, OperatorFamily::Secret);
    assert!(secret.deterministic && !secret.scoping);
    let alpha = &ops[ops.len() - 3];
    assert_eq!(alpha.family, OperatorFamily::Custom);
    assert!(!alpha.scoping && !alpha.deterministic);
}

#[cfg(feature = "datetime")]
#[test]
fn now_is_the_only_non_deterministic_builtin() {
    let engine = ExprEngine::builder().all_operators().build().unwrap();
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
    // `upper` is free on the default Core-only engine, and taken once
    // `ExtString` is selected.
    assert!(ExprEngine::builder().operator("upper", Nop).build().is_ok());
    assert!(config_error(
        ExprEngine::builder()
            .family(OperatorFamily::ExtString)
            .operator("upper", Nop)
            .build()
    ));
    assert!(config_error(
        ExprEngine::builder().family(OperatorFamily::Custom).build()
    ));
    #[cfg(not(feature = "datetime"))]
    assert!(config_error(
        ExprEngine::builder()
            .family(OperatorFamily::Datetime)
            .build()
    ));
    #[cfg(not(feature = "tensor"))]
    assert!(config_error(
        ExprEngine::builder().family(OperatorFamily::Tensor).build()
    ));
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
}

struct CheckedOp;

impl CustomOperator for CheckedOp {
    fn evaluate<'a>(
        &self,
        args: &[&'a datalogic_rs::DataValue<'a>],
        _ctx: &mut datalogic_rs::operator::EvalContext<'_, 'a>,
        arena: &'a datalogic_rs::bumpalo::Bump,
    ) -> datalogic_rs::Result<&'a datalogic_rs::DataValue<'a>> {
        Ok(args
            .first()
            .copied()
            .unwrap_or_else(|| arena.alloc(datalogic_rs::DataValue::Null)))
    }

    fn info(&self) -> datalogic_rs::CustomOperatorInfo {
        let mut info = datalogic_rs::CustomOperatorInfo::default();
        info.deterministic = true;
        info.reads_context = false;
        info.min_args = 1;
        info.max_args = Some(1);
        info
    }

    fn check(&self, args: &[OwnedDataValue]) -> std::result::Result<(), datalogic_rs::Diagnostic> {
        if args.len() != 1 {
            return Err(datalogic_rs::Diagnostic::error(
                "checked_op requires exactly 1 argument",
            ));
        }
        Ok(())
    }
}

#[test]
fn custom_operator_info_and_check_are_forwarded_through_shared_operator() {
    let engine = ExprEngine::builder()
        .operator("checked_op", CheckedOp)
        .build()
        .unwrap();

    // Valid arity compiles cleanly, and because `SharedOperator::info` forwards
    // `deterministic = true` and `reads_context = false`, Datalogic marks the
    // expression constant so `ExprEngine::compile` folds it at compile time.
    let compiled = engine
        .compile::<OwnedDataValue>(&serde_json::json!({"checked_op": [42]}))
        .expect("1 argument is accepted by check()");
    assert_eq!(
        compiled.constant(),
        Some(&OwnedDataValue::from(&serde_json::json!(42)))
    );

    // Invalid arity does not fold at compile time and fails at evaluation
    // before `CheckedOp::evaluate` runs, with `InvalidArguments` preserved on
    // `datalogic_error()` and `expr_cause()`.
    let bad = engine
        .compile::<OwnedDataValue>(&serde_json::json!({"checked_op": [1, 2]}))
        .expect("compiles to an unfettered node whose arity is checked before evaluate");
    assert!(bad.constant().is_none());
    let err = engine
        .eval(&bad, &OwnedDataValue::Null)
        .expect_err("arity check from info() rejects 2 args");
    assert_eq!(err.kind(), &dataflow_rs::ErrorKind::Eval);
    assert!(err.datalogic_error().is_some());
    assert_eq!(
        err.expr_cause().map(|c| c.code),
        Some(datalogic_rs::ErrorCode::InvalidArguments)
    );
}
