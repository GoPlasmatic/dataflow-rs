//! `Workflow::validate_authored` — authoring-time checking of a definition,
//! before it reaches `Engine::build()`.
//!
//! The API's promise is a biconditional: it returns empty **iff** the JSON
//! parses and validates. Two things have to be tested, and only one of them is
//! obvious.
//!
//! The obvious one is the biconditional itself. The subtle one is that the
//! biconditional is guaranteed by the *last* stage — an actual parse — so the
//! structural walk that produces good multi-violation reporting could quietly
//! become a no-op and every biconditional test would still pass. So each broken
//! fixture also asserts its **specific** semantic code, which fails if the walk
//! stops doing its job and the catch-all takes over.

use dataflow_rs::{IssueCode, Workflow};
use serde_json::{Value, json};

fn codes(json: &Value) -> Vec<IssueCode> {
    Workflow::validate_authored(json)
        .iter()
        .map(|i| i.code)
        .collect()
}

fn loads(json: &Value) -> bool {
    Workflow::from_json(&json.to_string()).is_ok_and(|w| w.validate().is_ok())
}

fn task(id: &str) -> Value {
    json!({"id": id, "name": id, "function": {"name": "map", "input": {"mappings": []}}})
}

fn workflow(tasks: Value) -> Value {
    json!({"id": "w", "name": "w", "priority": 0, "tasks": tasks})
}

/// Every fixture: what it is, and the code it must produce.
fn broken_fixtures() -> Vec<(&'static str, IssueCode, Value)> {
    vec![
        (
            "empty workflow id",
            IssueCode::EmptyWorkflowId,
            json!({"id": "", "name": "w", "tasks": [task("t")]}),
        ),
        (
            "missing workflow name",
            IssueCode::EmptyWorkflowName,
            json!({"id": "w", "tasks": [task("t")]}),
        ),
        (
            "empty tasks array",
            IssueCode::NoTasks,
            json!({"id": "w", "name": "w", "tasks": []}),
        ),
        (
            "tasks is not an array",
            IssueCode::NoTasks,
            json!({"id": "w", "name": "w", "tasks": "nope"}),
        ),
        (
            "step with no id",
            IssueCode::MissingStepId,
            workflow(
                json!([{"name": "t", "function": {"name": "map", "input": {"mappings": []}}}]),
            ),
        ),
        (
            "duplicate task ids",
            IssueCode::DuplicateStepId,
            workflow(json!([task("dup"), task("dup")])),
        ),
        (
            "group id colliding with a task id",
            IssueCode::DuplicateStepId,
            workflow(json!([task("shared"), {"id": "shared", "tasks": [task("inner")]}])),
        ),
        (
            "empty group",
            IssueCode::EmptyGroup,
            workflow(json!([{"id": "g", "tasks": []}])),
        ),
        (
            "task with no function",
            IssueCode::MissingFunction,
            workflow(json!([{"id": "t", "name": "t"}])),
        ),
        (
            "function with an empty name",
            IssueCode::InvalidFunctionName,
            workflow(json!([{"id": "t", "name": "t", "function": {"name": ""}}])),
        ),
        (
            "terminal is not a boolean",
            IssueCode::InvalidTerminal,
            workflow(json!([{"id": "t", "name": "t", "terminal": "yes",
                            "function": {"name": "map", "input": {"mappings": []}}}])),
        ),
        (
            "loop increment below one",
            IssueCode::LoopIncrementTooSmall,
            json!({"id": "w", "name": "w", "loop": {"max": 3, "increment": 0},
                   "tasks": [task("t")]}),
        ),
        (
            "loop max not above init",
            IssueCode::LoopBoundEmpty,
            json!({"id": "w", "name": "w", "loop": {"init": 5, "max": 5}, "tasks": [task("t")]}),
        ),
        (
            "loop counter with an empty segment",
            IssueCode::LoopCounterInvalid,
            json!({"id": "w", "name": "w", "loop": {"max": 3, "counter": "a..b"},
                   "tasks": [task("t")]}),
        ),
        (
            "halt_on is not an accepted spelling",
            IssueCode::InvalidHaltOn,
            workflow(json!([{"id": "t", "name": "t", "halt_on": "on_failure",
                            "function": {"name": "map", "input": {"mappings": []}}}])),
        ),
        (
            "halt_on is not a string",
            IssueCode::InvalidHaltOn,
            workflow(json!([{"id": "t", "name": "t", "halt_on": true,
                            "function": {"name": "map", "input": {"mappings": []}}}])),
        ),
        (
            "halt_on on a group",
            IssueCode::InvalidHaltOn,
            workflow(json!([{"id": "g", "halt_on": "failure", "tasks": [task("inner")]}])),
        ),
    ]
}

/// The load-bearing test. Each fixture must report *its own* code — not merely
/// be rejected. Without this, the structural walk could return nothing and the
/// stage-2 parse would still make every biconditional assertion pass.
#[test]
fn every_broken_fixture_reports_its_own_code() {
    for (label, expected, json) in broken_fixtures() {
        let found = codes(&json);
        assert!(
            found.contains(&expected),
            "{label}: expected {expected:?}, got {found:?} — \
             if this is ParseFailed, the structural walk stopped doing its job"
        );
        assert!(
            !found.contains(&IssueCode::ParseFailed),
            "{label}: fell through to the catch-all instead of being diagnosed"
        );
    }
}

#[test]
fn empty_iff_the_workflow_loads() {
    let valid = vec![
        workflow(json!([task("a")])),
        workflow(json!([task("a"), {"id": "g", "condition": true, "tasks": [task("b")]}])),
        json!({"id": "w", "name": "w", "loop": {"max": 3, "counter": "i"},
               "tasks": [task("t")]}),
        // A group carrying `continue_on_error` loads: it is reported by
        // `check_workflow`, never here. An informational finding on the
        // authored side would break the biconditional this test states.
        workflow(json!([
            {"id": "g", "continue_on_error": true, "tasks": [task("a")]}
        ])),
        // Both accepted `halt_on` spellings load and report nothing.
        workflow(json!([{"id": "t", "name": "t", "halt_on": "failure",
                        "function": {"name": "map", "input": {"mappings": []}}}])),
        workflow(json!([{"id": "t", "name": "t", "halt_on": "never",
                        "function": {"name": "map", "input": {"mappings": []}}}])),
    ];
    for json in valid {
        assert!(
            Workflow::validate_authored(&json).is_empty(),
            "loadable workflow reported issues: {:?}",
            Workflow::validate_authored(&json)
        );
        assert!(loads(&json), "fixture claimed valid but does not load");
    }

    for (label, _, json) in broken_fixtures() {
        assert!(
            !Workflow::validate_authored(&json).is_empty(),
            "{label}: reported no issues"
        );
        assert!(!loads(&json), "{label}: claimed broken but loads fine");
    }
}

/// The six cases from the design: they break no semantic rule, so only the
/// parse stage can catch them. This is what the biconditional rests on.
#[test]
fn a_type_error_falls_through_to_parse_failed() {
    let cases = vec![
        (
            "map with no mappings",
            workflow(json!([{"id": "t", "name": "t", "function": {"name": "map", "input": {}}}])),
        ),
        (
            "priority as a string",
            json!({"id": "w", "name": "w", "priority": "high", "tasks": [task("t")]}),
        ),
        (
            "continue_on_error as an integer",
            workflow(json!([{"id": "t", "name": "t", "continue_on_error": 3,
                          "function": {"name": "map", "input": {"mappings": []}}}])),
        ),
        (
            "misspelled status",
            json!({"id": "w", "name": "w", "status": "enabled", "tasks": [task("t")]}),
        ),
        (
            "http_call with no connector",
            workflow(
                json!([{"id": "t", "name": "t", "function": {"name": "http_call", "input": {}}}]),
            ),
        ),
        (
            "loop max as a string",
            json!({"id": "w", "name": "w", "loop": {"max": "3"}, "tasks": [task("t")]}),
        ),
    ];

    for (label, json) in cases {
        let issues = Workflow::validate_authored(&json);
        assert_eq!(
            issues.iter().map(|i| i.code).collect::<Vec<_>>(),
            vec![IssueCode::ParseFailed],
            "{label}: should be caught by the parse stage alone"
        );
        assert!(
            !issues[0].message.is_empty(),
            "{label}: carries the parser's own message"
        );
        assert!(!loads(&json), "{label}: should not load");
    }
}

#[test]
fn all_violations_are_reported_not_just_the_first() {
    let json = json!({
        "id": "", "name": "",
        "tasks": [task("dup"), task("dup"), {"id": "g", "tasks": []}]
    });
    let found = codes(&json);

    for expected in [
        IssueCode::EmptyWorkflowId,
        IssueCode::EmptyWorkflowName,
        IssueCode::DuplicateStepId,
        IssueCode::EmptyGroup,
    ] {
        assert!(
            found.contains(&expected),
            "missing {expected:?} in {found:?}"
        );
    }
    assert!(
        found.len() >= 4,
        "expected four distinct problems, got {found:?}"
    );
}

#[test]
fn violations_carry_authored_coordinates_not_flat_indices() {
    // The duplicate is the second member of a group. A flattened view would
    // call it tasks[2]; the author wrote tasks[1].tasks[1].
    let json = workflow(json!([
        task("first"),
        {"id": "g", "condition": true, "tasks": [task("inner"), task("first")]}
    ]));

    let issues = Workflow::validate_authored(&json);
    let dup = issues
        .iter()
        .find(|i| i.code == IssueCode::DuplicateStepId)
        .expect("the collision is reported");

    assert_eq!(dup.path.as_deref(), Some("tasks[1].tasks[1].id"));
    assert_eq!(dup.task_id.as_deref(), Some("first"));
    assert!(
        dup.message.contains("tasks[0]"),
        "the message names where the id was first used, got: {}",
        dup.message
    );
}

#[test]
fn a_group_past_the_depth_cap_is_reported_at_the_parsers_boundary() {
    let depth = dataflow_rs::MAX_GROUP_DEPTH;

    let mut node = task("innermost");
    for level in (0..=depth).rev() {
        node = json!({"id": format!("g{level}"), "condition": true, "tasks": [node]});
    }
    let json = workflow(json!([node]));

    assert!(codes(&json).contains(&IssueCode::GroupTooDeep));
    assert!(!loads(&json), "the parser rejects it at the same boundary");

    // Exactly at the cap is fine, on both sides.
    let mut node = task("innermost");
    for level in (0..depth).rev() {
        node = json!({"id": format!("g{level}"), "condition": true, "tasks": [node]});
    }
    let ok = workflow(json!([node]));
    assert!(Workflow::validate_authored(&ok).is_empty());
    assert!(loads(&ok));
}

#[test]
fn a_non_object_input_does_not_panic() {
    for input in [Value::Null, json!([]), json!("workflow"), json!(7)] {
        let issues = Workflow::validate_authored(&input);
        assert!(!issues.is_empty(), "{input} is not a workflow");
        assert!(!loads(&input));
    }
}

#[test]
fn issue_codes_have_distinct_stable_strings() {
    let all = [
        IssueCode::EmptyWorkflowId,
        IssueCode::EmptyWorkflowName,
        IssueCode::NoTasks,
        IssueCode::MissingStepId,
        IssueCode::DuplicateStepId,
        IssueCode::EmptyGroup,
        IssueCode::GroupTooDeep,
        IssueCode::MissingFunction,
        IssueCode::InvalidFunctionName,
        IssueCode::InvalidTerminal,
        IssueCode::InvalidHaltOn,
        IssueCode::GroupContinueOnError,
        IssueCode::UnguardedValidation,
        IssueCode::LoopIncrementTooSmall,
        IssueCode::LoopBoundEmpty,
        IssueCode::LoopCounterInvalid,
        IssueCode::ParseFailed,
        IssueCode::ValidateFailed,
    ];
    let mut seen = std::collections::HashSet::new();
    for code in all {
        assert!(seen.insert(code.as_str()), "duplicate string for {code:?}");
        assert_eq!(code.to_string(), code.as_str(), "Display matches as_str");
        assert!(
            code.as_str()
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '_'),
            "{code:?} is not SCREAMING_SNAKE"
        );
    }
}

#[test]
fn the_validate_failed_backstop_stays_unreached() {
    // If this ever fires, `check_shape` has stopped modelling a rule that
    // `Workflow::validate` enforces — the caller still gets a correct answer,
    // but a semantic code with a path was owed and not delivered.
    for (label, _, json) in broken_fixtures() {
        assert!(
            !codes(&json).contains(&IssueCode::ValidateFailed),
            "{label}: reached the backstop instead of being diagnosed structurally"
        );
    }
}

// =============================================================================
// check_workflow — the registry half. `validate_authored` proves a definition
// parses and validates; this proves the engine can actually run it.
// =============================================================================

use async_trait::async_trait;
use dataflow_rs::engine::functions::AsyncFunctionHandler;
use dataflow_rs::{Engine, Result, TaskContext, TaskOutcome, Template};
use serde::Deserialize;

#[derive(Deserialize)]
struct StrictInput {
    #[allow(dead_code)]
    required_field: String,
}

/// Declares a typed Input, so a mismatched config fails at parse.
struct Strict;

#[async_trait]
impl AsyncFunctionHandler for Strict {
    type Input = StrictInput;
    async fn execute(&self, _c: &mut TaskContext<'_>, _i: &Self::Input) -> Result<TaskOutcome> {
        Ok(TaskOutcome::Success)
    }
}

#[derive(Deserialize)]
struct TemplatedInput {
    expr: Template,
    #[serde(default)]
    reject: bool,
}

/// Compiles a `Template` field, and rejects when asked.
///
/// The rejection matters: the engine runs datalogic in *templating* mode, where
/// an unknown or malformed operator is inert data rather than an error, so a
/// bare expression essentially cannot fail `Template::compile`. What `compile_input`
/// really guards is a handler's own construction-time validation — and that is
/// what aborts `Engine::build()` today, so it is what `check_workflow` must report.
struct Templated;

#[async_trait]
impl AsyncFunctionHandler for Templated {
    type Input = TemplatedInput;

    fn compile_input(input: &mut Self::Input, c: &dataflow_rs::TemplateCompiler) -> Result<()> {
        input.expr.compile(c, "expr")?;
        if input.reject {
            return Err(dataflow_rs::DataflowError::LogicEvaluation(
                "expr: this handler rejects it at construction".to_string(),
            ));
        }
        Ok(())
    }

    async fn execute(&self, _c: &mut TaskContext<'_>, _i: &Self::Input) -> Result<TaskOutcome> {
        Ok(TaskOutcome::Success)
    }
}

fn wf(task: Value) -> Workflow {
    Workflow::from_json(&workflow(json!([task])).to_string()).expect("fixture parses")
}

fn call(name: &str, input: Value) -> Value {
    json!({"id": "t", "name": "t", "function": {"name": name, "input": input}})
}

#[test]
fn a_clean_workflow_produces_no_issues() {
    let workflow = wf(task("ok"));
    assert!(Engine::builder().check_workflow(&workflow).is_empty());

    let engine = Engine::builder().build().unwrap();
    assert!(engine.check_workflow(&workflow).is_empty());
}

#[test]
fn an_unregistered_custom_name_is_an_unknown_function() {
    let workflow = wf(call("typo_handler", json!({})));
    let issues = Engine::builder().check_workflow(&workflow);

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code, IssueCode::UnknownFunction);
    assert_eq!(issues[0].task_id.as_deref(), Some("t"));
    assert_eq!(issues[0].path.as_deref(), Some("function.name"));
}

#[test]
fn a_config_only_integration_with_no_handler_is_a_missing_handler() {
    // The enrich trap, given its own code: the name is real, so "unknown
    // function" would send the author looking for a typo that isn't there.
    let workflow = wf(call(
        "enrich",
        json!({"connector": "c", "merge_path": "data.out"}),
    ));
    let issues = Engine::builder().check_workflow(&workflow);

    assert_eq!(issues[0].code, IssueCode::MissingHandler);
    assert!(
        issues[0].message.contains("config schema only"),
        "the message must say what to do, got: {}",
        issues[0].message
    );

    // Registering one closes it.
    let ok = Engine::builder().register("enrich", Strict);
    assert!(ok.check_workflow(&workflow).is_empty());
}

#[test]
fn a_custom_input_that_does_not_deserialize_is_an_input_parse_issue() {
    let workflow = wf(call("strict", json!({"wrong": 1})));
    let issues = Engine::builder()
        .register("strict", Strict)
        .check_workflow(&workflow);

    assert_eq!(issues[0].code, IssueCode::InputParse);
    assert_eq!(issues[0].task_id.as_deref(), Some("t"));
    assert_eq!(issues[0].path.as_deref(), Some("function.input"));
    assert!(
        issues[0].message.contains("required_field"),
        "carries the underlying reason, got: {}",
        issues[0].message
    );
}

#[test]
fn a_rejected_compile_input_is_a_template_compile_issue() {
    let workflow = wf(call(
        "templated",
        json!({"expr": {"var": "data.x"}, "reject": true}),
    ));
    let issues = Engine::builder()
        .register("templated", Templated)
        .check_workflow(&workflow);

    assert_eq!(issues[0].code, IssueCode::TemplateCompile);
    assert_eq!(issues[0].path.as_deref(), Some("function.input"));
    assert_eq!(issues[0].task_id.as_deref(), Some("t"));

    // And it is the same rejection that would abort a build.
    let build = Engine::builder()
        .register("templated", Templated)
        .with_workflow(wf(call(
            "templated",
            json!({"expr": {"var": "data.x"}, "reject": true}),
        )))
        .build();
    assert!(
        build.is_err(),
        "check_workflow reported what build enforces"
    );

    // Not rejected: clean on both sides.
    let ok = wf(call("templated", json!({"expr": {"var": "data.x"}})));
    assert!(
        Engine::builder()
            .register("templated", Templated)
            .check_workflow(&ok)
            .is_empty()
    );
}

/// The property the issue asks for, in both directions: `check_workflow` is
/// empty exactly when `build()` **and first dispatch** would run clean.
///
/// The distinction is the whole point. `build()` alone is deliberately
/// permissive about the config-only integrations — a workflow naming `enrich`
/// with no handler builds without complaint and then fails every message — so
/// testing against `build()` alone would have declared that case healthy.
#[tokio::test]
async fn check_workflow_agrees_with_build_plus_first_dispatch() {
    let cases: Vec<(&str, Value, bool)> = vec![
        ("clean", task("ok"), true),
        ("unregistered name", call("typo_handler", json!({})), false),
        (
            "config-only integration, no handler",
            call(
                "enrich",
                json!({"connector": "c", "merge_path": "data.out"}),
            ),
            false,
        ),
        (
            "bad custom input",
            call("strict", json!({"wrong": 1})),
            false,
        ),
        (
            "good custom input",
            call("strict", json!({"required_field": "here"})),
            true,
        ),
    ];

    for (label, task_json, should_run) in cases {
        let issues = Engine::builder()
            .register("strict", Strict)
            .check_workflow(&wf(task_json.clone()));

        // Build, then actually push a message through.
        let runs = match Engine::builder()
            .register("strict", Strict)
            .with_workflow(wf(task_json))
            .build()
        {
            Err(_) => false,
            Ok(engine) => {
                let mut message = dataflow_rs::engine::message::Message::from_value(&json!({}));
                engine.process_message(&mut message).await.is_ok()
            }
        };

        assert_eq!(
            runs, should_run,
            "{label}: build+dispatch disagreed with the fixture's expectation"
        );
        assert_eq!(
            issues.is_empty(),
            runs,
            "{label}: check_workflow said {:?}, build+dispatch said {runs}",
            issues.iter().map(|i| i.code).collect::<Vec<_>>()
        );
    }
}

/// The case that makes the property non-trivial, stated on its own.
#[tokio::test]
async fn build_alone_would_have_called_the_enrich_trap_healthy() {
    let workflow = wf(call(
        "enrich",
        json!({"connector": "c", "merge_path": "data.out"}),
    ));

    let engine = Engine::builder()
        .with_workflow(wf(call(
            "enrich",
            json!({"connector": "c", "merge_path": "data.out"}),
        )))
        .build()
        .expect("build accepts it — that permissiveness is deliberate");

    let mut message = dataflow_rs::engine::message::Message::from_value(&json!({}));
    assert!(
        engine.process_message(&mut message).await.is_err(),
        "and every message then fails"
    );

    assert_eq!(
        Engine::builder().check_workflow(&workflow)[0].code,
        IssueCode::MissingHandler,
        "which is exactly what check_workflow catches before activation"
    );
}

#[test]
fn a_task_inside_a_group_is_checked_too() {
    // `Workflow::tasks` is flattened, so a bad function inside a guard clause
    // cannot escape the check.
    let json = workflow(json!([
        task("before"),
        {"id": "guard", "condition": true, "tasks": [call("typo_handler", json!({}))]}
    ]));
    let workflow = Workflow::from_json(&json.to_string()).unwrap();

    let issues = Engine::builder().check_workflow(&workflow);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code, IssueCode::UnknownFunction);
    assert_eq!(
        issues[0].task_id.as_deref(),
        Some("t"),
        "anchored on the leaf task, never the enclosing group"
    );
}

#[test]
fn the_builder_and_the_engine_it_builds_agree() {
    let workflow = wf(call(
        "enrich",
        json!({"connector": "c", "merge_path": "data.out"}),
    ));

    let from_builder = Engine::builder().check_workflow(&workflow);
    let from_engine = Engine::builder().build().unwrap().check_workflow(&workflow);

    assert_eq!(from_builder, from_engine);
}

#[test]
fn every_bad_task_is_reported_not_just_the_first() {
    let json = workflow(json!([
        call("typo_one", json!({})),
        {"id": "t2", "name": "t2", "function": {"name": "typo_two", "input": {}}},
        {"id": "t3", "name": "t3", "function": {"name": "enrich",
                                                "input": {"connector": "c", "merge_path": "d"}}}
    ]));
    let workflow = Workflow::from_json(&json.to_string()).unwrap();

    let issues = Engine::builder().check_workflow(&workflow);
    assert_eq!(issues.len(), 3, "got {issues:?}");
    assert_eq!(
        issues
            .iter()
            .map(|i| i.task_id.clone().unwrap())
            .collect::<Vec<_>>(),
        vec!["t", "t2", "t3"],
        "in task order"
    );
}

// -----------------------------------------------------------------------------
// `UNGUARDED_VALIDATION` — the informational lint
//
// A failing `validation` rule returns `400`, which `continue_on_error` does not
// cover, so the tasks after it still run. The lint says so at authoring time. It
// is deliberately blunt: it asks whether *any* gate follows, not whether the gate
// is correct, because a guard's condition can read anything.
// -----------------------------------------------------------------------------

/// A `validation` task, optionally carrying extra keys.
fn validation_task(id: &str, extra: Value) -> Value {
    let mut task = json!({
        "id": id, "name": id,
        "function": {"name": "validation", "input": {
            "rules": [{"logic": {"==": [1, 2]}, "message": "always fails"}]}}
    });
    if let Value::Object(extra) = extra {
        for (k, v) in extra {
            task[k] = v;
        }
    }
    task
}

fn wf_tasks(tasks: Value) -> Workflow {
    Workflow::from_json(&workflow(tasks).to_string()).expect("fixture parses")
}

/// The issue's own repro: a `validation` followed by an unconditional task.
#[test]
fn an_unguarded_validation_is_reported_for_audit_but_never_refused() {
    let w = wf_tasks(json!([
        validation_task("check", json!({})),
        task("respond")
    ]));

    let issues = Engine::builder().check_workflow(&w);
    assert_eq!(issues.len(), 1, "got {issues:?}");
    assert_eq!(issues[0].code, IssueCode::UnguardedValidation);
    assert_eq!(issues[0].task_id.as_deref(), Some("check"));
    assert_eq!(issues[0].path.as_deref(), Some("halt_on"));
    assert!(
        issues[0].message.contains("respond"),
        "the message names the task that still runs, got: {}",
        issues[0].message
    );

    // Informational: the workflow is legal and still builds.
    Engine::builder()
        .with_workflow(w)
        .build()
        .expect("an unguarded validation is reported, never refused");
}

#[test]
fn a_guarded_validation_is_silent() {
    let cases: Vec<(&str, Value)> = vec![
        (
            "halt_on stops the workflow itself",
            json!([
                validation_task("check", json!({"halt_on": "failure"})),
                task("respond")
            ]),
        ),
        (
            "terminal stops the workflow itself",
            json!([
                validation_task("check", json!({"terminal": true})),
                task("respond")
            ]),
        ),
        (
            "nothing follows it in this workflow",
            json!([task("first"), validation_task("check", json!({}))]),
        ),
        (
            "the next task carries a condition",
            json!([validation_task("check", json!({})),
                   {"id": "respond", "name": "respond",
                    "condition": {"<": [{"var": "metadata.progress.status_code"}, 400]},
                    "function": {"name": "map", "input": {"mappings": []}}}]),
        ),
        (
            "a filter follows it",
            json!([validation_task("check", json!({})),
                   {"id": "gate", "name": "gate",
                    "function": {"name": "filter", "input": {"condition": true}}},
                   task("respond")]),
        ),
        (
            "the following tasks sit in a conditioned group",
            json!([validation_task("check", json!({})),
                   {"id": "g", "condition": {"var": "data.ok"}, "tasks": [task("respond")]}]),
        ),
    ];

    for (label, tasks) in cases {
        let issues = Engine::builder().check_workflow(&wf_tasks(tasks));
        assert!(
            !issues
                .iter()
                .any(|i| i.code == IssueCode::UnguardedValidation),
            "{label}: should not fire, got {issues:?}"
        );
    }
}

/// The lint reads the *flattened* task list, so a validation nested in a group
/// is still checked against what follows it.
#[test]
fn an_unguarded_validation_inside_a_group_is_reported() {
    let w = wf_tasks(json!([
        {"id": "g", "condition": true,
         "tasks": [validation_task("check", json!({})), task("respond")]}
    ]));

    let issues = Engine::builder().check_workflow(&w);
    assert_eq!(
        issues
            .iter()
            .filter(|i| i.code == IssueCode::UnguardedValidation)
            .count(),
        1,
        "got {issues:?}"
    );
}

/// #54: the key parses cleanly, does nothing, and is now reported — without
/// becoming a refusal, because unlike `halt_on` it has an installed base.
#[test]
fn a_group_continue_on_error_is_reported_for_audit_but_never_refused() {
    let w = wf_tasks(json!([
        {"id": "g", "continue_on_error": true, "tasks": [task("inner")]}
    ]));

    let issues = Engine::builder().check_workflow(&w);
    assert_eq!(issues.len(), 1, "got {issues:?}");
    assert_eq!(issues[0].code, IssueCode::GroupContinueOnError);
    assert_eq!(
        issues[0].task_id.as_deref(),
        Some("g"),
        "anchored on the group, not on a task inside it"
    );
    assert_eq!(issues[0].path.as_deref(), Some("continue_on_error"));

    // Informational: the workflow is legal and still builds.
    Engine::builder()
        .with_workflow(w)
        .build()
        .expect("a group's continue_on_error is reported, never refused");
}

/// Only a literal `true` states an intent the engine defeats. Everything else
/// either describes what already happens or is not the key at all — and none of
/// it may stop the definition loading.
#[test]
fn a_group_without_continue_on_error_is_silent() {
    let cases: Vec<(&str, Value)> = vec![
        (
            "the key is absent",
            json!({"id": "g", "tasks": [task("inner")]}),
        ),
        (
            "false already describes the behaviour",
            json!({"id": "g", "continue_on_error": false, "tasks": [task("inner")]}),
        ),
        (
            "a non-bool spelling is an unknown key, as everywhere else",
            json!({"id": "g", "continue_on_error": "yes", "tasks": [task("inner")]}),
        ),
    ];

    for (label, group) in cases {
        assert!(
            loads(&workflow(json!([group.clone()]))),
            "{label}: capturing the key must not make it a parse error"
        );
        let issues = Engine::builder().check_workflow(&wf_tasks(json!([group])));
        assert!(
            !issues
                .iter()
                .any(|i| i.code == IssueCode::GroupContinueOnError),
            "{label}: should not fire, got {issues:?}"
        );
    }
}

/// Groups are recorded on the task that opens their span, outermost first, so
/// both levels of a nest are visited exactly once.
#[test]
fn a_nested_group_carrying_it_is_reported_too() {
    let w = wf_tasks(json!([
        {"id": "outer", "continue_on_error": true, "tasks": [
            {"id": "inner", "continue_on_error": true, "tasks": [task("t")]}
        ]}
    ]));

    let reported: Vec<String> = Engine::builder()
        .check_workflow(&w)
        .into_iter()
        .filter(|i| i.code == IssueCode::GroupContinueOnError)
        .filter_map(|i| i.task_id)
        .collect();

    assert_eq!(
        reported,
        ["outer", "inner"],
        "each group once, outermost first"
    );
}
