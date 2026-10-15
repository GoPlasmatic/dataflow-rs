extern crate dataflow_rs_v4 as dataflow_rs;

// The wire shapes read back (I125). A message and a trace v4 writes read
// into v4 and write the same text, and a trace read back replays to the same
// contexts; a message v3 wrote reads into v4 and writes v3's bytes. A context
// takes its three roots in any order, writes them in order, reads a missing
// one as `{}` and any value at each, and refuses any other key: break 22,
// where v3's message read a fourth root as data.

use dataflow_rs::function::FunctionConfig;
use dataflow_rs::observe::{StepResult, Trace, TraceOptions};
use dataflow_rs::run::{Context, RunStatus};
use dataflow_rs::{Ctx, Engine, Error, Expr, Function, Message, Outcome, Result, RunOptions};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

/// `put`'s config.
#[derive(FunctionConfig)]
struct PutConfig {
    /// Written through `Ctx::set` in key order.
    writes: BTreeMap<String, Expr>,
    /// `"ok"`, or `"err"` for a `PARTNER_DOWN` service error.
    outcome: Expr,
}

/// Writes, then returns what its config names.
struct Put;

impl Function for Put {
    type Config = PutConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &PutConfig) -> Result<Outcome> {
        for (path, expr) in &cfg.writes {
            let value = ctx.eval(expr)?.into_owned();
            ctx.set(&path.parse()?, value)?;
        }
        match Value::from(&*ctx.eval(&cfg.outcome)?).as_str() {
            Some("err") => Err(Error::service("PARTNER_DOWN", "down")),
            _ => Ok(Outcome::Ok),
        }
    }
}

/// v3's handler that always fails with a service error.
mod v3_twin {
    use async_trait::async_trait;
    use dataflow_rs_v3::{AsyncFunctionHandler, DataflowError, Result, TaskContext, TaskOutcome};
    use serde_json::Value;

    pub struct Fail;

    #[async_trait]
    impl AsyncFunctionHandler for Fail {
        type Input = Value;

        async fn execute(&self, _ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
            Err(DataflowError::service("PARTNER_DOWN", "down").build())
        }
    }
}

fn text<T: serde::Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).unwrap()
}

fn dv(v: Value) -> OwnedDataValue {
    OwnedDataValue::from(&v)
}

/// What a context read from `json` writes.
fn context(json: &str) -> String {
    text(&serde_json::from_str::<Context>(json).unwrap())
}

/// The message read from `json` refused: its error's text.
fn refused(json: &str) -> String {
    serde_json::from_str::<Message>(json)
        .map(|m| panic!("{json} read as {m:?}"))
        .unwrap_err()
        .to_string()
}

/// A message's JSON over `context`.
fn message_json(context: &str) -> String {
    format!(r#"{{"id":"m","payload":{{}},"context":{context},"audit_trail":[],"errors":[]}}"#)
}

#[tokio::test]
async fn a_v3_message_reads_into_v4_and_writes_v3s_bytes() {
    let workflow = json!({
        "id": "w", "name": "w",
        "tasks": [
            {"id": "m", "name": "m", "function": {"name": "map", "input": {"mappings": [
                {"path": "data.a", "logic": 1},
                {"path": "data.b", "logic": {"cat": [{"var": "data.seed"}, "!"]}},
                {"path": "temp_data.t", "logic": [1, {"x": 2.5}]}
            ]}}},
            {"id": "f", "name": "f", "continue_on_error": true,
             "function": {"name": "fail", "input": {}}},
            {"id": "after", "name": "after", "function": {"name": "map", "input": {"mappings": [
                {"path": "data.after", "logic": {"var": "data.a"}}
            ]}}}
        ]
    });
    let engine = dataflow_rs_v3::Engine::builder()
        .register("fail", v3_twin::Fail)
        .with_workflow(dataflow_rs_v3::Workflow::from_json(&workflow.to_string()).unwrap())
        .build()
        .unwrap();
    let mut v3 = dataflow_rs_v3::Message::builder()
        .id("m")
        .payload_json(&json!({"p": [1, 2.5, "s", null, 2.0], "z": {"y": true}}))
        .data_json(&json!({"seed": "s", "zz": 1, "aa": 2, "f": 3.0}))
        .metadata_json(&json!({"source": "api"}))
        .build();
    engine.process_message(&mut v3).await.unwrap();
    // The run made what the read must carry: changes, and an error the task
    // stepped past.
    assert_eq!(v3.audit_trail().len(), 3);
    assert!(!v3.audit_trail()[0].changes.is_empty());
    assert_eq!(v3.errors().len(), 1, "{:?}", v3.errors());
    let wire = text(&v3);
    assert!(wire.contains(r#""f":3.0"#), "{wire}");

    let v4: Message = serde_json::from_str(&wire).unwrap();
    assert_eq!(text(&v4), wire);
    assert_eq!(v4.id(), "m");
    assert_eq!(
        v4.payload(),
        &dv(json!({"p": [1, 2.5, "s", null, 2.0], "z": {"y": true}}))
    );
    assert_eq!(v4.context().data()["after"], dv(json!(1)));
    assert_eq!(v4.audit().len(), 3);
    assert_eq!(v4.errors()[0].code, "PARTNER_DOWN");
}

#[test]
fn a_context_takes_its_roots_in_any_order_and_writes_them_in_order() {
    let want = r#"{"data":{"z":1,"a":[1,2]},"metadata":{"m":"x"},"temp_data":{"t":{}}}"#;
    for json in [
        want,
        r#"{"temp_data":{"t":{}},"data":{"z":1,"a":[1,2]},"metadata":{"m":"x"}}"#,
        r#"{"metadata":{"m":"x"},"temp_data":{"t":{}},"data":{"z":1,"a":[1,2]}}"#,
    ] {
        assert_eq!(context(json), want, "{json}");
    }
    let c: Context = serde_json::from_str(want).unwrap();
    assert_eq!(c.data(), &dv(json!({"z": 1, "a": [1, 2]})));
    assert_eq!(c.metadata(), &dv(json!({"m": "x"})));
    assert_eq!(c.temp_data(), &dv(json!({"t": {}})));

    // Inside a message too.
    let json = message_json(r#"{"temp_data":{},"metadata":{},"data":{"a":1}}"#);
    let m: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(
        text(&m),
        message_json(r#"{"data":{"a":1},"metadata":{},"temp_data":{}}"#)
    );
}

#[test]
fn a_missing_root_reads_as_an_empty_object() {
    for (json, want) in [
        ("{}", r#"{"data":{},"metadata":{},"temp_data":{}}"#),
        (
            r#"{"metadata":{"a":1}}"#,
            r#"{"data":{},"metadata":{"a":1},"temp_data":{}}"#,
        ),
        (
            r#"{"temp_data":[1],"data":{"d":true}}"#,
            r#"{"data":{"d":true},"metadata":{},"temp_data":[1]}"#,
        ),
    ] {
        assert_eq!(context(json), want, "{json}");
    }
    let m: Message = serde_json::from_str(&message_json("{}")).unwrap();
    assert!(m.context().data().is_object() && m.context().temp_data().is_object());
}

#[test]
fn a_fourth_context_key_is_refused() {
    // Break 22: a fourth key would be readable by `{"var": …}` beside the
    // three roots, so a context refuses it, wherever it sits.
    let smuggled =
        message_json(r#"{"data":{},"metadata":{},"temp_data":{},"secrets":{"partner_key":"k"}}"#);
    let err = refused(&smuggled);
    assert!(
        err.starts_with("a context holds only `data`, `metadata` and `temp_data`, not `secrets`"),
        "{err}"
    );
    assert!(!err.contains("partner_key"), "{err}");
    for json in [
        r#"{"extra":1}"#,
        r#"{"data":{},"extra":null}"#,
        r#"{"extra":{},"data":{},"metadata":{},"temp_data":{}}"#,
    ] {
        let err = serde_json::from_str::<Context>(json)
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with("a context holds only `data`, `metadata` and `temp_data`, not `extra`"),
            "{json}: {err}"
        );
    }
    // A root named twice is refused, and so is a context that is not an
    // object.
    let err = serde_json::from_str::<Context>(r#"{"data":{},"data":{"a":1}}"#).unwrap_err();
    assert!(
        err.to_string().starts_with("duplicate field `data`"),
        "{err}"
    );
    for json in ["null", "[]", r#""data""#, "1"] {
        let err = refused(&message_json(json));
        assert!(err.starts_with("invalid type"), "{json}: {err}");
    }

    // v3 read the same JSON: the fourth root came in as data, and so did a
    // context that is not an object.
    let v3: dataflow_rs_v3::Message = serde_json::from_str(&smuggled).unwrap();
    assert_eq!(v3.context["secrets"]["partner_key"], dv(json!("k")));
    let v3: dataflow_rs_v3::Message = serde_json::from_str(&message_json("null")).unwrap();
    assert_eq!(v3.context, OwnedDataValue::Null);
}

#[tokio::test]
async fn a_null_root_reads_back() {
    // Any value at a root reads back as written, `null` included.
    for json in [
        r#"{"data":null,"metadata":{},"temp_data":{}}"#,
        r#"{"data":5,"metadata":"s","temp_data":[1,{"a":null}]}"#,
    ] {
        assert_eq!(context(json), json);
    }
    let c: Context = serde_json::from_str(r#"{"data":null}"#).unwrap();
    assert_eq!(c.data(), &OwnedDataValue::Null);

    // A trace's redacted root.
    let engine = Engine::builder()
        .workflow_json(
            &json!({"id": "w", "tasks": [{"id": "m", "function": {"name": "map",
            "input": {"mappings": [{"path": "data.a", "logic": 1}]}}}]}),
        )
        .build()
        .unwrap();
    let mut m = Message::from_parts(
        OwnedDataValue::Null,
        dv(json!({"card": "4111"})),
        dv(json!({"k": "v"})),
    )
    .with_id("m");
    let opts = TraceOptions::default().redact(vec!["data".parse().unwrap()]);
    let run = engine
        .run_with(&mut m, &RunOptions::default().trace(opts))
        .await;
    let trace = run.trace().expect("a traced run returns its trace");
    assert_eq!(trace.initial.data(), &OwnedDataValue::Null);
    let wire = text(trace);
    assert!(wire.starts_with(r#"{"initial":{"data":null,"#), "{wire}");
    let back: Trace = serde_json::from_str(&wire).unwrap();
    assert_eq!(back.initial.data(), &OwnedDataValue::Null);
    assert_eq!(text(&back), wire);
}

#[tokio::test]
async fn a_trace_reads_back_and_replays_the_same() {
    let put = |id: &str, writes: Value, outcome: &str| {
        json!({"id": id, "function": {"name": "put", "input": {
            "writes": writes, "outcome": outcome}}})
    };
    let mut fan_out = put("f", json!({"data.last": {"var": "temp_data.x"}}), "ok");
    fan_out["for_each"] = json!({"over": [1, 2], "as": "x"});
    let mut stepped_past = put("boom", json!({}), "err");
    stepped_past["on_failure"] = json!("continue");
    let engine = Engine::builder()
        .register("put", Put)
        .workflow_json(&json!({
            "id": "w",
            "loop": {"counter": "i", "max": 2},
            "tasks": [
                {"id": "m", "function": {"name": "map", "input": {"mappings": [
                    {"path": "data.card", "logic": "4111"},
                    {"path": "data.n", "logic": {"+": [{"var": "temp_data.i"}, 0.5]}}
                ]}}},
                fan_out,
                stepped_past,
                {"id": "never", "condition": false, "function": {"name": "map",
                    "input": {"mappings": [{"path": "data.never", "logic": 1}]}}}
            ]
        }))
        .workflow_json(&json!({
            "id": "v", "priority": 1, "on_failure": "continue",
            "tasks": [put("fails", json!({"data.lost": 1}), "err")]
        }))
        .build()
        .unwrap_or_else(|issues| panic!("the engine builds: {issues:?}"));
    let mut m = Message::from_parts(
        dv(json!({"in": [1, 2]})),
        dv(json!({"seed": {"z": 1, "a": 2}})),
        OwnedDataValue::Null,
    )
    .with_id("m");
    let opts = TraceOptions::default()
        .timings(true)
        .redact(vec!["data.card".parse().unwrap()]);
    let run = engine
        .run_with(&mut m, &RunOptions::default().trace(opts))
        .await;
    assert!(
        matches!(run.status(), RunStatus::CompletedWithErrors),
        "{run:?}"
    );
    let trace = run.trace().expect("a traced run returns its trace");

    // The trace holds every shape a step takes.
    let steps = &trace.steps;
    assert!(steps.iter().any(|s| s.loop_counter.is_some()), "a loop");
    assert!(steps.iter().any(|s| s.element_index.is_some()), "a fan-out");
    assert!(
        steps
            .iter()
            .any(|s| s.result == StepResult::Executed && !s.errors.is_empty()),
        "a failure stepped past"
    );
    assert!(
        steps.iter().any(|s| s.mapping_ends == [1, 2]),
        "a map's mapping ends"
    );
    assert!(
        steps.iter().any(|s| s.result == StepResult::Failed),
        "a failure"
    );
    assert!(
        steps.iter().any(|s| s.result == StepResult::Skipped),
        "a skip"
    );
    assert!(steps.iter().any(|s| s.elapsed.is_some()), "timings");
    let wire = text(trace);
    assert!(!wire.contains("4111"), "redaction: {wire}");
    assert!(wire.contains(r#"{"path":"data.card","old_value":null,"new_value":null}"#));

    let back: Trace = serde_json::from_str(&wire).unwrap();
    assert_eq!(text(&back), wire);
    let replayed = |t: &Trace| t.contexts().map(|c| text(&c)).collect::<Vec<_>>();
    assert_eq!(replayed(&back), replayed(trace));
    // A step's elapsed reads back in whole microseconds, as it was written.
    for (step, read) in steps.iter().zip(&back.steps) {
        let micros = step
            .elapsed
            .map(|d| Duration::from_micros(d.as_micros() as u64));
        assert_eq!(read.elapsed, micros);
    }
}

#[test]
fn step_result_and_the_ids_read_back() {
    use dataflow_rs::program::{TaskIx, WorkflowIx};
    for (json, result) in [
        (r#""executed""#, StepResult::Executed),
        (r#""skipped""#, StepResult::Skipped),
        (r#""failed""#, StepResult::Failed),
    ] {
        assert_eq!(serde_json::from_str::<StepResult>(json).unwrap(), result);
        assert_eq!(text(&result), json);
    }
    assert!(serde_json::from_str::<StepResult>(r#""Executed""#).is_err());

    let workflow: WorkflowIx = serde_json::from_str("3").unwrap();
    assert_eq!((workflow.index(), text(&workflow)), (3, "3".to_owned()));
    let task: TaskIx = serde_json::from_str("7").unwrap();
    assert_eq!((task.index(), text(&task)), (7, "7".to_owned()));
    for json in ["-1", r#""3""#, "1.5", "null"] {
        assert!(serde_json::from_str::<TaskIx>(json).is_err(), "{json}");
        assert!(serde_json::from_str::<WorkflowIx>(json).is_err(), "{json}");
    }
}

#[tokio::test]
async fn a_whole_number_float_reads_back_as_written() {
    // `OwnedDataValue`'s own reader makes `2.0` the integer `2`; a message
    // and a trace read every value as written, as `Message::from_json`
    // builds it.
    let engine = Engine::builder()
        .workflow_json(
            &json!({"id": "w", "tasks": [{"id": "m", "function": {"name": "map",
            "input": {"mappings": [{"path": "data.c", "logic": 2.0}]}}}]}),
        )
        .build()
        .unwrap();
    let mut m = Message::from_parts(
        dv(json!([1.0])),
        dv(json!({"f": 3.0, "i": 3})),
        OwnedDataValue::Null,
    )
    .with_id("m");
    let run = engine
        .run_with(
            &mut m,
            &RunOptions::default().trace(TraceOptions::default()),
        )
        .await;
    let wire = text(&m);
    for written in [
        r#""payload":[1.0]"#,
        r#""f":3.0,"i":3"#,
        r#""new_value":2.0"#,
    ] {
        assert!(wire.contains(written), "{written}: {wire}");
    }
    let back: Message = serde_json::from_str(&wire).unwrap();
    assert_eq!(text(&back), wire);
    assert_eq!(back, m);

    let trace = text(run.trace().expect("a traced run returns its trace"));
    assert!(trace.contains(r#""f":3.0"#) && trace.contains(r#""new_value":2.0"#));
    assert_eq!(text(&serde_json::from_str::<Trace>(&trace).unwrap()), trace);
}

#[tokio::test]
async fn an_arithmetic_float_reads_back() {
    // `0.1 + 0.33` is `0.43000000000000005`, seventeen significant digits,
    // which serde_json's default parser reads one ULP off; v4 builds it with
    // `float_roundtrip`, so the message and the trace read back byte for byte.
    let engine = Engine::builder()
        .workflow_json(&json!({"id": "w", "tasks": [{"id": "m", "function": {
            "name": "map", "input": {"mappings": [{"path": "data.total",
            "logic": {"+": [{"var": "data.a"}, {"var": "data.b"}]}}]}}}]}))
        .build()
        .unwrap();
    let mut m = Message::from_parts(
        OwnedDataValue::Null,
        dv(json!({"a": 0.1, "b": 0.33})),
        OwnedDataValue::Null,
    )
    .with_id("m");
    let run = engine
        .run_with(
            &mut m,
            &RunOptions::default().trace(TraceOptions::default()),
        )
        .await;
    let wire = text(&m);
    assert!(wire.contains(r#""total":0.43000000000000005"#), "{wire}");
    let back: Message = serde_json::from_str(&wire).unwrap();
    assert_eq!(text(&back), wire);
    assert_eq!(back, m);

    let trace = run.trace().expect("a traced run returns its trace");
    let written = text(trace);
    let read: Trace = serde_json::from_str(&written).unwrap();
    assert_eq!(text(&read), written);
    let replayed = |t: &Trace| t.contexts().map(|c| text(&c)).collect::<Vec<_>>();
    assert_eq!(replayed(&read), replayed(trace));
}

#[test]
fn every_finite_float_reads_back() {
    // A batch of finite floats from random bit patterns (xorshift64*, seeded,
    // so a failure reproduces), each read back as the bits it was written from.
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut floats = Vec::new();
    while floats.len() < 20_000 {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let f = f64::from_bits(state.wrapping_mul(0x2545_F491_4F6C_DD1D));
        if f.is_finite() {
            floats.push(f);
        }
    }
    let values = floats.iter().map(|&f| json!(f)).collect::<Vec<_>>();
    let m = Message::from_parts(
        OwnedDataValue::Null,
        dv(json!({"f": values})),
        dv(json!({})),
    )
    .with_id("m");
    let wire = text(&m);
    let back: Message = serde_json::from_str(&wire).unwrap();
    let Some(OwnedDataValue::Array(read)) = back.context().data().get("f") else {
        panic!("an array");
    };
    let moved = floats
        .iter()
        .zip(read)
        .filter(|(written, read)| {
            Value::from(*read).as_f64().map(f64::to_bits) != Some(written.to_bits())
        })
        .count();
    assert_eq!((read.len(), moved), (floats.len(), 0), "floats that moved");
    assert!(text(&back) == wire, "the message reads back byte for byte");
}
