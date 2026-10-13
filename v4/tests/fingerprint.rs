extern crate dataflow_rs_v4 as dataflow_rs;

use dataflow_rs::datalogic_rs::{self, CustomOperator};
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::expr::{ExprEngine, Interpolated};
use dataflow_rs::program::{CompileOptions, Fingerprint, ParseFingerprintError};
use dataflow_rs::{Ctx, Function, Outcome, Registry, Result};
use serde_json::{Value, json};

#[derive(dataflow_rs::function::FunctionConfig)]
struct DocConfig {
    body: Interpolated,
}

/// A host function with an `Interpolated` document.
struct Doc;

impl Function for Doc {
    type Config = DocConfig;

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &DocConfig) -> Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

struct Shout;

impl CustomOperator for Shout {
    fn evaluate<'a>(
        &self,
        _args: &[&'a datalogic_rs::DataValue<'a>],
        _ctx: &mut datalogic_rs::operator::EvalContext<'_, 'a>,
        arena: &'a datalogic_rs::bumpalo::Bump,
    ) -> datalogic_rs::Result<&'a datalogic_rs::DataValue<'a>> {
        Ok(arena.alloc(datalogic_rs::DataValue::Null))
    }
}

fn fingerprint_in(registry: Registry, json: &Value) -> Fingerprint {
    let parsed = parse(json, ParseOptions::default());
    let def = parsed.def.expect("the definition parses");
    let compiled = registry
        .compiler(CompileOptions::default())
        .compile_one(&def)
        .unwrap_or_else(|issues| panic!("compiles: {issues:?}"));
    compiled.workflow.fingerprint()
}

fn fingerprint(json: &Value) -> Fingerprint {
    let mut registry = Registry::default();
    registry.register("doc", Doc);
    fingerprint_in(registry, json)
}

fn base() -> Value {
    json!({
        "id": "w",
        "condition": {"==": [{"var": "data.kind"}, "a"]},
        "tasks": [
            {"id": "m", "function": {"name": "map", "input": {"mappings": [
                {"path": "data.out", "logic": {"result": {"var": "data.x"}}},
                {"path": "data.esc", "logic": {"$shape": 1}},
            ]}}},
            {"id": "l", "function": {"name": "log", "input": {"message": "hi"}}},
        ]
    })
}

/// The golden value. A stored fingerprint is compared against a fresh one
/// across processes and releases, so this literal must never change.
///
/// It hashes exactly the operators `{"==", "var"}` and the data keys
/// `{"result"}`. The escaped `{"$shape": 1}` contributes nothing: with
/// `$shape` among the data keys the hash would differ.
#[test]
fn the_golden_fingerprint_never_changes() {
    assert_eq!(fingerprint(&base()).to_string(), "v1:fbe0013c1692ddb4");
}

#[test]
fn the_fingerprint_ignores_order_ids_repeats_and_literals() {
    let want = fingerprint(&base());
    let variant = |edit: &dyn Fn(&mut Value)| {
        let mut v = base();
        edit(&mut v);
        fingerprint(&v)
    };
    // Reordered tasks and mappings.
    assert_eq!(
        variant(&|v| {
            v["tasks"].as_array_mut().unwrap().reverse();
            v["tasks"][1]["function"]["input"]["mappings"]
                .as_array_mut()
                .unwrap()
                .reverse();
        }),
        want
    );
    // Reordered object keys.
    let mut keys = base();
    keys["condition"] = json!({"==": ["a", {"var": "data.kind"}]});
    assert_eq!(fingerprint(&keys), want);
    // Renamed ids.
    assert_eq!(
        variant(&|v| {
            v["id"] = json!("other");
            v["tasks"][0]["id"] = json!("renamed");
        }),
        want
    );
    // A second identical mapping.
    assert_eq!(
        variant(&|v| {
            let m = v["tasks"][0]["function"]["input"]["mappings"][0].clone();
            v["tasks"][0]["function"]["input"]["mappings"]
                .as_array_mut()
                .unwrap()
                .push(m);
        }),
        want
    );
    // A changed literal.
    assert_eq!(
        variant(&|v| v["tasks"][1]["function"]["input"]["message"] = json!("bye")),
        want
    );
    // A different data key does change it.
    assert_ne!(
        variant(
            &|v| v["tasks"][0]["function"]["input"]["mappings"][0]["logic"] =
                json!({"answer": {"var": "data.x"}})
        ),
        want
    );
}

#[test]
fn an_interpolated_document_is_not_classified() {
    let with = |body: Value| {
        json!({"id": "w", "tasks": [
            {"id": "d", "function": {"name": "doc", "input": {"body": body}}}
        ]})
    };
    let a = fingerprint(&with(json!({"k": {"var": "data.a"}, "x": {"$oid": "1"}})));
    let b = fingerprint(&with(json!({"other": {"cat": ["x"]}, "y": 2})));
    assert_eq!(a, b);
}

#[test]
fn a_registered_operator_changes_the_fingerprint() {
    let json = json!({"id": "w", "tasks": [
        {"id": "m", "function": {"name": "map", "input": {"mappings": [
            {"path": "data.out", "logic": {"shout": {"var": "data.x"}}}
        ]}}}
    ]});
    let plain = fingerprint_in(Registry::default(), &json);
    let engine = ExprEngine::builder()
        .operator("shout", Shout)
        .build()
        .unwrap();
    let with = fingerprint_in(Registry::new(engine), &json);
    assert_ne!(plain, with);
}

#[test]
fn display_and_from_str_round_trip_and_refuse_the_rest() {
    let f = fingerprint(&base());
    assert_eq!(f.to_string().parse::<Fingerprint>(), Ok(f));
    let upper = format!("v1:{}", f.to_string()[3..].to_uppercase());
    assert_eq!(upper.parse::<Fingerprint>(), Ok(f));
    let hex = &f.to_string()[3..];
    for bad in [
        format!("v2:{hex}"),
        format!("v1:{}", &hex[..15]),
        format!("v1:{hex}0"),
        "v1:zzzzzzzzzzzzzzzz".to_owned(),
        hex.to_owned(),
        String::new(),
    ] {
        let err: ParseFingerprintError = bad.parse::<Fingerprint>().unwrap_err();
        assert!(err.to_string().contains("not a fingerprint"), "{bad}");
    }
}
