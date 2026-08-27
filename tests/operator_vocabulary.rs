//! `Engine::operator_names` — the vocabulary a build actually evaluates.
//!
//! Since datalogic 5.3.0 the built-in half of that vocabulary comes from
//! `Engine::builtin_operator_names` — derived from the very table datalogic's
//! compiler resolves operator keys against — so this crate no longer keeps a
//! copy that could drift. What is still worth testing is that the *reported*
//! vocabulary and the *evaluated* one are the same set: the report is now
//! second-hand, and this crate's own feature flags are what forward to
//! datalogic's. So the check here is that **every name reported is a live
//! operator on the running engine** — checked by evaluating it, not by
//! comparing two lists.
//!
//! That check works because the engine runs datalogic in templating mode: an
//! unknown operator is not an error, the object echoes back as literal data.
//! So "does `{name: args}` come back unchanged?" is exactly "is this name
//! inert?".

use dataflow_rs::Engine;
use serde_json::json;
use std::collections::HashSet;

mod common;

/// Evaluate `{name: args}` and report whether the engine treated it as a live
/// operator rather than echoing it back as data.
///
/// An `Err` counts as live: inert literal data never fails to evaluate, so an
/// argument-arity complaint is itself proof the name dispatched.
fn is_live_operator(engine: &Engine, name: &str) -> bool {
    let expression = json!({ name: [] });
    // `common::eval` collapses a compile failure and an evaluation failure into
    // one `Err`, which is exactly this probe's rule: either is proof the name
    // dispatched.
    match common::eval(engine, expression.clone(), json!({})) {
        Err(_) => true,
        Ok(rendered) => rendered != expression,
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
    {
        assert!(names.contains("switch"));
        // `match` is an input alias of `switch`, and an alias is a live key
        // too — a lint that flagged it would be wrong.
        assert!(names.contains("match"));
    }
    #[cfg(not(feature = "ext-control"))]
    {
        assert!(!names.contains("switch"));
        assert!(!names.contains("match"));
    }

    #[cfg(feature = "datetime")]
    assert!(names.contains("now"));
    #[cfg(not(feature = "datetime"))]
    assert!(!names.contains("now"));

    #[cfg(feature = "ext-object")]
    assert!(names.contains("entries"));
    #[cfg(not(feature = "ext-object"))]
    assert!(!names.contains("entries"));
}

/// One name is one entry, even before any custom registration: the built-in
/// half arrives from datalogic as a flat list of table keys, so a duplicate
/// there would be double-reported here and make a `HashSet`-shaped consumer
/// disagree with a `Vec`-shaped one about how large the vocabulary is.
#[test]
fn no_name_is_reported_twice() {
    let engine = Engine::builder().build().unwrap();

    let reported: Vec<&str> = engine.operator_names().collect();
    let unique: HashSet<&str> = reported.iter().copied().collect();

    assert_eq!(
        reported.len(),
        unique.len(),
        "a name is reported more than once: {reported:?}"
    );
}

#[test]
fn a_registered_custom_operator_joins_the_vocabulary() {
    let bare = Engine::builder().build().unwrap();
    assert!(!bare.operator_names().any(|n| n == "shout"));

    let engine = Engine::builder()
        .with_datalogic_operator("shout", common::Shout)
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
        .with_datalogic_operator("shout", common::Shout)
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
        .with_datalogic_operator("cat", common::Shout)
        .build()
        .unwrap();

    assert_eq!(
        engine.operator_names().filter(|n| *n == "cat").count(),
        1,
        "one name is one entry, whichever side supplied it"
    );
}
