//! `decide`: the one definition of failed, rejected and halted.
//!
//! A call's result becomes the status its audit entry and progress write
//! carry, the record `message.errors()` gains, and what the interpreter does
//! next. A row's flow is computed first; `terminal` and `Outcome::Halt` then
//! upgrade a `Next`, and only a `Next`, to `Halt`, so a terminal task that
//! fails still records and still fails (v3's `handle_task_result`).
//!
//! A task decides under its site's [`Policy`]; each element of a fan-out
//! under [`Policy::per_element`], which cannot halt, since `terminal` and
//! the halting policies apply once, to the whole fan-out.

use crate::base::{Error, Result};
use crate::def::{FailurePolicy, RejectionPolicy};
use crate::function::Outcome;
use crate::program::CallSite;
use std::borrow::Cow;

/// The status a `Halt` records, as v3's `HALT_STATUS_CODE`.
pub(crate) const HALT_STATUS: u16 = 299;

/// What the interpreter does after an op.
#[derive(Debug)]
pub(crate) enum Flow {
    /// The next op.
    Next,
    /// The op at this index; the ops' length is the end.
    Jump(u32),
    /// The workflow ends `Halted`.
    Halt,
    /// The failure goes to the workflow's `on_failure`.
    Fail(Error),
}

/// What one completed call records and does.
#[derive(Debug)]
pub(crate) struct Decision {
    /// The status the audit entry and the progress write carry; `None` for `Skip`.
    pub(crate) status: Option<u16>,
    /// What `message.errors()` gains: (code, message).
    pub(crate) record: Option<(Cow<'static, str>, String)>,
    pub(crate) flow: Flow,
}

/// What a task's outcome does: its three policies, as its call site holds
/// them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Policy {
    /// What a failure (a 5xx or an `Err`) does.
    pub(crate) on_failure: FailurePolicy,
    /// What a rejection (a 4xx) does.
    pub(crate) on_rejection: RejectionPolicy,
    /// Whether completing the task ends the workflow.
    pub(crate) terminal: bool,
}

impl CallSite {
    /// The policies this site's outcome is decided under.
    pub(crate) fn policy(&self) -> Policy {
        Policy {
            on_failure: self.on_failure,
            on_rejection: self.on_rejection,
            terminal: self.terminal,
        }
    }
}

impl Policy {
    /// What one element of a fan-out runs under: nothing that can halt.
    /// `terminal` is off, and `on_rejection: halt` and `on_failure: halt`
    /// become `continue`; `fail` and `continue` stay. The fold applies the
    /// task's own policy once, after the whole fan-out (v3's `fan_out_pass`).
    pub(crate) fn per_element(self) -> Self {
        Self {
            on_failure: match self.on_failure {
                FailurePolicy::Fail => FailurePolicy::Fail,
                FailurePolicy::Halt | FailurePolicy::Continue => FailurePolicy::Continue,
            },
            on_rejection: RejectionPolicy::Continue,
            terminal: false,
        }
    }
}

/// `on_failure`'s flow for a failure whose error is `error`.
fn on_failure(policy: FailurePolicy, error: impl FnOnce() -> Error) -> Flow {
    match policy {
        FailurePolicy::Fail => Flow::Fail(error()),
        FailurePolicy::Halt => Flow::Halt,
        FailurePolicy::Continue => Flow::Next,
    }
}

/// The only definition of failed, rejected and halted. Pure.
///
/// | Result | Status | Record | Flow before the upgrade |
/// | --- | --- | --- | --- |
/// | `Ok(Skip)` | none | none | `Next`, and no upgrade applies |
/// | `Ok(Ok)` | 200 | none | `Next` |
/// | `Ok(Halt)` | 299 | none | `Next`, then `Halt` by the upgrade |
/// | `Ok(Status(s))`, `s < 400` | `s` | none | `Next` |
/// | `Ok(Status(s))`, `400 ≤ s < 500` | `s` | none | `on_rejection` |
/// | `Ok(Status(s))`, `s ≥ 500` | `s` | `TASK_STATUS_ERROR` | `on_failure`, failing with `TASK_ERROR` |
/// | `Err(e)` | 500 | `e`'s code | `on_failure`, failing with `e` |
pub(crate) fn decide(policy: Policy, task_id: &str, result: Result<Outcome>) -> Decision {
    let (status, record, flow) = match result {
        Ok(Outcome::Skip) => {
            return Decision {
                status: None,
                record: None,
                flow: Flow::Next,
            };
        }
        Ok(Outcome::Ok) => (200, None, Flow::Next),
        Ok(Outcome::Halt) => (HALT_STATUS, None, Flow::Halt),
        Ok(Outcome::Status(s)) if s < 400 => (s, None, Flow::Next),
        Ok(Outcome::Status(s)) if s < 500 => {
            let flow = match policy.on_rejection {
                RejectionPolicy::Continue => Flow::Next,
                RejectionPolicy::Halt => Flow::Halt,
            };
            (s, None, flow)
        }
        Ok(Outcome::Status(s)) => {
            let record = (
                Cow::Borrowed("TASK_STATUS_ERROR"),
                format!("Task {task_id} returned status {s}"),
            );
            let flow = on_failure(policy.on_failure, || {
                Error::service(
                    "TASK_ERROR",
                    format!("Task {task_id} failed with status {s}"),
                )
            });
            (s, Some(record), flow)
        }
        Err(e) => {
            let record = (
                Cow::Owned(e.code().to_owned()),
                format!("Task {task_id} error: {}", e.record_text()),
            );
            (500, Some(record), on_failure(policy.on_failure, || e))
        }
    };
    // `terminal` upgrades a `Next` and nothing else: a failure still fails.
    let flow = match flow {
        Flow::Next if policy.terminal => Flow::Halt,
        other => other,
    };
    Decision {
        status: Some(status),
        record,
        flow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::ErrorKind;
    use crate::def::{ParseOptions, parse};
    use crate::function::Registry;
    use crate::program::{CompileOptions, CompiledWorkflow};
    use serde_json::json;

    /// A one-task workflow whose task carries the three policies.
    fn site(on_failure: FailurePolicy, on_rejection: RejectionPolicy, terminal: bool) -> Compiled {
        let json = json!({"id": "w", "tasks": [{
            "id": "t", "on_failure": on_failure.as_str(),
            "on_rejection": on_rejection.as_str(), "terminal": terminal,
            "function": {"name": "log", "input": {"message": "m"}}
        }]});
        let parsed = parse(&json, ParseOptions::default().compat(false));
        let def = parsed.def.unwrap_or_else(|| panic!("{:?}", parsed.issues));
        Compiled(
            Registry::default()
                .compiler(CompileOptions::default())
                .compile_one(&def)
                .unwrap()
                .workflow,
        )
    }

    struct Compiled(CompiledWorkflow);

    /// What a decision is, compared by its flow's shape and its failure's code.
    #[derive(Debug, PartialEq)]
    enum Shape {
        Next,
        Halt,
        Fail(String, String),
    }

    fn shape(flow: &Flow) -> Shape {
        match flow {
            Flow::Next => Shape::Next,
            Flow::Halt => Shape::Halt,
            Flow::Fail(e) => Shape::Fail(e.code().to_owned(), e.message().to_owned()),
            Flow::Jump(_) => panic!("decide never jumps"),
        }
    }

    /// One row of the table, as the brief writes it, for one policy set.
    fn expected(
        result: &str,
        on_failure: FailurePolicy,
        on_rejection: RejectionPolicy,
        terminal: bool,
    ) -> (Option<u16>, Option<(&'static str, String)>, Shape) {
        let failure = |error: Shape| match on_failure {
            FailurePolicy::Fail => error,
            FailurePolicy::Halt => Shape::Halt,
            FailurePolicy::Continue => Shape::Next,
        };
        let (status, record, flow, upgrade) = match result {
            "skip" => return (None, None, Shape::Next),
            "ok" => (Some(200), None, Shape::Next, terminal),
            "halt" => (Some(299), None, Shape::Next, true),
            "302" => (Some(302), None, Shape::Next, terminal),
            "404" => {
                let flow = match on_rejection {
                    RejectionPolicy::Continue => Shape::Next,
                    RejectionPolicy::Halt => Shape::Halt,
                };
                (Some(404), None, flow, terminal)
            }
            "503" => (
                Some(503),
                Some(("TASK_STATUS_ERROR", "Task t returned status 503".to_owned())),
                failure(Shape::Fail(
                    "TASK_ERROR".to_owned(),
                    "Task t failed with status 503".to_owned(),
                )),
                terminal,
            ),
            "err" => (
                Some(500),
                Some((
                    "LOGIC_ERROR",
                    "Task t error: Logic evaluation error: boom".to_owned(),
                )),
                failure(Shape::Fail("LOGIC_ERROR".to_owned(), "boom".to_owned())),
                terminal,
            ),
            other => unreachable!("{other}"),
        };
        let flow = if upgrade && flow == Shape::Next {
            Shape::Halt
        } else {
            flow
        };
        (status, record, flow)
    }

    fn result(row: &str) -> Result<Outcome> {
        match row {
            "skip" => Ok(Outcome::Skip),
            "ok" => Ok(Outcome::Ok),
            "halt" => Ok(Outcome::Halt),
            "302" => Ok(Outcome::Status(302)),
            "404" => Ok(Outcome::Status(404)),
            "503" => Ok(Outcome::Status(503)),
            "err" => Err(Error::new(ErrorKind::Eval, "boom")),
            other => unreachable!("{other}"),
        }
    }

    #[test]
    fn decide_matches_the_table() {
        let failures = [
            FailurePolicy::Fail,
            FailurePolicy::Halt,
            FailurePolicy::Continue,
        ];
        let rejections = [RejectionPolicy::Continue, RejectionPolicy::Halt];
        let mut rows = 0;
        for on_failure in failures {
            for on_rejection in rejections {
                for terminal in [false, true] {
                    let wf = site(on_failure, on_rejection, terminal);
                    let site = &wf.0.calls[0];
                    assert_eq!(
                        (site.on_failure, site.on_rejection, site.terminal),
                        (on_failure, on_rejection, terminal)
                    );
                    for row in ["skip", "ok", "halt", "302", "404", "503", "err"] {
                        let d = decide(site.policy(), "t", result(row));
                        let got = (
                            d.status,
                            d.record.map(|(code, message)| (code.into_owned(), message)),
                            shape(&d.flow),
                        );
                        let (status, record, flow) =
                            expected(row, on_failure, on_rejection, terminal);
                        let want = (
                            status,
                            record.map(|(code, message)| (code.to_owned(), message)),
                            flow,
                        );
                        assert_eq!(
                            got, want,
                            "{row} under {on_failure:?}, {on_rejection:?}, terminal {terminal}"
                        );
                        rows += 1;
                    }
                }
            }
        }
        assert_eq!(rows, 84);
    }

    #[test]
    fn per_element_neutralises_what_can_halt() {
        // Each element of a fan-out runs under a policy that cannot halt:
        // `terminal`, `on_rejection: halt` and `on_failure: halt` apply once,
        // to the whole fan-out, at the fold. `fail` stays, so a failed
        // element still fails the task.
        let mut combinations = 0;
        for on_failure in [
            FailurePolicy::Fail,
            FailurePolicy::Halt,
            FailurePolicy::Continue,
        ] {
            for on_rejection in [RejectionPolicy::Continue, RejectionPolicy::Halt] {
                for terminal in [false, true] {
                    let policy = Policy {
                        on_failure,
                        on_rejection,
                        terminal,
                    };
                    let element = policy.per_element();
                    let kept = match on_failure {
                        FailurePolicy::Fail => FailurePolicy::Fail,
                        FailurePolicy::Halt | FailurePolicy::Continue => FailurePolicy::Continue,
                    };
                    let at = format!("{on_failure:?}, {on_rejection:?}, terminal {terminal}");
                    assert_eq!(element.on_failure, kept, "{at}");
                    assert_eq!(element.on_rejection, RejectionPolicy::Continue, "{at}");
                    assert!(!element.terminal, "{at}");
                    for row in ["skip", "ok", "halt", "302", "404", "503", "err"] {
                        // Only a `Halt` halts an element, and only `fail`
                        // fails one; the status and the record are the
                        // site's own.
                        let d = decide(element, "t", result(row));
                        let want = match row {
                            "halt" => Shape::Halt,
                            "503" | "err" if kept == FailurePolicy::Fail => {
                                expected(row, on_failure, on_rejection, false).2
                            }
                            _ => Shape::Next,
                        };
                        assert_eq!(shape(&d.flow), want, "{row} under {at}");
                        let site = decide(policy, "t", result(row));
                        assert_eq!(
                            (d.status, d.record),
                            (site.status, site.record),
                            "{row} under {at}"
                        );
                    }
                    combinations += 1;
                }
            }
        }
        assert_eq!(combinations, 12);
    }

    /// One run of a message over two workflows on v3 and on v4: the
    /// message's wire text, normalised, and the outcome's code.
    mod against_v3 {
        use crate::run::{Engine, Message, RunStatus};
        use dataflow_corpus::json::Json;
        use dataflow_corpus::normalise;
        use datavalue::OwnedDataValue;
        use serde_json::{Value, json};

        /// The four v3 flag combinations of 06's policy table:
        /// `continue_on_error: true`, and `halt_on: "failure"`.
        pub(super) const FLAGS: [(&str, bool, bool); 4] = [
            ("neither", false, false),
            ("continue_on_error", true, false),
            ("halt_on", false, true),
            ("both", true, true),
        ];

        /// The task under test, then a task that marks that the workflow went
        /// on, then a second workflow that marks that the run went on. The
        /// task is `validation` (400), `map` (200, or 500 over a string at
        /// `data.s`) or `parse_xml` (`Err` over invalid XML).
        pub(super) fn workflows(flags: usize, function: &str) -> Vec<Value> {
            let input = match function {
                "validation" => json!({"rules": [
                    {"logic": {"==": [{"var": "data.ok"}, true]}, "message": "not ok"}]}),
                "map" => json!({"mappings": [
                    {"path": "data.s", "logic": "x", "mode": "append"}]}),
                "parse_xml" => json!({"source": "data.xml", "target": "data.parsed"}),
                other => unreachable!("{other}"),
            };
            let mut task = json!({"id": "t", "name": "t",
                                  "function": {"name": function, "input": input}});
            let (_, continue_on_error, halt_on) = FLAGS[flags];
            if continue_on_error {
                task["continue_on_error"] = json!(true);
            }
            if halt_on {
                task["halt_on"] = json!("failure");
            }
            let mark = |id: &str| {
                json!({"id": id, "name": id, "function": {"name": "map", "input": {
                    "mappings": [{"path": format!("data.{id}"), "logic": true}]}}})
            };
            vec![
                json!({"id": "w", "name": "w", "priority": 0, "tasks": [task, mark("after")]}),
                json!({"id": "w2", "name": "w2", "priority": 1, "tasks": [mark("next")]}),
            ]
        }

        /// v3's wire text with `WORKFLOW_ERROR` records removed (break 4:
        /// v4 records no wrapper for a workflow that stops), normalised.
        fn normalised(text: &str, drop_workflow_errors: bool) -> String {
            let mut json = Json::from_text(text).expect("a message parses");
            if drop_workflow_errors && let Some(Json::Array(errors)) = json.get_mut("errors") {
                errors.retain(|e| e.get("code") != Some(&Json::str("WORKFLOW_ERROR")));
            }
            normalise::message(&mut json, None);
            json.to_text()
        }

        pub(super) async fn v3(workflows: &[Value], data: &Value) -> (String, String) {
            let workflows: Vec<_> = workflows
                .iter()
                .map(|w| dataflow_rs_v3::Workflow::from_json(&w.to_string()).expect("v3 parses"))
                .collect();
            let engine = dataflow_rs_v3::Engine::builder()
                .with_workflows(workflows)
                .build()
                .expect("v3 builds");
            let mut message = dataflow_rs_v3::Message::builder()
                .id("m")
                .data_json(data)
                .build();
            let result = engine.process_message(&mut message).await;
            let code = match result {
                Ok(()) => "ok".to_owned(),
                Err(e) => dataflow_rs_v3::ErrorInfo::new(None, None, e).code,
            };
            let text = serde_json::to_string(&message).expect("a message serializes");
            (normalised(&text, true), code)
        }

        pub(super) async fn v4(workflows: &[Value], data: &Value) -> (String, String) {
            let engine = workflows
                .iter()
                .fold(Engine::builder(), |b, w| b.workflow_json(w))
                .build()
                .expect("v4 builds");
            let mut message = Message::from_parts(
                OwnedDataValue::Null,
                OwnedDataValue::from(data),
                OwnedDataValue::Null,
            )
            .with_id("m");
            let run = engine.run(&mut message).await;
            let code = match run.status() {
                RunStatus::Stopped(e) => e.code().to_owned(),
                _ => "ok".to_owned(),
            };
            let text = serde_json::to_string(&message).expect("a message serializes");
            (normalised(&text, false), code)
        }
    }

    #[tokio::test]
    async fn decide_matches_v3s_flags() {
        // Each function over each message: `data.ok` passes the validation,
        // `data.s` an array lets the append succeed, `data.xml` valid XML
        // parses. 200, 400, 500 and `Err`.
        let cases = [
            ("validation", json!({"ok": true})),
            ("validation", json!({"ok": false})),
            ("map", json!({"s": []})),
            ("map", json!({"s": "str"})),
            ("parse_xml", json!({"xml": "<a>1</a>"})),
            ("parse_xml", json!({"xml": "<a>"})),
        ];
        for (i, (name, _, _)) in against_v3::FLAGS.iter().enumerate() {
            for (function, data) in &cases {
                let workflows = against_v3::workflows(i, function);
                let v3 = against_v3::v3(&workflows, data).await;
                let v4 = against_v3::v4(&workflows, data).await;
                assert_eq!(v4, v3, "{function} over {data} with {name}");
            }
        }
    }
}
