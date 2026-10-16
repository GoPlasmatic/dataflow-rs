// The engine against v3 over workflows without loops: groups, nested and
// terminal groups, task, group and workflow conditions, every built-in, every
// policy combination, and workflows chained through `metadata.progress`. Each
// set of workflows is built once on each side and run over several messages;
// what is compared is each message's wire text and the outcome's code, as
// `support/vs_v3.rs` normalises them (break 4's `WORKFLOW_ERROR` dropped).
//
// Only Active workflows appear without a channel: v3 also runs Paused ones
// there, which break 7 removes.

#[path = "support/vs_v3.rs"]
mod vs_v3;

use serde_json::{Value, json};
use vs_v3::{Run, data, on_v3, on_v4, v3_engine, v4_engine};

/// A `map` task appending `item` at `data.order`, so the order tasks ran in
/// is in the message.
fn mark(id: &str) -> Value {
    json!({"id": id, "function": {"name": "map", "input": {
        "mappings": [{"path": "data.order", "logic": id, "mode": "append"}]}}})
}

fn with(mut node: Value, extra: &[(&str, Value)]) -> Value {
    for (key, value) in extra {
        node[*key] = value.clone();
    }
    node
}

fn workflow(id: &str, priority: u32, extra: &[(&str, Value)], tasks: Vec<Value>) -> Value {
    with(
        json!({"id": id, "priority": priority, "tasks": tasks}),
        extra,
    )
}

/// `validation`: 400 unless `data.ok` is `true`.
fn requires_ok(id: &str) -> Value {
    json!({"id": id, "function": {"name": "validation", "input": {"rules": [
        {"logic": {"==": [{"var": "data.ok"}, true]}, "message": "not ok"}]}}})
}

/// `map`: 500 over a string at `data.s`, 200 over an array.
fn appends(id: &str) -> Value {
    json!({"id": id, "function": {"name": "map", "input": {
        "mappings": [{"path": "data.s", "logic": id, "mode": "append"}]}}})
}

/// `parse_xml`: `Err` unless `data.xml` is a document.
fn parses_xml(id: &str) -> Value {
    json!({"id": id, "function": {"name": "parse_xml", "input": {
        "source": "data.xml", "target": "doc"}}})
}

/// Every combination of the three failures being on or off.
fn failure_grid() -> Vec<Run> {
    let mut runs = Vec::new();
    for bits in 0..8 {
        runs.push(data(json!({
            "ok": bits & 1 == 0,
            "s": if bits & 2 == 0 { json!([]) } else { json!("str") },
            "xml": if bits & 4 == 0 { "<a>1</a>" } else { "<a>" },
        })));
    }
    runs
}

/// Groups: nested, terminal, conditional and not, and task conditions.
fn groups() -> (Vec<Value>, Vec<Run>) {
    let cond = |key: &str| json!({"var": format!("data.{key}")});
    let workflows = vec![
        workflow(
            "w",
            0,
            &[],
            vec![
                json!({"id": "A", "condition": cond("a"), "tasks": [
                    mark("t1"),
                    {"id": "B", "condition": cond("b"), "terminal": true, "tasks": [
                        mark("t2"),
                        with(mark("t3"), &[("condition", cond("t3"))]),
                    ]},
                    mark("t4"),
                ]}),
                with(mark("t5"), &[("condition", cond("t5"))]),
                json!({"id": "C", "condition": cond("c"), "terminal": true, "tasks": [
                    {"id": "D", "condition": cond("d"), "tasks": [mark("t6")]},
                ]}),
                mark("t7"),
                // A group whose condition fails to evaluate is false.
                json!({"id": "E", "condition": {"/": [1, {"var": "data.zero"}]}, "tasks": [
                    mark("t8")]}),
                mark("t9"),
                // So is a task's.
                with(
                    mark("t10"),
                    &[("condition", json!({"/": [1, {"var": "data.zero"}]}))],
                ),
                mark("t11"),
            ],
        ),
        workflow("w2", 1, &[], vec![mark("u1")]),
    ];
    let mut runs = Vec::new();
    for bits in 0..64u32 {
        let flag = |i: u32| bits & (1 << i) != 0;
        runs.push(data(json!({
            "a": flag(0), "b": flag(1), "t3": flag(2), "t5": flag(3), "c": flag(4),
            "d": flag(5), "zero": 0,
        })));
    }
    // Truthy is not `true`.
    runs.push(data(json!({"a": 1, "b": "yes", "c": [1], "zero": 0})));
    (workflows, runs)
}

/// Every built-in, in one workflow, over messages that make each succeed
/// and fail.
fn builtins() -> (Vec<Value>, Vec<Run>) {
    let task = |id: &str, name: &str, input: Value| json!({"id": id, "continue_on_error": true, "function": {"name": name, "input": input}});
    let workflows = vec![workflow(
        "w",
        0,
        &[],
        vec![
            task(
                "parse",
                "parse_json",
                json!({"source": "payload", "target": "input"}),
            ),
            task(
                "parse_inner",
                "parse_json",
                json!({"source": "payload.inner", "target": "inner"}),
            ),
            task(
                "xml",
                "parse_xml",
                json!({"source": "data.xml", "target": "doc"}),
            ),
            task(
                "map",
                "map",
                json!({"mappings": [
                    {"path": "data.total", "logic": {"+": [{"var": "data.input.a"}, {"var": "data.input.b"}]}},
                    {"path": "temp_data.seen", "logic": {"var": "data.input"}},
                    {"path": "metadata.flag", "logic": true},
                    {"path": "data.input.b", "unset": true},
                    {"path": "data.list", "logic": {"var": "data.total"}, "mode": "append"},
                    {"path": "data.maybe", "logic": {"if": [{"var": "data.input.a"}, "set", null]}},
                ]}),
            ),
            task(
                "validate",
                "validation",
                json!({"rules": [
                    {"logic": {">": [{"var": "data.total"}, 0]}, "message": "total must be positive"},
                    {"logic": {"==": [{"var": "data.input.kind"}, "order"]},
                     "message": {"cat": ["bad kind: ", {"var": "data.input.kind"}]}},
                    {"logic": {"/": [1, {"var": "data.zero"}]}, "message": "never"},
                ]}),
            ),
            task(
                "log",
                "log",
                json!({"level": "info", "message": {"cat": ["total ", {"var": "data.total"}]}}),
            ),
            task(
                "gate",
                "filter",
                json!({"condition": {">": [{"var": "data.total"}, 1]}, "on_reject": "skip"}),
            ),
            task(
                "json",
                "publish_json",
                json!({"source": "input", "target": "out_json"}),
            ),
            task(
                "pretty",
                "publish_json",
                json!({"source": "data.input", "target": "out_pretty", "pretty": true}),
            ),
            task(
                "missing",
                "publish_json",
                json!({"source": "absent", "target": "out_missing"}),
            ),
            task(
                "xml_out",
                "publish_xml",
                json!({"source": "input", "target": "out_xml", "root_element": "doc"}),
            ),
            task(
                "stop",
                "filter",
                json!({"condition": {"var": "data.go_on"}}),
            ),
            mark("after_stop"),
        ],
    )];
    let run = |payload: Value, data: Value| Run {
        payload,
        data,
        ..Run::default()
    };
    let runs = vec![
        run(
            json!({"a": 1, "b": 2, "kind": "order", "inner": {"x": [1, 2]}}),
            json!({"xml": "<a><b>1</b></a>", "zero": 0, "go_on": true}),
        ),
        run(
            json!({"a": 0, "b": 0, "kind": "refund"}),
            json!({"xml": "<a>", "zero": 0}),
        ),
        run(
            json!("{\"a\": 2, \"b\": 3}"),
            json!({"xml": 5, "list": "not a list", "zero": 1}),
        ),
        run(Value::Null, json!({})),
    ];
    // A seeded progress: v3 keeps the key order of one with exactly its three
    // keys and replaces any other. `parse_json` runs first and reads no view,
    // so the first progress write is an owned one.
    let mut runs = runs;
    for progress in [
        json!({"workflow_id": "x", "task_id": "y", "status_code": 1}),
        json!({"task_id": "y", "status_code": 1, "workflow_id": "x", "extra": 0}),
        json!({"task_id": "y", "status_code": 1, "other": "x"}),
        json!(["not", "an", "object"]),
    ] {
        runs.push(Run {
            payload: json!({"a": 1, "b": 2}),
            metadata: json!({"progress": progress}),
            ..Run::default()
        });
    }
    (workflows, runs)
}

/// Every policy combination: the four v3 flag combinations on the tasks,
/// each with and without `terminal`, under both workflow policies.
fn policies() -> Vec<(Vec<Value>, Vec<Run>)> {
    let mut sets = Vec::new();
    for continue_on_error in [false, true] {
        for halt_on in [false, true] {
            for terminal in [false, true] {
                for workflow_continues in [false, true] {
                    let flags = |mut task: Value| {
                        if continue_on_error {
                            task["continue_on_error"] = json!(true);
                        }
                        if halt_on {
                            task["halt_on"] = json!("failure");
                        }
                        task
                    };
                    let mut middle = flags(appends("m"));
                    if terminal {
                        middle["terminal"] = json!(true);
                    }
                    let workflows = vec![
                        workflow(
                            "w",
                            0,
                            &[("continue_on_error", json!(workflow_continues))],
                            vec![
                                flags(requires_ok("v")),
                                mark("after_v"),
                                middle,
                                mark("after_m"),
                                flags(parses_xml("x")),
                                mark("after_x"),
                            ],
                        ),
                        workflow("w2", 1, &[], vec![mark("next")]),
                    ];
                    sets.push((workflows, failure_grid()));
                }
            }
        }
    }
    sets
}

/// Workflows chained through `metadata.progress`, with workflow conditions
/// that pass, fail, error, and read the previous workflow's last task.
fn chained() -> (Vec<Value>, Vec<Run>) {
    let progress = |key: &str| json!({"var": format!("metadata.progress.{key}")});
    let workflows = vec![
        workflow(
            "first",
            0,
            &[("continue_on_error", json!(true))],
            vec![
                mark("f1"),
                with(
                    requires_ok("check"),
                    &[("condition", json!({"var": "data.check"}))],
                ),
                with(
                    appends("risky"),
                    &[("condition", json!({"var": "data.risky"}))],
                ),
            ],
        ),
        workflow(
            "after_ok",
            1,
            &[(
                "condition",
                json!({"and": [
                    {"==": [progress("workflow_id"), "first"]},
                    {"==": [progress("status_code"), 200]},
                ]}),
            )],
            vec![mark("ok1")],
        ),
        workflow(
            "after_reject",
            2,
            &[("condition", json!({"==": [progress("status_code"), 400]}))],
            vec![mark("rej1")],
        ),
        workflow(
            "after_failure",
            3,
            &[("condition", json!({"==": [progress("status_code"), 500]}))],
            vec![mark("fail1")],
        ),
        workflow(
            "broken",
            4,
            &[("condition", json!({"/": [1, {"var": "data.zero"}]}))],
            vec![mark("never")],
        ),
        workflow(
            "last",
            5,
            &[(
                "condition",
                json!({"in": [progress("task_id"), ["ok1", "rej1", "fail1"]]}),
            )],
            vec![mark("end")],
        ),
    ];
    let mut runs = Vec::new();
    for check in [false, true] {
        for risky in [false, true] {
            for ok in [false, true] {
                for s in [json!([]), json!("str")] {
                    runs.push(data(json!({
                        "check": check, "risky": risky, "ok": ok, "s": s, "zero": 0
                    })));
                }
            }
        }
    }
    // Seeded progress is not reset at run start (I63).
    runs.push(Run {
        data: json!({"zero": 0}),
        metadata: json!({"progress": {"workflow_id": "x", "task_id": "y", "status_code": 1}}),
        ..Run::default()
    });
    (workflows, runs)
}

/// Channels, rollouts and statuses, and an audit mode.
fn selection() -> (Vec<Value>, Vec<Run>) {
    let rollout = |start: u8, end: u8| json!({"bucket_start": start, "bucket_end": end});
    let workflows = vec![
        workflow(
            "o1",
            0,
            &[("channel", json!("orders")), ("rollout", rollout(0, 50))],
            vec![mark("o1")],
        ),
        workflow(
            "o2",
            1,
            &[("channel", json!("orders")), ("rollout", rollout(50, 100))],
            vec![mark("o2")],
        ),
        workflow(
            "paused",
            2,
            &[("channel", json!("orders")), ("status", json!("paused"))],
            vec![mark("paused")],
        ),
        workflow(
            "p1",
            3,
            &[("channel", json!("payments"))],
            vec![mark("p1"), mark("p2"), mark("p3")],
        ),
    ];
    let mut runs = Vec::new();
    for channel in [Some("orders"), Some("payments"), Some("nowhere")] {
        for bucket in [None, Some(0), Some(49), Some(50), Some(99)] {
            runs.push(Run {
                channel,
                bucket,
                ..Run::default()
            });
        }
    }
    for audit in [0, 1, 2, 5] {
        runs.push(Run {
            channel: Some("payments"),
            audit: Some(audit),
            ..Run::default()
        });
    }
    (workflows, runs)
}

#[tokio::test]
async fn linear_workflows_match_v3() {
    let mut sets = vec![groups(), builtins(), chained(), selection()];
    sets.extend(policies());
    let mut compared = 0;
    for (i, (workflows, runs)) in sets.into_iter().enumerate() {
        let v3 = v3_engine(&workflows).unwrap_or_else(|e| panic!("v3 builds set {i}: {e}"));
        let v4 = v4_engine(&workflows).unwrap_or_else(|e| panic!("v4 builds set {i}: {e}"));
        for run in &runs {
            let expected = on_v3(&v3, run).await;
            let (text, code, _) = on_v4(&v4, run).await;
            assert_eq!(
                (text, code),
                expected,
                "set {i}, run over payload {} data {} metadata {} (channel {:?}, bucket {:?}, \
                 audit {:?})",
                run.payload,
                run.data,
                run.metadata,
                run.channel,
                run.bucket,
                run.audit
            );
            compared += 1;
        }
    }
    assert_eq!(compared, 237, "every run is compared");
}

/// The progress write updates a progress object of exactly its three keys in
/// place, with no view and with one, as v3's `overwrite_progress_in_place`
/// does: the same wire text as v3 over every seeded shape, with ids that grow
/// and shrink. No-view writes come before the first view and after each large
/// owned write, which checkpoints mid-run, the last two in later workflows.
/// Each `snap` copies the progress the write before it left, which a later
/// write would otherwise overwrite.
///
/// `map`s shape the progress object before writes made with a view. In the
/// first workflow, shapes replaced by a fresh object. In the second, values
/// that are not the ids' strings or the status's integer, and a status that
/// changes: several values change, so the object is marked as a whole. The
/// third runs on one view to the end, and each of its writes changes one
/// value, marked at its key, so only those marks bring the owned tree up to
/// date: three keys in an order neither alphabetical (as `json!` seeds are)
/// nor fresh, a key removed and written again, and every value already the
/// write's.
#[tokio::test]
async fn progress_written_in_place_matches_v3() {
    let parse = |id: &str, source: &str, target: &str| {
        json!({"id": id, "function": {"name": "parse_json",
            "input": {"source": source, "target": target}}})
    };
    let snap = |id: &str| {
        json!({"id": id, "function": {"name": "map", "input": {"mappings": [
            {"path": format!("data.{id}"), "logic": {"var": "metadata.progress"}}]}}})
    };
    let shape = |id: &str, mappings: Value| json!({"id": id, "function": {"name": "map", "input": {"mappings": mappings}}});
    let at = |key: &str| format!("metadata.progress.{key}");
    let workflows = vec![
        workflow(
            "w",
            0,
            &[],
            vec![
                parse("p", "payload.small", "a"),
                parse("a_much_longer_task_id", "payload.small", "b"),
                parse("q", "payload.small", "c"),
                snap("snap1"),
                // With a view: shapes replaced by a fresh object. The
                // checkpoint `big` takes brings the owned tree up to date.
                shape("four_keys", json!([{"path": at("extra"), "logic": 0}])),
                snap("snap4"),
                shape(
                    "two_keys",
                    json!([{"path": at("workflow_id"), "unset": true}]),
                ),
                snap("snap5"),
                shape(
                    "not_an_object",
                    json!([{"path": "metadata.progress", "logic": "x"}]),
                ),
                snap("snap6"),
                parse("big", "payload.big", "big"),
                parse("after_big", "payload.small", "d"),
                snap("snap2"),
            ],
        ),
        workflow(
            "second_workflow",
            1,
            &[],
            vec![
                parse("bigger", "payload.bigger", "bigger"),
                snap("snap3"),
                shape(
                    "odd_values",
                    json!([
                        {"path": at("task_id"), "logic": 5},
                        {"path": at("workflow_id"), "logic": [1]},
                        {"path": at("status_code"), "logic": 200.5}
                    ]),
                ),
                snap("snap7"),
                requires_ok("rejects"),
                snap("snap8"),
            ],
        ),
        workflow(
            "shaped",
            2,
            &[],
            vec![
                parse("biggest", "payload.biggest", "biggest"),
                // workflow_id moved last, its value kept: task_id,
                // status_code, workflow_id.
                shape(
                    "reordered",
                    json!([
                        {"path": "temp_data.kept", "logic": {"var": at("workflow_id")}},
                        {"path": at("workflow_id"), "unset": true},
                        {"path": at("workflow_id"), "logic": {"var": "temp_data.kept"}}
                    ]),
                ),
                snap("snap9"),
                shape(
                    "readded",
                    json!([
                        {"path": at("task_id"), "unset": true},
                        {"path": at("task_id"), "logic": "back"}
                    ]),
                ),
                snap("snap10"),
                shape(
                    "settled",
                    json!([{"path": at("task_id"), "logic": "settled"}]),
                ),
                snap("snap11"),
            ],
        ),
    ];
    // Each large value is past what the view holds when it is written.
    let payload = json!({
        "small": {"x": 1},
        "big": (0..5_000).collect::<Vec<_>>(),
        "bigger": (0..20_000).collect::<Vec<_>>(),
        "biggest": (0..30_000).collect::<Vec<_>>(),
    });
    let mut runs = Vec::new();
    for progress in [
        None,
        Some(json!({"workflow_id": "x", "task_id": "y", "status_code": 1})),
        Some(json!({"status_code": 1, "task_id": "a_long_seeded_task", "workflow_id": "x"})),
        Some(json!({"task_id": 5, "workflow_id": null, "status_code": "s"})),
        Some(json!({"task_id": "y", "status_code": 1, "workflow_id": "x", "extra": 0})),
        Some(json!({"task_id": "y", "status_code": 1})),
        Some(json!({"task_id": "y", "status_code": 1, "other": "x"})),
        Some(json!("not an object")),
    ] {
        runs.push(Run {
            payload: payload.clone(),
            metadata: progress.map_or_else(|| json!({}), |p| json!({"progress": p})),
            ..Run::default()
        });
    }
    let v3 = v3_engine(&workflows).expect("v3 builds");
    let v4 = v4_engine(&workflows).expect("v4 builds");
    for run in &runs {
        let (text, code, report) = on_v4(&v4, run).await;
        assert_eq!(
            (text, code),
            on_v3(&v3, run).await,
            "seeded {}",
            run.metadata
        );
        assert_eq!(
            report.arena_stats().views,
            4,
            "each large write dropped the view"
        );
    }
}

/// An audit mode that keeps nothing builds no entry, and the message is v3's
/// under it, and v4's own `Full` message with an empty audit trail: every
/// built-in, its changes and errors, and chained workflows.
#[tokio::test]
async fn audit_modes_that_keep_nothing_match_v3() {
    let mut compared = 0;
    for (workflows, runs) in [builtins(), chained()] {
        let v3 = v3_engine(&workflows).expect("v3 builds");
        let v4 = v4_engine(&workflows).expect("v4 builds");
        for full in runs {
            let mut kept: Value =
                serde_json::from_str(&on_v4(&v4, &full).await.0).expect("wire text is JSON");
            kept["audit_trail"] = json!([]);
            let off = Run {
                audit_off: true,
                ..full.clone()
            };
            let last_0 = Run {
                audit: Some(0),
                ..full.clone()
            };
            for run in [off, last_0] {
                let (text, code, _) = on_v4(&v4, &run).await;
                let label = format!(
                    "payload {} data {} (off {}, audit {:?})",
                    run.payload, run.data, run.audit_off, run.audit
                );
                assert_eq!(
                    serde_json::from_str::<Value>(&text).expect("wire text is JSON"),
                    kept,
                    "{label}: only the audit trail differs from Full"
                );
                assert_eq!((text, code), on_v3(&v3, &run).await, "{label}");
                compared += 1;
            }
        }
    }
    assert_eq!(compared, 2 * (8 + 17), "every run is compared");
}
