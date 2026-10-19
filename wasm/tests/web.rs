#![cfg(target_arch = "wasm32")]

//! The package on wasm32 under Node: v4's first run on
//! `wasm32-unknown-unknown`, ids through `uuid/js`, `processed_at` through
//! `chrono/wasmbind`, timings and deadlines through `web-time`.
//!
//! Deliberately no `wasm_bindgen_test_configure!(run_in_browser)`: nothing
//! here touches a browser-only API, so the suite runs under both
//! `wasm-pack test --node` (no driver, so CI can run it) and `--headless`.

use dataflow_wasm::*;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

/// A JS value from JSON text.
fn js(text: &str) -> JsValue {
    js_sys::JSON::parse(text).expect("the test's JSON parses")
}

/// A JS object from JSON text.
fn obj(text: &str) -> js_sys::Object {
    js(text).into()
}

/// A JS value read back as JSON.
fn json_of(v: &JsValue) -> Value {
    let text = js_sys::JSON::stringify(v).expect("a result stringifies");
    serde_json::from_str(&text.as_string().expect("a string")).expect("a result is JSON")
}

/// A thrown `EngineError`'s `code`, `message` and `issues`.
fn error_parts(e: &JsValue) -> (String, String, Value) {
    let get = |key: &str| js_sys::Reflect::get(e, &JsValue::from_str(key)).unwrap();
    let code = get("code").as_string().unwrap_or_default();
    let message = get("message").as_string().unwrap_or_default();
    let issues = get("issues");
    let issues = if issues.is_undefined() {
        Value::Null
    } else {
        json_of(&issues)
    };
    (code, message, issues)
}

/// A workflow whose one task writes `logic` at `path`.
fn mapping_workflow(id: &str, path: &str, logic: Value) -> Value {
    json!({
        "id": id,
        "tasks": [{"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": path, "logic": logic}]}}}]
    })
}

fn engine(workflows: Value) -> WasmEngine {
    WasmEngine::new(js(&workflows.to_string()), None).expect("the workflows load")
}

async fn run(engine: &WasmEngine, payload: JsValue, options: Option<&str>) -> Value {
    let promise = engine.run(payload, options.map(obj));
    let result = JsFuture::from(promise).await.expect("the run resolves");
    json_of(&result)
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
fn an_engine_builds_from_json_text_and_from_an_array() {
    let workflows = json!([
        mapping_workflow("a", "data.x", json!(1)),
        mapping_workflow("b", "data.y", json!(2))
    ]);
    let from_text = WasmEngine::new(JsValue::from_str(&workflows.to_string()), None).unwrap();
    assert_eq!(from_text.workflow_count(), 2);
    let from_array = engine(workflows);
    assert_eq!(
        from_array.workflow_ids(),
        vec!["a".to_owned(), "b".to_owned()]
    );
}

#[wasm_bindgen_test]
fn malformed_arguments_are_invalid_argument_errors() {
    let (code, message, issues) =
        error_parts(&WasmEngine::new(JsValue::from_str("not json"), None).unwrap_err());
    assert_eq!(code, "INVALID_ARGUMENT");
    assert!(message.starts_with("workflows: "), "{message}");
    assert!(issues.is_null());

    let (code, message, _) =
        error_parts(&WasmEngine::new(js(r#"{"id": "single"}"#), None).unwrap_err());
    assert_eq!(code, "INVALID_ARGUMENT");
    assert!(message.contains("must be an array"), "{message}");

    // An unknown option key is refused, so a typo cannot pass silently.
    let (code, message, _) =
        error_parts(&WasmEngine::new(js("[]"), Some(obj(r#"{"secret": {}}"#))).unwrap_err());
    assert_eq!(code, "INVALID_ARGUMENT");
    assert!(message.contains("secret"), "{message}");
}

#[wasm_bindgen_test]
fn a_refused_definition_is_a_rejected_error_carrying_the_issues() {
    let workflows =
        json!([{"id": "w", "tasks": [{"id": "t", "function": {"name": "nope", "input": {}}}]}]);
    let (code, message, issues) =
        error_parts(&WasmEngine::new(js(&workflows.to_string()), None).unwrap_err());
    assert_eq!(code, "REJECTED");
    assert!(message.contains("UNKNOWN_FUNCTION"), "{message}");
    let issues = issues.as_array().expect("issues is an array");
    assert_eq!(issues[0]["code"], "UNKNOWN_FUNCTION");
    assert_eq!(issues[0]["severity"], "REJECTED");
    assert_eq!(issues[0]["path"], "tasks[0].function.name");
}

#[wasm_bindgen_test]
async fn secrets_are_read_through_the_options_and_an_undeclared_name_is_rejected() {
    let workflows = json!([{
        "id": "gated",
        "condition": {"==": [{"secret": "token"}, "t0k"]},
        "tasks": [{"id": "mark", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.ran", "logic": true}]}}}]
    }]);
    let (code, _, issues) =
        error_parts(&WasmEngine::new(js(&workflows.to_string()), None).unwrap_err());
    assert_eq!(code, "REJECTED");
    assert!(
        issues.to_string().contains("token"),
        "the issue names the secret: {issues}"
    );

    let engine = WasmEngine::new(
        js(&workflows.to_string()),
        Some(obj(r#"{"secrets": {"token": "t0k"}}"#)),
    )
    .unwrap();
    let result = run(&engine, js("{}"), None).await;
    assert_eq!(result["message"]["context"]["data"]["ran"], true);
    // The store is the engine's: nothing of it reaches the message.
    assert!(!result.to_string().contains("t0k"));

    let (code, _, _) = error_parts(
        &WasmEngine::new(
            js(&workflows.to_string()),
            Some(obj(r#"{"secrets": ["t0k"]}"#)),
        )
        .unwrap_err(),
    );
    assert_eq!(code, "INVALID_ARGUMENT");
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
async fn a_string_payload_stays_a_string_and_parse_json_reads_it() {
    let engine = engine(json!([{
        "id": "mapper",
        "tasks": [
            {"id": "parse", "function": {"name": "parse_json", "input": {"source": "payload", "target": "input"}}},
            {"id": "copy", "function": {"name": "map", "input": {
                "mappings": [{"path": "data.output_name", "logic": {"var": "data.input.input_name"}}]}}}
        ]
    }]));
    let result = run(
        &engine,
        JsValue::from_str(r#"{"input_name": "Alice"}"#),
        None,
    )
    .await;
    assert_eq!(result["message"]["payload"], r#"{"input_name": "Alice"}"#);
    assert_eq!(result["message"]["context"]["data"]["output_name"], "Alice");
    assert_eq!(result["run"]["status"], "completed");
    assert_eq!(result["run"]["matched"], 1);
    assert!(result.get("trace").is_none(), "no trace unless asked");
}

#[wasm_bindgen_test]
async fn an_object_payload_is_stored_as_the_object_it_is() {
    let engine = engine(json!([mapping_workflow(
        "w",
        "data.processed",
        json!(true)
    )]));
    let result = run(&engine, js(r#"{"some": "data", "n": 2}"#), None).await;
    assert_eq!(
        result["message"]["payload"],
        json!({"some": "data", "n": 2})
    );
    assert_eq!(result["message"]["context"]["data"]["processed"], true);
    // `undefined` is `null`.
    let result = run(&engine, JsValue::UNDEFINED, None).await;
    assert_eq!(result["message"]["payload"], Value::Null);
}

#[wasm_bindgen_test]
async fn the_message_options_seed_the_id_data_and_metadata() {
    let engine = engine(json!([mapping_workflow(
        "w",
        "data.out",
        json!({"var": "data.name"})
    )]));
    let result = run(
        &engine,
        JsValue::NULL,
        Some(r#"{"id": "m-1", "data": {"name": "n"}, "metadata": {"tenant": "t"}}"#),
    )
    .await;
    assert_eq!(result["message"]["id"], "m-1");
    assert_eq!(result["message"]["context"]["data"]["out"], "n");
    assert_eq!(result["message"]["context"]["metadata"]["tenant"], "t");
}

#[wasm_bindgen_test]
async fn the_id_is_a_uuid_v7_and_processed_at_is_stamped() {
    let engine = engine(json!([mapping_workflow("w", "data.x", json!(1))]));
    let result = run(&engine, JsValue::NULL, None).await;
    let id = result["message"]["id"].as_str().unwrap();
    assert_eq!(id.len(), 36, "{id}");
    assert_eq!(&id[14..15], "7", "a UUID v7: {id}");
    let stamp = result["message"]["context"]["metadata"]["processed_at"]
        .as_str()
        .expect("processed_at is stamped");
    assert!(stamp.starts_with("20"), "{stamp}");
    assert_eq!(
        result["message"]["context"]["metadata"]["engine_version"],
        engine_version()
    );
}

#[wasm_bindgen_test]
async fn a_condition_selects_and_the_run_says_why_a_workflow_did_not_run() {
    let engine = engine(json!([
        {
            "id": "conditional",
            "condition": {"==": [{"var": "data.should_run"}, true]},
            "tasks": [{"id": "mark", "function": {"name": "map", "input": {
                "mappings": [{"path": "data.ran", "logic": true}]}}}]
        }
    ]));
    let ran = run(
        &engine,
        JsValue::NULL,
        Some(r#"{"data": {"should_run": true}}"#),
    )
    .await;
    assert_eq!(ran["message"]["context"]["data"]["ran"], true);
    assert_eq!(ran["run"]["workflows"][0]["selection"], "ran");

    let skipped = run(
        &engine,
        JsValue::NULL,
        Some(r#"{"data": {"should_run": false}}"#),
    )
    .await;
    assert!(skipped["message"]["context"]["data"]["ran"].is_null());
    assert_eq!(skipped["run"]["matched"], 0);
    assert_eq!(
        skipped["run"]["workflows"][0]["selection"],
        "skipped_by_condition"
    );
}

#[wasm_bindgen_test]
async fn channel_and_routing_bucket_select_workflows() {
    let engine = engine(json!([
        {"id": "orders", "channel": "orders", "tasks": [{"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.orders", "logic": true}]}}}]},
        {"id": "rolled", "rollout": {"bucket_start": 0, "bucket_end": 10}, "tasks": [{"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.rolled", "logic": true}]}}}]}
    ]));
    let orders = run(&engine, JsValue::NULL, Some(r#"{"channel": "orders"}"#)).await;
    assert_eq!(orders["message"]["context"]["data"]["orders"], true);
    assert!(orders["message"]["context"]["data"]["rolled"].is_null());

    let out = run(&engine, JsValue::NULL, Some(r#"{"routing_bucket": 99}"#)).await;
    assert!(out["message"]["context"]["data"]["rolled"].is_null());
    let record = out["run"]["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == "rolled")
        .unwrap()
        .clone();
    assert_eq!(record["selection"], "skipped_by_rollout");

    let within = run(&engine, JsValue::NULL, Some(r#"{"routing_bucket": 5}"#)).await;
    assert_eq!(within["message"]["context"]["data"]["rolled"], true);

    // Buckets are 0 to 99: a hash `% 256` would match no `[x, 100)` range.
    for bucket in ["100", "255"] {
        let options = format!(r#"{{"routing_bucket": {bucket}}}"#);
        let promise = engine.run(JsValue::NULL, Some(obj(&options)));
        let (code, message, _) = error_parts(&JsFuture::from(promise).await.unwrap_err());
        assert_eq!(code, "INVALID_ARGUMENT", "{bucket}");
        assert!(message.contains("options.routing_bucket"), "{message}");
    }
}

#[wasm_bindgen_test]
async fn the_audit_options_govern_the_trail() {
    let engine = engine(json!([mapping_workflow("w", "data.x", json!(1))]));
    let full = run(&engine, JsValue::NULL, None).await;
    let trail = full["message"]["audit_trail"].as_array().unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0]["changes"][0]["path"], "data.x");

    let uncaptured = run(
        &engine,
        JsValue::NULL,
        Some(r#"{"capture_changes": false}"#),
    )
    .await;
    assert_eq!(
        uncaptured["message"]["audit_trail"][0]["changes"],
        json!([])
    );

    let off = run(&engine, JsValue::NULL, Some(r#"{"audit": "off"}"#)).await;
    assert_eq!(off["message"]["audit_trail"], json!([]));

    let last = run(&engine, JsValue::NULL, Some(r#"{"audit": {"last": 0}}"#)).await;
    assert_eq!(last["message"]["audit_trail"], json!([]));

    let promise = engine.run(JsValue::NULL, Some(obj(r#"{"audit": "some"}"#)));
    let (code, message, _) = error_parts(&JsFuture::from(promise).await.unwrap_err());
    assert_eq!(code, "INVALID_ARGUMENT");
    assert!(message.contains("options.audit"), "{message}");
}

#[wasm_bindgen_test]
async fn a_failing_run_resolves_with_its_status_its_error_and_its_failed_step() {
    // An `append` over a string fails the mapping, and the workflow's default
    // policy stops the run.
    let engine = engine(json!([{
        "id": "w",
        "tasks": [{"id": "f", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.s", "logic": "x", "mode": "append"}]}}}]
    }]));
    let result = run(
        &engine,
        JsValue::NULL,
        Some(r#"{"data": {"s": "str"}, "trace": true}"#),
    )
    .await;
    assert_eq!(result["run"]["status"], "stopped");
    assert!(
        result["run"]["error"]["code"].is_string(),
        "{}",
        result["run"]
    );
    assert_eq!(result["run"]["partial"], true);
    assert_eq!(result["run"]["workflows"][0]["exit"], "stopped");
    assert!(!result["message"]["errors"].as_array().unwrap().is_empty());

    let steps = result["trace"]["steps"]
        .as_array()
        .expect("a trace on a stopped run");
    let failed = steps
        .iter()
        .find(|s| s["result"] == "failed")
        .expect("the failed step");
    assert_eq!(failed["task_id"], "f");
    assert!(!failed["errors"].as_array().unwrap().is_empty());
}

#[wasm_bindgen_test]
async fn a_deadline_ends_a_loop_timed_out_with_a_partial_result() {
    let engine = engine(json!([{
        "id": "loop",
        "loop": {"counter": "i", "max": 100000},
        "tasks": [{"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.n", "logic": {"var": "temp_data.i"}}]}}}]
    }]));
    let result = run(
        &engine,
        JsValue::NULL,
        Some(r#"{"timeout_ms": 0, "trace": true}"#),
    )
    .await;
    assert_eq!(result["run"]["status"], "timed_out");
    assert_eq!(result["run"]["retryable"], true);
    assert_eq!(result["run"]["partial"], true);
    assert!(result["trace"]["steps"].is_array());
}

// ---------------------------------------------------------------------------
// The trace and its replay
// ---------------------------------------------------------------------------

fn traced_engine() -> WasmEngine {
    engine(json!([
        {
            "id": "first",
            "tasks": [{"id": "m", "function": {"name": "map", "input": {"mappings": [
                {"path": "data.a", "logic": 1},
                {"path": "data.skipped", "logic": null},
                {"path": "data.list", "logic": "x", "mode": "append"},
                {"path": "data", "logic": {"b": 2, "c": 3}}
            ]}}}]
        },
        {
            "id": "second",
            "tasks": [{"id": "v", "function": {"name": "validation", "input": {"rules": [
                {"logic": {"==": [{"var": "data.a"}, 2]}, "path": "data.a", "message": "a must be 2"}
            ]}}}]
        }
    ]))
}

#[wasm_bindgen_test]
async fn a_trace_replays_to_the_messages_context_and_timings_read_the_wasm_clock() {
    let engine = traced_engine();
    let result = run(
        &engine,
        JsValue::NULL,
        Some(r#"{"trace": {"timings": true, "redact": ["metadata"]}}"#),
    )
    .await;
    let trace = &result["trace"];
    assert_eq!(trace["initial"]["metadata"], Value::Null, "redacted");
    let steps = trace["steps"].as_array().unwrap();
    assert!(steps.iter().any(|s| s["elapsed_us"].is_number()), "{trace}");
    assert_eq!(steps[0]["mapping_ends"], json!([1, 1, 3, 4]));
    // The validation's rule failed: its record is on its step.
    let v = steps.iter().find(|s| s["task_id"] == "v").unwrap();
    assert_eq!(v["status"], 400);
    assert_eq!(v["errors"][0]["message"], "a must be 2");

    let replayed = json_of(&contexts(js(&trace.to_string())).unwrap());
    let replayed = replayed.as_array().unwrap();
    assert_eq!(replayed.len(), steps.len());
    let last = replayed.last().unwrap();
    assert_eq!(last["data"], result["message"]["context"]["data"]);
    assert_eq!(last["metadata"], Value::Null, "redacted all the way");
}

#[wasm_bindgen_test]
async fn mapping_contexts_give_the_context_after_each_mapping() {
    let engine = traced_engine();
    let result = run(&engine, JsValue::NULL, Some(r#"{"trace": true}"#)).await;
    let trace = js(&result["trace"].to_string());
    let after = json_of(&mapping_contexts(trace.clone(), 0.0).unwrap());
    let after = after.as_array().unwrap();
    assert_eq!(after.len(), 4);
    assert_eq!(after[0]["data"]["a"], 1);
    assert!(after[1]["data"].get("skipped").is_none());
    assert_eq!(after[2]["data"]["list"], json!(["x"]));
    assert_eq!(after[3]["data"]["b"], 2);
    assert_eq!(after[3]["data"]["c"], 3);
    // Not a `map` step: no mapping contexts.
    let steps = result["trace"]["steps"].as_array().unwrap();
    let v = steps.iter().position(|s| s["task_id"] == "v").unwrap();
    assert_eq!(
        json_of(&mapping_contexts(trace.clone(), v as f64).unwrap()),
        json!([])
    );
    // Past the end, and a number that is not a step: each refused, none
    // wrapped by the glue.
    for bad in [steps.len() as f64, -1.0, 1.5, f64::NAN, 4294967296.0] {
        let (code, message, _) = error_parts(&mapping_contexts(trace.clone(), bad).unwrap_err());
        assert_eq!(code, "INVALID_ARGUMENT", "{bad}");
        assert!(message.starts_with("step: "), "{message}");
    }
}

#[wasm_bindgen_test]
async fn a_malformed_trace_is_refused_before_it_is_indexed() {
    let engine = traced_engine();
    let result = run(&engine, JsValue::NULL, Some(r#"{"trace": true}"#)).await;
    let mut trace = result["trace"].clone();
    trace["steps"][0]["mapping_ends"] = json!([1, 99]);
    let (code, message, _) = error_parts(&contexts(js(&trace.to_string())).unwrap_err());
    assert_eq!(code, "INVALID_TRACE");
    assert!(message.contains("mapping_ends"), "{message}");

    let (code, _, _) = error_parts(&contexts(js(r#"{"steps": "no"}"#)).unwrap_err());
    assert_eq!(code, "INVALID_ARGUMENT");

    // Ends that go backwards are refused too.
    let mut backwards = result["trace"].clone();
    backwards["steps"][0]["mapping_ends"] = json!([3, 1]);
    let (code, _, _) = error_parts(&contexts(js(&backwards.to_string())).unwrap_err());
    assert_eq!(code, "INVALID_TRACE");

    // A change at an index past the engine's cap cannot grow an array: the
    // path names a key there, which writes nothing on an array, so the replay
    // neither allocates nor panics and the context is unchanged.
    let mut hostile = result["trace"].clone();
    hostile["steps"][0]["changes"] = json!([
        {"path": "data.list.4294967295", "old_value": null, "new_value": 7},
        {"path": "data.list.3000000000", "old_value": null, "new_value": 7}
    ]);
    hostile["steps"][0]["mapping_ends"] = json!([]);
    hostile["initial"]["data"] = json!({"list": []});
    let replayed = json_of(&contexts(js(&hostile.to_string())).unwrap());
    assert_eq!(replayed[0]["data"], json!({"list": []}));
}

#[wasm_bindgen_test]
async fn an_unredacted_trace_replays_to_the_whole_context() {
    let engine = traced_engine();
    let result = run(&engine, JsValue::NULL, Some(r#"{"trace": true}"#)).await;
    let replayed = json_of(&contexts(js(&result["trace"].to_string())).unwrap());
    let replayed = replayed.as_array().unwrap();
    assert_eq!(
        replayed.len(),
        result["trace"]["steps"].as_array().unwrap().len()
    );
    assert_eq!(replayed.last().unwrap(), &result["message"]["context"]);
}

#[wasm_bindgen_test]
async fn the_trace_option_takes_false_the_changes_switch_and_a_byte_budget() {
    let engine = traced_engine();
    let off = run(&engine, JsValue::NULL, Some(r#"{"trace": false}"#)).await;
    assert!(off.get("trace").is_none());

    let profile = run(
        &engine,
        JsValue::NULL,
        Some(r#"{"trace": {"changes": false}}"#),
    )
    .await;
    let steps = profile["trace"]["steps"].as_array().unwrap();
    assert!(
        steps
            .iter()
            .all(|s| s["changes"] == json!([]) && s.get("mapping_ends").is_none())
    );

    let capped = run(
        &engine,
        JsValue::NULL,
        Some(r#"{"trace": {"max_bytes": 1}}"#),
    )
    .await;
    assert_eq!(capped["trace"]["truncated"], true);

    // A mistake inside the options names the key, and a bad shape says what
    // the option takes.
    for (options, needle) in [
        (r#"{"trace": {"bogus": 1}}"#, "options.trace"),
        (r#"{"trace": "yes"}"#, "must be true, false or an object"),
        (r#"{"audit": {"last": -1}}"#, "options.audit"),
        (
            r#"{"audit": 5}"#,
            "must be \"full\", \"off\" or { last: n }",
        ),
    ] {
        let promise = engine.run(JsValue::NULL, Some(obj(options)));
        let (code, message, _) = error_parts(&JsFuture::from(promise).await.unwrap_err());
        assert_eq!(code, "INVALID_ARGUMENT", "{options}");
        assert!(message.contains(needle), "{options}: {message}");
    }
}

#[wasm_bindgen_test]
async fn a_payload_that_is_not_json_is_refused_by_name() {
    let engine = engine(json!([mapping_workflow("w", "data.x", json!(1))]));
    let function = js_sys::Function::new_no_args("return 1");
    let promise = engine.run(function.into(), None);
    let (code, message, _) = error_parts(&JsFuture::from(promise).await.unwrap_err());
    assert_eq!(code, "INVALID_ARGUMENT");
    assert!(message.starts_with("payload: "), "{message}");
}

// ---------------------------------------------------------------------------
// The engine's facts and the definition tools
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
fn engine_facts_name_the_built_ins_and_the_defaults() {
    let facts = json_of(&engine_facts().unwrap());
    assert_eq!(facts["version"], engine_version());
    let names = facts["builtin_functions"].as_array().unwrap();
    assert_eq!(names.len(), 8);
    assert!(names.contains(&json!("validation")) && names.contains(&json!("map")));
    assert_eq!(facts["max_group_depth"], 8);
    assert_eq!(facts["loop_defaults"], json!({"init": 0, "increment": 1}));
    assert_eq!(facts["for_each_defaults"], json!({"max_concurrency": 1}));
    let keys = &facts["definition_keys"];
    assert_eq!(keys["rollout"], json!(["bucket_start", "bucket_end"]));
    assert!(
        keys["task"]
            .as_array()
            .unwrap()
            .contains(&json!("for_each"))
    );
    assert!(
        keys["workflow"]
            .as_array()
            .unwrap()
            .contains(&json!("loop"))
    );
}

#[wasm_bindgen_test]
fn parse_rewrites_a_v3_spelling_and_reports_it() {
    let parsed = json_of(
        &parse(js(
            r#"{"id": "w", "tasks": [{"id": "t", "continue_on_error": true,
                "function": {"name": "validate", "input": {"rules": []}}}]}"#,
        ))
        .unwrap(),
    );
    assert_eq!(parsed["definition"]["tasks"][0]["on_failure"], "continue");
    assert_eq!(
        parsed["definition"]["tasks"][0]["function"]["name"],
        "validation"
    );
    let codes: Vec<&str> = parsed["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"V3_SPELLING"), "{codes:?}");

    let rejected = json_of(&parse(js(r#"{"tasks": []}"#)).unwrap());
    assert_eq!(rejected["definition"], Value::Null);
    assert!(!rejected["issues"].as_array().unwrap().is_empty());

    let (code, _, _) = error_parts(&parse(js("[1]")).unwrap_err());
    assert_eq!(code, "INVALID_ARGUMENT");
}

#[wasm_bindgen_test]
fn check_reports_an_unknown_function_at_its_path() {
    let engine = engine(json!([mapping_workflow("w", "data.x", json!(1))]));
    let issues = json_of(
        &engine
            .check(js(
                r#"{"id": "n", "tasks": [{"id": "t", "function": {"name": "nope", "input": {}}}]}"#,
            ))
            .unwrap(),
    );
    let issues = issues.as_array().unwrap();
    assert_eq!(issues[0]["code"], "UNKNOWN_FUNCTION");
    assert_eq!(issues[0]["path"], "tasks[0].function.name");
    let clean = json_of(
        &engine
            .check(js(&mapping_workflow("ok", "data.y", json!(2)).to_string()))
            .unwrap(),
    );
    assert_eq!(clean, json!([]));
}

// ---------------------------------------------------------------------------
// Limits on wasm: nothing races, so the checks are the whole limit
//
// The wasm package registers no host functions and exposes no token, so
// these use the engine directly. Natively a host call is raced against the
// deadline and cut short mid-flight; here the call runs to its end and the
// checks around it see the deadline afterwards.
// ---------------------------------------------------------------------------

/// A host function that records that it ran and returns `Ok`.
struct Once(Arc<AtomicBool>);

impl dataflow_rs::Function for Once {
    type Config = ();

    async fn call(
        &self,
        _ctx: &mut dataflow_rs::Ctx<'_>,
        _cfg: &(),
    ) -> dataflow_rs::Result<dataflow_rs::Outcome> {
        self.0.store(true, Ordering::SeqCst);
        Ok(dataflow_rs::Outcome::Ok)
    }
}

/// A host function that outlasts a deadline, then fails.
struct Slow(Arc<AtomicBool>);

impl dataflow_rs::Function for Slow {
    type Config = ();

    async fn call(
        &self,
        _ctx: &mut dataflow_rs::Ctx<'_>,
        _cfg: &(),
    ) -> dataflow_rs::Result<dataflow_rs::Outcome> {
        // A call that outlasts the deadline without yielding: on wasm
        // nothing races it, so it runs to its end and the check after it
        // sees the deadline and overrides its `Err`.
        let start = dataflow_rs::run::Instant::now();
        while start.elapsed() < Duration::from_millis(200) {
            std::hint::spin_loop();
        }
        self.0.store(true, Ordering::SeqCst);
        Err(dataflow_rs::Error::service("BOOM", "the slow call failed"))
    }
}

/// An engine whose one task calls `name`.
fn host_engine<F: dataflow_rs::Function + 'static>(name: &str, f: F) -> dataflow_rs::Engine {
    dataflow_rs::Engine::builder()
        .register(name, f)
        .workflow_json(
            &json!({"id": "w", "tasks": [{"id": "t", "on_failure": "continue",
            "function": {"name": name, "input": {}}}]}),
        )
        .build()
        .expect("the workflow compiles")
}

#[wasm_bindgen_test]
async fn a_non_zero_deadline_ends_a_loop_timed_out_mid_run() {
    // The same loop as `timeout_ms: 0`, with a deadline that passes while it
    // runs: at least one sweep completes before the check fires.
    let engine = engine(json!([{
        "id": "loop",
        "loop": {"counter": "i", "max": 100000},
        "tasks": [{"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.n", "logic": {"var": "temp_data.i"}}]}}}]
    }]));
    let result = run(&engine, JsValue::NULL, Some(r#"{"timeout_ms": 5}"#)).await;
    assert_eq!(result["run"]["status"], "timed_out");
    assert_eq!(result["run"]["partial"], true);
    let swept = result["message"]["context"]["data"]["n"]
        .as_u64()
        .expect("the loop wrote data.n before the deadline");
    assert!(swept >= 1, "the loop ran no sweep: {result}");
}

#[wasm_bindgen_test]
async fn a_cancelled_token_stops_the_run_before_the_call() {
    use dataflow_rs::run::{CancelToken, RunStatus};
    let ran = Arc::new(AtomicBool::new(false));
    let engine = host_engine("record", Once(Arc::clone(&ran)));
    let token = CancelToken::new();
    token.cancel();
    let opts = dataflow_rs::RunOptions::default().cancel(&token);
    let run = engine
        .run_with(&mut dataflow_rs::Message::from_json(&json!({})), &opts)
        .await;
    assert!(
        matches!(run.status(), RunStatus::Cancelled),
        "{:?}",
        run.status()
    );
    assert!(
        !ran.load(Ordering::SeqCst),
        "the token stopped the run before the call"
    );
}

#[wasm_bindgen_test]
async fn a_slow_host_call_is_not_cut_short_but_the_deadline_still_ends_the_run() {
    use dataflow_rs::run::{Instant, RunStatus};
    let finished = Arc::new(AtomicBool::new(false));
    let engine = host_engine("slow", Slow(Arc::clone(&finished)));
    let opts =
        dataflow_rs::RunOptions::default().deadline(Instant::now() + Duration::from_millis(50));
    let run = engine
        .run_with(&mut dataflow_rs::Message::from_json(&json!({})), &opts)
        .await;
    assert!(
        finished.load(Ordering::SeqCst),
        "nothing races on wasm: the call ran to its end"
    );
    assert!(
        matches!(run.status(), RunStatus::TimedOut),
        "{:?}",
        run.status()
    );
}
