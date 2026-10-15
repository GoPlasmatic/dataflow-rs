extern crate dataflow_rs_v4 as dataflow_rs;

// `function::testing`: one call of one function through the real compile,
// the real `Ctx` and overlay, and the layers given, with no registry,
// compiler, definition or engine assembled by hand (R59, I93).

use dataflow_rs::expr::{CompileCx, ExprVisitor, FieldSpec, Output};
use dataflow_rs::function::testing::{self, Called, Write};
use dataflow_rs::function::{BoxFuture, CallLayer, FunctionConfig, Next, Prepare};
use dataflow_rs::program::{IssueCode, StepKind};
use dataflow_rs::run::{Change, ErrorKind, ErrorRecord};
use dataflow_rs::{Ctx, Error, Expr, Function, Issue, Message, Outcome, Path, Result};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

type Log = Arc<Mutex<Vec<String>>>;

fn dv(v: Value) -> OwnedDataValue {
    OwnedDataValue::from(&v)
}

fn path(dotted: &str) -> Path {
    dotted.parse().unwrap()
}

fn json_of(v: &OwnedDataValue) -> Value {
    Value::from(v)
}

/// Each issue's code and path.
fn found(issues: &[Issue]) -> Vec<(IssueCode, String)> {
    issues
        .iter()
        .map(|i| (i.code, i.path.to_string()))
        .collect()
}

/// `greet`'s config.
#[derive(FunctionConfig)]
struct GreetConfig {
    /// Evaluated against the context.
    greeting: Expr,
    #[output(default = "data.greeting")]
    output: Output,
}

/// Writes its greeting through its output, removes `data.old`, then writes
/// below `data.n`, which the call's own context cannot take where `data.n`
/// is a scalar.
struct Greet;

impl Function for Greet {
    type Config = GreetConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &GreetConfig) -> Result<Outcome> {
        let greeting = ctx.eval(&cfg.greeting)?.into_owned();
        ctx.emit_default(greeting)?;
        ctx.remove(&path("data.old"))?;
        ctx.set(&path("data.n.x"), dv(json!(1)))?;
        Ok(Outcome::Status(201))
    }
}

/// Logs the call it wraps, and what the call left at `data.greeting`.
struct Watch(Log);

impl CallLayer for Watch {
    fn call<'a>(&'a self, ctx: &'a mut Ctx<'_>, next: Next<'a>) -> BoxFuture<'a, Result<Outcome>> {
        Box::pin(async move {
            let entered = format!("enter {}", ctx.info().function);
            self.0.lock().unwrap().push(entered);
            let outcome = next.run(ctx).await;
            let seen = ctx.get(&path("data.greeting")).map(|v| json_of(&v));
            let left = format!("leave {outcome:?} {seen:?}");
            self.0.lock().unwrap().push(left);
            outcome
        })
    }
}

#[tokio::test]
async fn call_runs_the_real_compile_overlay_and_layers() {
    let log = Log::default();
    let given = Message::from_parts(
        OwnedDataValue::Null,
        dv(json!({"name": "ada", "old": 1, "n": 5})),
        OwnedDataValue::Null,
    );
    let input = json!({"greeting": {"cat": ["hello ", {"var": "data.name"}]}});
    let called = testing::call(Greet, input)
        .message(given.clone())
        .layer(Watch(Arc::clone(&log)))
        .await;

    assert!(called.issues.is_empty(), "{:?}", called.issues);
    assert_eq!(called.outcome.unwrap(), Outcome::Status(201));
    // The overlay in order, each kind of write as the call made it: the
    // write below the scalar is kept, and replayed at the settle.
    assert_eq!(
        called.writes,
        [
            (path("data.greeting"), Write::Set(dv(json!("hello ada")))),
            (path("data.old"), Write::Removed),
            (path("data.n.x"), Write::Blocked(dv(json!(1)))),
        ]
    );
    assert_eq!(
        called.changes,
        [
            Change::write(
                "data.greeting",
                OwnedDataValue::Null,
                dv(json!("hello ada"))
            ),
            Change::removal("data.old", dv(json!(1))),
            Change::write("data.n.x", OwnedDataValue::Null, dv(json!(1))),
        ]
    );
    // The message after the settle: the writes applied, the blocked one
    // landing nowhere; no recorder ran, so no progress, entry or record.
    assert_eq!(
        json_of(called.message.context().data()),
        json!({"name": "ada", "n": 5, "greeting": "hello ada"})
    );
    assert_eq!(json_of(called.message.context().metadata()), json!({}));
    assert!(called.message.audit().is_empty());
    assert!(called.errors.is_empty());
    assert!(called.message.errors().is_empty());
    assert_eq!(called.message.id(), given.id());
    // The layer wrapped the call, and saw its writes.
    assert_eq!(
        *log.lock().unwrap(),
        [
            "enter function",
            "leave Ok(Status(201)) Some(String(\"hello ada\"))"
        ]
    );
}

#[tokio::test]
async fn a_rejected_config_reports_every_issue_and_runs_nothing() {
    let log = Log::default();
    let given = Message::from_parts(
        OwnedDataValue::Null,
        dv(json!({"name": "ada"})),
        OwnedDataValue::Null,
    );

    // The config's own issues, every one, at its path below the input and
    // stamped with the test's task.
    let called = testing::call(Greet, json!({"greting": "hi"}))
        .message(given.clone())
        .layer(Watch(Arc::clone(&log)))
        .await;
    assert_eq!(
        found(&called.issues),
        [
            (IssueCode::LikelyTypo, "greting".to_owned()),
            (IssueCode::InputParse, "greeting".to_owned()),
        ]
    );
    for issue in &called.issues {
        assert_eq!(issue.step_kind, Some(StepKind::Task), "{issue:?}");
        assert_eq!(issue.step_id.as_deref(), Some("test"), "{issue:?}");
    }
    let error = called.outcome.unwrap_err();
    assert_eq!(*error.kind(), ErrorKind::Compile);
    assert!(called.writes.is_empty() && called.changes.is_empty());
    assert!(called.errors.is_empty());
    assert_eq!(called.message, given);

    // A config that compiles and fails the checks a workflow's compile
    // runs: its output, which the engine records, reads a secret.
    let input = json!({"greeting": "hi", "output": {"cat": ["data.", {"secret": "k"}]}});
    let called = testing::call(Greet, input)
        .message(given.clone())
        .layer(Watch(Arc::clone(&log)))
        .await;
    assert_eq!(
        found(&called.issues),
        [(IssueCode::SecretInMessageWrite, "output".to_owned())]
    );
    assert_eq!(*called.outcome.unwrap_err().kind(), ErrorKind::Compile);
    assert_eq!(called.message, given);

    // Neither call reached the layer or the function.
    assert!(log.lock().unwrap().is_empty());
}

/// Writes, records two errors, then fails.
struct Flaky;

impl Function for Flaky {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        ctx.set(&path("data.partial"), dv(json!(true)))?;
        ctx.record_error(ErrorRecord::new("PARTNER_SLOW", "slow"));
        let mut dated = ErrorRecord::new("PARTNER_RETRIED", "retried");
        dated.timestamp = Some("then".to_owned());
        ctx.record_error(dated);
        Err(Error::service("PARTNER_DOWN", "down"))
    }
}

/// Replaces `data` whole.
struct Replace;

impl Function for Replace {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        ctx.set(&path("data"), dv(json!({"fresh": true})))?;
        Ok(Outcome::Ok)
    }
}

#[tokio::test]
async fn a_write_at_a_root_says_replaced_as_recorded() {
    // `Called::changes` keeps the flag as the call recorded it; only the
    // audit entry's copy reads `false` (I106).
    let called = testing::call(Replace, json!({})).await;
    assert_eq!(called.changes.len(), 1);
    assert!(called.changes[0].replaced, "{:?}", called.changes);
    assert_eq!(called.message.context().data(), &dv(json!({"fresh": true})));
}

#[tokio::test]
async fn an_err_leaves_the_message_as_given_and_keeps_the_records() {
    let given = Message::from_parts(
        OwnedDataValue::Null,
        dv(json!({"a": 1})),
        OwnedDataValue::Null,
    );
    let called = testing::call(Flaky, json!({})).message(given.clone()).await;

    let error = called.outcome.unwrap_err();
    assert_eq!(
        *error.kind(),
        ErrorKind::Service {
            code: "PARTNER_DOWN".into()
        }
    );
    assert_eq!(error.message(), "down");
    // The writes and their `Change`s are dropped.
    assert!(called.writes.is_empty());
    assert!(called.changes.is_empty());
    assert_eq!(called.message.context(), given.context());
    assert!(called.message.audit().is_empty());

    // The records are kept as given, a timestamp added where they carry
    // none (I96), and kept on the message as a run keeps them.
    let codes: Vec<&str> = called.errors.iter().map(|r| r.code.as_str()).collect();
    assert_eq!(codes, ["PARTNER_SLOW", "PARTNER_RETRIED"]);
    let stamped = called.errors[0].timestamp.as_deref().expect("stamped");
    assert!(
        chrono::DateTime::parse_from_rfc3339(stamped).is_ok(),
        "{stamped}"
    );
    assert_eq!(called.errors[1].timestamp.as_deref(), Some("then"));
    assert!(called.errors.iter().all(|r| r.element_index.is_none()));
    assert_eq!(called.message.errors(), called.errors);
}

/// A hand-written config that refuses its input without saying why.
struct Silent;

impl FunctionConfig for Silent {
    fn compile(_input: &Value, _cx: &mut CompileCx<'_>) -> Option<Self> {
        None
    }
    fn visit(&self, _v: &mut dyn ExprVisitor) {}
    fn fields() -> &'static [FieldSpec] {
        &[]
    }
}

#[test]
fn config_compiles_a_config_alone() {
    let Ok(cfg) = testing::config::<GreetConfig>(json!({"greeting": "hi"})) else {
        panic!("the config compiles");
    };
    assert_eq!(cfg.greeting.constant(), Some(&dv(json!("hi"))));
    assert_eq!(cfg.output.constant(), Some(&path("data.greeting")));

    // `Err` carries every issue, at its path below the input; no task
    // compiled it, so none is stamped with a step.
    let Err(issues) = testing::config::<GreetConfig>(json!({"greting": "hi"})) else {
        panic!("the config is rejected");
    };
    assert_eq!(
        found(&issues),
        [
            (IssueCode::LikelyTypo, "greting".to_owned()),
            (IssueCode::InputParse, "greeting".to_owned()),
        ]
    );
    assert!(issues.iter().all(|i| i.step_kind.is_none()));

    // A refusal with no issue is reported, so `Err` is never empty.
    let Err(issues) = testing::config::<Silent>(json!({})) else {
        panic!("the config refuses");
    };
    assert_eq!(found(&issues), [(IssueCode::InputParse, String::new())]);
}

/// Logs what `prepare` was told and what the call saw.
struct Names(Log);

impl Names {
    fn log(&self, line: String) {
        self.0.lock().unwrap().push(line);
    }
}

impl Function for Names {
    type Config = ();

    fn prepare(&self, _cfg: &mut (), p: &Prepare<'_>) -> std::result::Result<(), Vec<Issue>> {
        self.log(format!(
            "prepare {} {} {}",
            p.name(),
            p.workflow_id(),
            p.task_id()
        ));
        Ok(())
    }

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        let info = ctx.info();
        let line = format!(
            "call {} {} {} {:?} {:?} {:?}",
            info.function,
            info.workflow_id,
            info.task_id,
            info.channel,
            info.loop_counter,
            info.element_index
        );
        self.log(line);
        let payload = json_of(ctx.payload());
        self.log(format!("payload {payload}"));
        ctx.set(&path("data.seen"), dv(json!(true)))?;
        Ok(Outcome::Ok)
    }
}

#[tokio::test]
async fn the_defaults_name_the_call_function_test_test() {
    let log = Log::default();
    let called = testing::call(Names(Arc::clone(&log)), json!({})).await;
    assert_eq!(
        *log.lock().unwrap(),
        [
            "prepare function test test",
            "call function test test None None None",
            "payload null",
        ]
    );
    // An empty message by default, and changes captured.
    assert_eq!(called.outcome.unwrap(), Outcome::Ok);
    assert_eq!(
        json_of(called.message.context().data()),
        json!({"seen": true})
    );
    assert_eq!(
        called.changes,
        [Change::write(
            "data.seen",
            OwnedDataValue::Null,
            dv(json!(true))
        )]
    );

    // The name is what `prepare` and the call are told; without capture
    // the write still lands, and records no `Change`.
    log.lock().unwrap().clear();
    let called = testing::call(Names(Arc::clone(&log)), json!(null))
        .name("lookup")
        .capture_changes(false)
        .await;
    assert_eq!(
        *log.lock().unwrap(),
        [
            "prepare lookup test test",
            "call lookup test test None None None",
            "payload null",
        ]
    );
    assert!(called.changes.is_empty());
    assert_eq!(
        called.writes,
        [(path("data.seen"), Write::Set(dv(json!(true))))]
    );
    assert_eq!(
        json_of(called.message.context().data()),
        json!({"seen": true})
    );
}

#[tokio::test]
async fn call_is_awaited_directly() {
    // `.await` on the builder runs it: `Call` is `IntoFuture`.
    let called: Called = testing::call(Greet, json!({"greeting": "hi"})).await;
    assert_eq!(called.outcome.unwrap(), Outcome::Status(201));

    // Its future is `Send` and `'static`, so a test may spawn it.
    let future = testing::call(Greet, json!({"greeting": "hi"})).into_future();
    let called = tokio::spawn(future).await.unwrap();
    assert_eq!(called.outcome.unwrap(), Outcome::Status(201));
    assert_eq!(
        json_of(called.message.context().data()),
        json!({"greeting": "hi", "n": {"x": 1}})
    );
}
