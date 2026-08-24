//! `Engine::operator_names` — the vocabulary a build actually evaluates.
//!
//! The mirror problem this API removes is also the risk it carries: the core
//! names are listed in this crate because datalogic keeps its own table
//! private. So the important test here is not that the list has the right
//! shape, but that **every name in it is a live operator on the running
//! engine** — checked by evaluating it, not by comparing two lists.
//!
//! That check works because the engine runs datalogic in templating mode: an
//! unknown operator is not an error, the object echoes back as literal data.
//! So "does `{name: args}` come back unchanged?" is exactly "is this name
//! inert?".

use dataflow_rs::Engine;
use serde_json::{Value, json};
use std::collections::HashSet;

/// Evaluate `{name: args}` and report whether the engine treated it as a live
/// operator rather than echoing it back as data.
///
/// An `Err` counts as live: inert literal data never fails to evaluate, so an
/// argument-arity complaint is itself proof the name dispatched.
fn is_live_operator(engine: &Engine, name: &str) -> bool {
    let expression = json!({ name: [] });
    let Ok(logic) = engine.datalogic().compile_arc(&expression) else {
        return true; // refused to compile => the name means something
    };

    let arena = dataflow_rs::datalogic_rs::bumpalo::Bump::new();
    match engine.datalogic().evaluate(&logic, &json!({}), &arena) {
        Err(_) => true,
        Ok(value) => {
            let rendered = serde_json::to_value(value).unwrap_or(Value::Null);
            rendered != expression
        }
    }
}

#[test]
fn a_name_outside_the_vocabulary_is_inert_data() {
    // Establishes that the probe above can actually tell the difference —
    // without this, a probe that always returned `true` would make the real
    // test below vacuous.
    let engine = Engine::builder().build().unwrap();

    for typo in ["lenght", "not_an_operator", "starts_wth"] {
        assert!(
            !is_live_operator(&engine, typo),
            "'{typo}' is not an operator, so it must echo back as data"
        );
        assert!(
            !engine.operator_names().any(|n| n == typo),
            "and it must not be in the vocabulary"
        );
    }
}

/// The drift net. Every name this crate mirrors is checked against the engine
/// that will actually evaluate it, so a name datalogic renames or drops fails
/// here rather than silently weakening a downstream lint.
#[test]
fn every_reported_operator_name_is_live() {
    let engine = Engine::builder().build().unwrap();

    for name in engine.operator_names() {
        assert!(
            is_live_operator(&engine, name),
            "'{name}' is reported as an operator but the engine treats it as \
             inert data — the mirrored list has drifted from datalogic"
        );
    }
}

#[test]
fn the_vocabulary_tracks_the_compiled_families() {
    let engine = Engine::builder().build().unwrap();
    let names: HashSet<&str> = engine.operator_names().collect();

    // Core is unconditional.
    for core in ["var", "if", "==", "map", "reduce", "missing"] {
        assert!(names.contains(core), "'{core}' is core vocabulary");
    }

    // Both directions, per the repo's rule about never testing only
    // --all-features: a family that is off must not be listed, because those
    // names really are inert data in that build.
    #[cfg(feature = "ext-string")]
    {
        assert!(names.contains("length"));
        assert!(is_live_operator(&engine, "length"));
    }
    #[cfg(not(feature = "ext-string"))]
    {
        assert!(!names.contains("length"));
        assert!(
            !is_live_operator(&engine, "length"),
            "with ext-string off, {{\"length\": …}} is data, and the vocabulary agrees"
        );
    }

    #[cfg(feature = "ext-control")]
    assert!(names.contains("switch"));
    #[cfg(not(feature = "ext-control"))]
    assert!(!names.contains("switch"));
}

#[test]
fn a_registered_custom_operator_joins_the_vocabulary() {
    let bare = Engine::builder().build().unwrap();
    assert!(!bare.operator_names().any(|n| n == "shout"));

    let engine = Engine::builder()
        .with_datalogic_operator("shout", common_ops::Shout)
        .build()
        .unwrap();

    assert!(
        engine.operator_names().any(|n| n == "shout"),
        "an operator registered through the builder is part of this build's vocabulary"
    );
}

#[test]
fn a_custom_operator_survives_a_hot_reload() {
    let engine = Engine::builder()
        .with_datalogic_operator("shout", common_ops::Shout)
        .build()
        .unwrap();

    let reloaded = engine.with_new_workflows(Vec::new()).unwrap();
    assert!(
        reloaded.operator_names().any(|n| n == "shout"),
        "hot reload rebuilds the datalogic engine and must re-register operators"
    );
}

#[test]
fn a_custom_name_shadowing_a_builtin_is_reported_once() {
    let engine = Engine::builder()
        .with_datalogic_operator("cat", common_ops::Shout)
        .build()
        .unwrap();

    assert_eq!(
        engine.operator_names().filter(|n| *n == "cat").count(),
        1,
        "one name is one entry, whichever side supplied it"
    );
}

mod common_ops {
    pub struct Shout;

    impl dataflow_rs::datalogic_rs::CustomOperator for Shout {
        fn evaluate<'a>(
            &self,
            args: &[&'a dataflow_rs::datalogic_rs::DataValue<'a>],
            _ctx: &mut dataflow_rs::datalogic_rs::operator::EvalContext<'_, 'a>,
            arena: &'a dataflow_rs::datalogic_rs::bumpalo::Bump,
        ) -> dataflow_rs::datalogic_rs::Result<&'a dataflow_rs::datalogic_rs::DataValue<'a>>
        {
            use dataflow_rs::datalogic_rs::ArenaExt;
            let s = args.first().and_then(|v| v.as_str()).unwrap_or_default();
            Ok(arena.string(&s.to_uppercase()))
        }
    }
}
