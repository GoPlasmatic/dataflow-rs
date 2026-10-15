extern crate dataflow_rs_v4 as dataflow_rs;

// `function::testing`: one call of one function through the real compile,
// the real `Ctx` and overlay, and the layers given, with no registry,
// compiler, definition or engine assembled by hand (R59, I93).

use dataflow_rs::datalogic_rs::{self, CustomOperator};
use dataflow_rs::expr::{CompileCx, ExprEngine, ExprVisitor, FieldSpec, Output};
use dataflow_rs::function::testing::{self, Called, Write};
use dataflow_rs::function::{BoxFuture, CallLayer, FunctionConfig, Next, Prepare};
use dataflow_rs::program::{IssueCode, StepKind};
use dataflow_rs::run::{CancelToken, Change, ErrorKind, ErrorRecord, Instant};
use dataflow_rs::{Ctx, Error, Expr, Function, Issue, Message, Outcome, Path, Result};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

/// What a call was told of its channel and the run's limits.
#[derive(Debug)]
struct Told {
    channel: Option<String>,
    deadline: Option<Instant>,
    cancel: Option<CancelToken>,
}

/// Records what it was told, writes `data.ran`, then, when `cancel_self`
/// says so, cancels the token it was given and waits on it forever.
struct Tell {
    told: Arc<Mutex<Vec<Told>>>,
    cancel_self: bool,
}

impl Tell {
    fn new(told: &Arc<Mutex<Vec<Told>>>) -> Self {
        Self {
            told: Arc::clone(told),
            cancel_self: false,
        }
    }
}

impl Function for Tell {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        self.told.lock().unwrap().push(Told {
            channel: ctx.info().channel.map(str::to_owned),
            deadline: ctx.deadline(),
            cancel: ctx.cancel().cloned(),
        });
        ctx.set(&path("data.ran"), dv(json!(true)))?;
        if self.cancel_self {
            ctx.cancel().expect("a token was given").cancel();
            std::future::pending::<()>().await;
        }
        Ok(Outcome::Ok)
    }
}

#[tokio::test]
async fn a_call_is_told_the_channel_deadline_and_token_given() {
    let told = Arc::new(Mutex::new(Vec::new()));
    let at = Instant::now() + Duration::from_secs(60);
    let token = CancelToken::new();
    let called = testing::call(Tell::new(&told), json!({}))
        .channel("orders")
        .deadline(at)
        .cancel(token.clone())
        .await;
    assert_eq!(called.outcome.unwrap(), Outcome::Ok);
    assert_eq!(
        json_of(called.message.context().data()),
        json!({"ran": true})
    );
    {
        let told = told.lock().unwrap();
        assert_eq!(told[0].channel.as_deref(), Some("orders"));
        assert_eq!(told[0].deadline, Some(at));
        // The call holds the token given: cancelling it cancels the call's.
        let seen = told[0].cancel.as_ref().expect("the call sees a token");
        assert!(!seen.is_cancelled());
        token.cancel();
        assert!(seen.is_cancelled());
    }

    // By default, none of the three.
    told.lock().unwrap().clear();
    let called = testing::call(Tell::new(&told), json!({})).await;
    assert_eq!(called.outcome.unwrap(), Outcome::Ok);
    let told = told.lock().unwrap();
    assert!(told[0].channel.is_none() && told[0].deadline.is_none() && told[0].cancel.is_none());
}

#[tokio::test]
async fn the_runs_limits_end_a_call_as_a_run_ends_it() {
    let told = Arc::new(Mutex::new(Vec::new()));
    // A token cancelled before the call: the function never runs, and the
    // call ends `Cancelled`, naming it, as a run's host call does.
    let token = CancelToken::new();
    token.cancel();
    let called = testing::call(Tell::new(&told), json!({}))
        .name("lookup")
        .cancel(token)
        .await;
    let err = called.outcome.unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::Cancelled);
    assert_eq!(err.message(), "the run was cancelled during `lookup`");
    assert!(told.lock().unwrap().is_empty());
    assert!(called.writes.is_empty() && called.changes.is_empty());
    assert_eq!(json_of(called.message.context().data()), json!({}));

    // A deadline already passed: `TimedOut`, and nothing runs.
    let called = testing::call(Tell::new(&told), json!({}))
        .name("lookup")
        .deadline(Instant::now())
        .await;
    let err = called.outcome.unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::TimedOut);
    assert_eq!(err.message(), "the run's deadline passed during `lookup`");
    assert!(told.lock().unwrap().is_empty());

    // A token cancelled during the call: the race drops it, and its write.
    let called = testing::call(
        Tell {
            told: Arc::clone(&told),
            cancel_self: true,
        },
        json!({}),
    )
    .name("lookup")
    .cancel(CancelToken::new())
    .await;
    let err = called.outcome.unwrap_err();
    assert_eq!(err.kind(), &ErrorKind::Cancelled);
    assert_eq!(told.lock().unwrap().len(), 1);
    assert!(called.writes.is_empty() && called.changes.is_empty());
    assert_eq!(json_of(called.message.context().data()), json!({}));
}

/// `answer`: always 42.
struct Answer42;

impl CustomOperator for Answer42 {
    fn evaluate<'a>(
        &self,
        _args: &[&'a datalogic_rs::DataValue<'a>],
        _ctx: &mut datalogic_rs::operator::EvalContext<'_, 'a>,
        arena: &'a datalogic_rs::bumpalo::Bump,
    ) -> datalogic_rs::Result<&'a datalogic_rs::DataValue<'a>> {
        Ok(arena.alloc(datalogic_rs::DataValue::Number(
            datavalue::NumberValue::from_i64(42),
        )))
    }
}

/// A config whose limit must be known at compile, from what the engine
/// it compiles with can evaluate: the host's operators included.
#[derive(FunctionConfig)]
#[config(check = known_now)]
struct Limit {
    limit: Expr<u64>,
}

/// Evaluates the limit over an empty context with the compile's engine.
fn known_now(cfg: &Limit, cx: &mut CompileCx<'_>) {
    let empty = json!({"data": {}, "metadata": {}, "temp_data": {}});
    if let Err(e) = cx.exprs().eval(&cfg.limit, &empty) {
        let why = format!("the limit must be known at compile: {}", e.message());
        cx.at("limit", |cx| cx.issue(IssueCode::InputParse, why));
    }
}

#[test]
fn config_with_compiles_with_the_engine_given() {
    let exprs = ExprEngine::builder()
        .operator("answer", Answer42)
        .build()
        .unwrap();
    let input = json!({"limit": {"answer": []}});
    // To the default engine `answer` is no operator: the object is data,
    // which is not a u64, so `config` refuses it.
    let Err(issues) = testing::config::<Limit>(input.clone()) else {
        panic!("`answer` is no operator to the default engine");
    };
    assert_eq!(
        found(&issues),
        [(IssueCode::InputParse, "limit".to_owned())]
    );
    assert!(issues[0].message.contains("a u64"), "{}", issues[0].message);
    // With the host's engine it is an operator, and evaluates.
    let Ok(cfg) = testing::config_with::<Limit>(&exprs, input) else {
        panic!("the config compiles with the host's operator");
    };
    let root = dv(json!({"data": {}, "metadata": {}, "temp_data": {}}));
    assert_eq!(exprs.eval(&cfg.limit, &root).unwrap().value, 42);
    // `Err` carries every issue, as `config`'s does.
    let Err(issues) = testing::config_with::<Limit>(&exprs, json!({"limt": 1})) else {
        panic!("the config is rejected");
    };
    assert_eq!(
        found(&issues),
        [
            (IssueCode::LikelyTypo, "limt".to_owned()),
            (IssueCode::InputParse, "limit".to_owned()),
        ]
    );
}

/// Writes what `ctx.input()` gives at `data.input`.
struct EchoInput;

impl Function for EchoInput {
    type Config = GreetConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &GreetConfig) -> Result<Outcome> {
        let input = ctx.input()?;
        ctx.set(&path("data.input"), input)?;
        Ok(Outcome::Ok)
    }
}

#[tokio::test]
async fn a_tested_call_reads_its_input_resolved() {
    let input = json!({"greeting": {"cat": ["hi ", {"var": "data.name"}]}});
    let called = testing::call(EchoInput, input)
        .message(Message::from_parts(
            OwnedDataValue::Null,
            dv(json!({"name": "ada"})),
            OwnedDataValue::Null,
        ))
        .await;
    assert!(called.issues.is_empty(), "{:?}", called.issues);
    assert_eq!(called.outcome.unwrap(), Outcome::Ok);
    // The greeting evaluated over the message given, and the output's
    // default path.
    assert_eq!(
        called
            .message
            .context()
            .get(&path("data.input"))
            .map(json_of),
        Some(json!({"greeting": "hi ada", "output": "data.greeting"}))
    );
}
