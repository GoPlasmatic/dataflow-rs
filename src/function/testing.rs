//! One function call outside a run, for a host's unit tests.
//!
//! [`call`] runs one call of one function through the path a run takes:
//!
//! - the config compiles as it would in a workflow: the function is
//!   registered, its config compiled under a [`Prepare`](super::Prepare) naming it, and the
//!   checks a workflow's compile runs walk the config's expressions;
//! - the call gets a real [`Ctx`](super::Ctx) and overlay over the message
//!   given, and the layers given run around it, as a run's layers do;
//! - it settles as a run settles a call: an `Ok`, whatever its [`Outcome`],
//!   applies the call's writes to the message, an `Err` drops them and their
//!   `Change`s, and the records the call added are kept either way.
//!
//! No recorder runs, so the message gains no audit entry, no
//! `metadata.progress` and no record of the call's own `Err`: the [`Called`]
//! says what the call returned. No registry, compiler, definition or engine
//! is assembled by hand. [`config`] compiles a config alone, and
//! [`config_with`] compiles one with the host's own [`ExprEngine`].

pub use super::overlay::Write;
use super::registry::Entry;
use super::{
    BoundCall, BoxFuture, CallInfo, CallLayer, CallStart, Function, FunctionConfig, Outcome,
    Registry,
};
use crate::base::any_rejected;
use crate::base::{
    AuthoredPath, CancelToken, Change, Error, ErrorKind, ErrorRecord, Instant, Issue, ProgramId,
    Result, StepKind, TaskIx, WorkflowIx,
};
use crate::expr::{CompileCx, ExprEngine, Path};
use crate::program::{Callee, CalleeAt, Checks, compile_callee, refused};
use crate::run::host::{Invoked, Layers, apply_writes, invoke, keep_records};
use crate::run::{Message, TaskIds};
use crate::state::{RunContext, Tuning};
use chrono::Utc;
use datavalue::OwnedDataValue;
use serde_json::Value;
use std::fmt;
use std::future::IntoFuture;
use std::sync::Arc;

/// The workflow and task ids a tested call is compiled and run under.
const TEST: &str = "test";

/// One call of `f`, with its config compiled from `input` as a workflow
/// would compile it; `.await` runs it.
///
/// The builder's setters are optional: an empty message, no layers,
/// [`ExprEngine::default`], the name `"function"`, changes captured, and no
/// channel, deadline or cancellation token.
///
/// # Panics
///
/// When awaited, wherever [`Registry::register`] panics: on a name it
/// reserves, or on a spec the function's config cannot honour.
///
/// ```
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// use dataflow_rs::datavalue::OwnedDataValue;
/// use dataflow_rs::expr::Output;
/// use dataflow_rs::function::testing::{self, Write};
/// use dataflow_rs::function::{BoxFuture, CallLayer, FunctionConfig, Next};
/// use dataflow_rs::program::IssueCode;
/// use dataflow_rs::{Ctx, Expr, Function, Message, Outcome, Result};
/// use serde_json::json;
/// use std::collections::BTreeMap;
/// use std::sync::atomic::{AtomicUsize, Ordering};
/// use std::sync::Arc;
///
/// #[derive(FunctionConfig)]
/// struct DbRead {
///     query: Expr<String>,
///     #[output(default = "data.rows")]
///     output: Output,
/// }
///
/// /// Answers each query from the rows it was built with, as the host's
/// /// database would.
/// struct MyFn {
///     rows: BTreeMap<String, serde_json::Value>,
/// }
///
/// impl MyFn {
///     fn new(rows: BTreeMap<String, serde_json::Value>) -> Self {
///         Self { rows }
///     }
/// }
///
/// impl Function for MyFn {
///     type Config = DbRead;
///
///     async fn call(&self, ctx: &mut Ctx<'_>, cfg: &DbRead) -> Result<Outcome> {
///         let query = ctx.eval(&cfg.query)?;
///         let Some(rows) = self.rows.get(&*query) else {
///             return Ok(Outcome::Status(404));
///         };
///         ctx.emit(&cfg.output, rows.into())?;
///         Ok(Outcome::Ok)
///     }
/// }
///
/// /// Counts the calls it wraps.
/// struct Counted(Arc<AtomicUsize>);
///
/// impl CallLayer for Counted {
///     fn call<'a>(&'a self, ctx: &'a mut Ctx<'_>, next: Next<'a>) -> BoxFuture<'a, Result<Outcome>> {
///         self.0.fetch_add(1, Ordering::Relaxed);
///         next.run(ctx)
///     }
/// }
///
/// let deps = BTreeMap::from([(
///     "select name from customers where id = 7".to_owned(),
///     json!([{"name": "Ada"}]),
/// )]);
/// let calls = Arc::new(AtomicUsize::new(0));
/// let input = json!({
///     "query": {"cat": ["select name from customers where id = ", {"var": "data.id"}]},
///     "output": "data.rows",
/// });
/// let data = OwnedDataValue::from(&json!({"id": 7}));
/// let out = testing::call(MyFn::new(deps), input)
///     .message(Message::from_parts(OwnedDataValue::Null, data, OwnedDataValue::Null))
///     .layer(Counted(Arc::clone(&calls)))
///     .await;
///
/// let rows = OwnedDataValue::from(&json!([{"name": "Ada"}]));
/// assert!(out.issues.is_empty());
/// assert_eq!(out.outcome.unwrap(), Outcome::Ok);
/// assert_eq!(out.writes, [("data.rows".parse().unwrap(), Write::Set(rows.clone()))]);
/// assert_eq!(out.changes.len(), 1);
/// assert_eq!(out.message.context().get(&"data.rows".parse().unwrap()), Some(&rows));
/// assert_eq!(calls.load(Ordering::Relaxed), 1);
///
/// // A config alone; `Err` carries every issue.
/// let cfg: DbRead = testing::config(json!({"query": "select 1"})).ok().unwrap();
/// assert_eq!(cfg.output.constant().unwrap().to_string(), "data.rows");
/// let issues = testing::config::<DbRead>(json!({"qeury": "select 1"})).err().unwrap();
/// let codes: Vec<IssueCode> = issues.iter().map(|i| i.code).collect();
/// assert_eq!(codes, [IssueCode::LikelyTypo, IssueCode::InputParse]);
/// # });
/// ```
pub fn call<F: Function>(f: F, input: Value) -> Call<F> {
    Call {
        f,
        input,
        message: None,
        layers: Vec::new(),
        exprs: None,
        name: "function".to_owned(),
        capture: true,
        channel: None,
        routing_bucket: None,
        deadline: None,
        cancel: None,
    }
}

/// One call of a function, waiting to be awaited: what [`call`] returns.
///
/// It is [`IntoFuture`], so `.await` runs it, and the future is `Send` and
/// `'static`, so a test may also spawn it.
#[must_use = "a call does nothing until it is awaited"]
pub struct Call<F: Function> {
    f: F,
    input: Value,
    message: Option<Message>,
    layers: Vec<Arc<dyn CallLayer>>,
    exprs: Option<ExprEngine>,
    name: String,
    capture: bool,
    channel: Option<String>,
    routing_bucket: Option<u8>,
    deadline: Option<Instant>,
    cancel: Option<CancelToken>,
}

impl<F: Function> Call<F> {
    /// The message the call runs over. By default an empty one: a `null`
    /// payload and empty roots.
    pub fn message(mut self, m: Message) -> Self {
        self.message = Some(m);
        self
    }

    /// Adds a layer around the call, as a run's layers wrap a call. The
    /// first given is the outermost.
    pub fn layer<L: CallLayer>(mut self, l: L) -> Self {
        self.layers.push(Arc::new(l));
        self
    }

    /// The engine the config compiles with and the call evaluates with,
    /// secrets included. By default [`ExprEngine::default`].
    pub fn exprs(mut self, e: ExprEngine) -> Self {
        self.exprs = Some(e);
        self
    }

    /// The name the function is registered and called by: what [`Prepare`](super::Prepare)
    /// and [`CallInfo::function`] say. By default `"function"`; a built-in's
    /// name is reserved, as [`Registry::register`] reserves it.
    pub fn name(mut self, n: &str) -> Self {
        n.clone_into(&mut self.name);
        self
    }

    /// Whether the call records its `Change`s, as a run's
    /// [`capture_changes`](method@crate::RunOptions::capture_changes) says.
    /// On by default.
    pub fn capture_changes(mut self, on: bool) -> Self {
        self.capture = on;
        self
    }

    /// The channel the call's run was started on, as
    /// [`CallInfo::channel`] says. By default none.
    pub fn channel(mut self, c: &str) -> Self {
        self.channel = Some(c.to_owned());
        self
    }

    /// The run's routing bucket, as [`CallInfo::routing_bucket`] says. By
    /// default none.
    pub fn routing_bucket(mut self, b: u8) -> Self {
        self.routing_bucket = Some(b);
        self
    }

    /// The run's deadline, as [`Ctx::deadline`](super::Ctx::deadline) says.
    /// The call is checked against it and, natively, raced against it, as
    /// a run's host call is: one it passes ends `Err` of kind
    /// [`TimedOut`](ErrorKind::TimedOut), its writes dropped. A deadline
    /// needs a Tokio runtime with its time driver. By default none.
    pub fn deadline(mut self, at: Instant) -> Self {
        self.deadline = Some(at);
        self
    }

    /// The run's cancellation token, as [`Ctx::cancel`](super::Ctx::cancel)
    /// says. The call is checked against it and, natively, raced against
    /// it, as a run's host call is: one it cancels ends `Err` of kind
    /// [`Cancelled`](ErrorKind::Cancelled), its writes dropped. By default
    /// none.
    pub fn cancel(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// Compiles the config, runs the call, and settles it.
    async fn run(self) -> Called {
        let Self {
            f,
            input,
            message,
            layers,
            exprs,
            name,
            capture,
            channel,
            routing_bucket,
            deadline,
            cancel,
        } = self;
        let mut message = message.unwrap_or_else(|| Message::new(OwnedDataValue::Null));
        // Registered as a host registers it, so `register`'s checks of the
        // spec run here too.
        let mut registry = Registry::new(exprs.unwrap_or_default());
        registry.register(&name, f);
        let (bound, issues) = compile(&registry, &name, &input);
        let Some(bound) = bound else {
            return Called {
                issues,
                outcome: Err(Error::new(
                    ErrorKind::Compile,
                    format!("the config of `{name}` was rejected; `issues` says why"),
                )),
                writes: Vec::new(),
                changes: Vec::new(),
                errors: Vec::new(),
                message,
            };
        };
        // The clock is read once, as a run reads it, for the records' stamp.
        let now_text: Arc<str> = Arc::from(Utc::now().to_rfc3339());
        let parts = message.parts();
        let mut rc = RunContext::new(parts.context, Tuning::default());
        let Invoked {
            result,
            overlay,
            changes,
            errors,
            ..
        } = {
            let reader = rc.reader();
            let start = CallStart {
                reader,
                binding: None,
                payload: parts.payload,
                exprs: registry.exprs(),
                info: CallInfo {
                    program: ProgramId::next(),
                    workflow: WorkflowIx::new(0),
                    task: TaskIx::new(0),
                    function: &name,
                    workflow_id: TEST,
                    task_id: TEST,
                    channel: channel.as_deref(),
                    routing_bucket,
                    loop_counter: None,
                    element_index: None,
                },
                deadline,
                cancel: cancel.as_ref(),
                capture,
            };
            let layers = Layers {
                run: &layers,
                registry: &[],
            };
            invoke(start, &*bound, layers, None, crate::run::timers_available()).await
        };
        // The settle, as a run's: the records the call added are stamped and
        // pushed onto the message, and an `Ok` replays the overlay and keeps
        // the `Change`s while an `Err` drops both. The settle takes the
        // overlay, so `writes` is a copy of it.
        let outcome = result.into_result();
        let ok = outcome.is_ok();
        let writes = if ok {
            overlay.clone().into_writes()
        } else {
            Vec::new()
        };
        let test_id: Arc<str> = Arc::from(TEST);
        let ids = TaskIds {
            workflow: &test_id,
            task: &test_id,
        };
        let errors_before = parts.errors.len();
        keep_records(ids, &now_text, parts.errors, errors, None);
        let errors = parts.errors[errors_before..].to_vec();
        let changes = apply_writes(&mut rc, ok, overlay, changes);
        rc.finish();
        Called {
            issues,
            outcome,
            writes,
            changes,
            errors,
            message,
        }
    }
}

impl<F: Function> IntoFuture for Call<F> {
    type Output = Called;
    type IntoFuture = BoxFuture<'static, Called>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

impl<F: Function> fmt::Debug for Call<F> {
    /// The name, the input, the layer count, whether changes are captured,
    /// the channel and the deadline; not the function, the layers or the
    /// token.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Call")
            .field("name", &self.name)
            .field("input", &self.input)
            .field("layers", &self.layers.len())
            .field("capture_changes", &self.capture)
            .field("channel", &self.channel)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

/// What one [`call`] did.
#[derive(Debug)]
#[non_exhaustive]
pub struct Called {
    /// Every issue compiling the config found, at its path below the input,
    /// each stamped with the task `"test"`. Advisories and Defects may sit
    /// beside an `Ok`.
    pub issues: Vec<Issue>,
    /// What the call returned: through the layers, as a run sees it. `Err`
    /// of kind [`Compile`](ErrorKind::Compile) when the config was rejected
    /// and nothing ran.
    pub outcome: Result<Outcome>,
    /// The call's writes, in the order it made them. Empty after an `Err`,
    /// which drops them.
    pub writes: Vec<(Path, Write)>,
    /// The call's `Change`s, as the call recorded them: a write at a root
    /// says [`replaced`](Change::replaced), as a trace's copy does, where
    /// the task's audit entry, which keeps v3's bytes, does not. Empty after
    /// an `Err`, or without capture.
    pub changes: Vec<Change>,
    /// The records the call added, kept whatever it returned, stamped as a
    /// run stamps them: the ids (`test` for both) and a timestamp, where
    /// they carry none.
    pub errors: Vec<ErrorRecord>,
    /// The message after the settle: the writes applied on `Ok`, the records
    /// appended to its errors. Its audit is untouched, since no recorder ran.
    pub message: Message,
}

/// A config compiled alone, against [`ExprEngine::default`], with nothing
/// prepared or checked beyond the config's own compile. `Err` carries every
/// issue, at its path below the input.
///
/// # Errors
///
/// Every issue the compile found, when any is Rejected or the config
/// refused its input.
pub fn config<C: FunctionConfig>(input: Value) -> std::result::Result<C, Vec<Issue>> {
    config_with(&ExprEngine::default(), input)
}

/// A config compiled alone, as [`config`] compiles it, against `exprs`: the
/// host's engine, with its own operators and secrets.
///
/// # Errors
///
/// Every issue the compile found, when any is Rejected or the config
/// refused its input.
pub fn config_with<C: FunctionConfig>(
    exprs: &ExprEngine,
    input: Value,
) -> std::result::Result<C, Vec<Issue>> {
    let mut issues = Vec::new();
    let compiled = {
        let mut cx = CompileCx::new(exprs, &mut issues);
        let before = cx.rejected();
        let compiled = C::compile(&input, &mut cx).filter(|_| cx.rejected() == before);
        if compiled.is_none() && cx.rejected() == before {
            refused(&mut cx, std::any::type_name::<C>());
        }
        compiled
    };
    compiled.ok_or(issues)
}

/// Compiles `name`'s config from `input` through the compiler's own
/// [`compile_callee`], as a task's host function at workflow and task
/// `"test"`, walked with the checks a workflow's compile runs. The bound
/// call, unless anything Rejected was found, and every issue.
fn compile(
    registry: &Registry,
    name: &str,
    input: &Value,
) -> (Option<Box<dyn BoundCall>>, Vec<Issue>) {
    let entry = registry.entry(name);
    let Some(entry @ Entry::Host(_)) = entry else {
        unreachable!("`{name}` was registered as a host function");
    };
    let mut issues = Vec::new();
    let at = CalleeAt {
        name,
        workflow_id: TEST,
        step: (StepKind::Task, TEST),
        input_at: &AuthoredPath::root(),
    };
    let mut checks = Checks::new(registry.exprs());
    let callee = compile_callee(
        registry.exprs(),
        entry,
        input,
        &at,
        &mut checks,
        &mut issues,
    );
    let rejected = any_rejected(&issues);
    let bound = match callee {
        Some(Callee::Host(bound)) if !rejected => Some(bound),
        _ => None,
    };
    (bound, issues)
}
