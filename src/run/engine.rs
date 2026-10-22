//! The engine: a [`Program`] and its config, run over messages.

use super::RunStatus;
use super::budget::{ConditionErrorPolicy, TaskBudget};
use super::builder::EngineBuilder;
use super::cancel::{Limits, timers_available};
use super::error_context::ErrorContext;
use super::interp::{CaptureMode, Exit, Frame, RunEnv, Watch, run_workflow};
use super::message::{Message, finish_audit, keeps_entries};
use super::observers::Observers;
use super::options::RunOptions;
use super::record;
use super::report::{Record, Run};
use super::trace::Capture;
use crate::def::Rollout;
use crate::expr::Path;
use crate::observe::{
    Event, Observer, Selection, StepResult, TraceStep, WorkflowExit, WorkflowRecord,
};
use crate::program::{Compiler, Program};
use crate::state::{RunContext, Tuning};
use chrono::Utc;
use datavalue::OwnedDataValue;
use std::fmt;
use std::sync::Arc;

/// What an [`Engine`] is configured with beside its program. A reload,
/// [`Engine::with_program`], keeps it.
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct EngineConfig {
    /// The observer every run of the engine reports to, beside a run's own
    /// ([`RunOptions::observer`](RunOptions::observer())). `None` by
    /// default.
    pub observer: Option<Arc<dyn Observer>>,
    /// Where the records of each call that added any are mirrored in the
    /// context, and how many are kept. `None` by default: nothing is
    /// mirrored, and nothing is reset at run start.
    pub error_context: Option<ErrorContext>,
    /// Engine-wide default for how condition evaluation errors are handled
    /// when [`RunOptions::condition_error_policy`] is `None`. Defaults to
    /// [`ConditionErrorPolicy::Skip`].
    pub condition_error_policy: ConditionErrorPolicy,
}

impl EngineConfig {
    /// Reports every run of the engine to `o`.
    #[must_use]
    pub fn observer(mut self, o: Arc<dyn Observer>) -> Self {
        self.observer = Some(o);
        self
    }

    /// Mirrors every run's failure records by `ec`.
    #[must_use]
    pub fn error_context(mut self, ec: ErrorContext) -> Self {
        self.error_context = Some(ec);
        self
    }

    /// Sets the engine-wide default for how condition evaluation errors are
    /// handled.
    #[must_use]
    pub fn condition_error_policy(mut self, policy: ConditionErrorPolicy) -> Self {
        self.condition_error_policy = policy;
        self
    }

    /// Alias for [`Self::condition_error_policy`].
    #[must_use]
    pub fn with_condition_error_policy(self, policy: ConditionErrorPolicy) -> Self {
        self.condition_error_policy(policy)
    }
}

impl fmt::Debug for EngineConfig {
    /// The observer as a presence, and the error context:
    /// `EngineConfig { observer: present, error_context: None, .. }`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let observer = if self.observer.is_some() {
            "present"
        } else {
            "absent"
        };
        f.debug_struct("EngineConfig")
            .field("observer", &format_args!("{observer}"))
            .field("error_context", &self.error_context)
            .finish_non_exhaustive()
    }
}

/// A program and its config: what runs messages.
///
/// An `Arc` inside, so cloning is cheap and a clone runs the same program.
/// [`with_program`](Self::with_program) is a reload: the engine it returns
/// runs the new program, and this one, with any run in flight, keeps the old.
///
/// ```
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// use dataflow_rs::datavalue::OwnedDataValue;
/// use dataflow_rs::{Engine, Message};
/// use serde_json::json;
///
/// let engine = Engine::builder()
///     .workflow_json(&json!({"id": "w", "tasks": [
///         {"id": "t", "function": {"name": "map", "input": {
///             "mappings": [{"path": "data.greeting", "logic": "hello"}]}}}
///     ]}))
///     .build()
///     .unwrap();
/// let mut message = Message::from_json(&json!({}));
/// let run = engine.run(&mut message).await;
/// assert_eq!(run.matched(), 1);
/// let greeting = OwnedDataValue::from(&json!({"greeting": "hello"}));
/// assert_eq!(message.context().data(), &greeting);
/// # });
/// ```
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

struct Inner {
    program: Program,
    config: EngineConfig,
}

/// The `metadata` keys the engine writes on every run: the progress after
/// every call, which cross-workflow chaining reads, and the run's stamps,
/// written before any op.
pub(crate) mod slots {
    pub(crate) const PROGRESS: &str = "progress";
    pub(crate) const PROCESSED_AT: &str = "processed_at";
    pub(crate) const ENGINE_VERSION: &str = "engine_version";
    pub(crate) const CHANNEL: &str = "channel";
    /// Every one of them.
    pub(crate) const ALL: [&str; 4] = [PROGRESS, PROCESSED_AT, ENGINE_VERSION, CHANNEL];
}

/// `metadata.<key>`.
pub(crate) fn metadata(key: &str) -> Path {
    format!("metadata.{key}")
        .parse()
        .expect("a metadata key is a path")
}

impl Engine {
    /// An engine running `program` with `cfg`.
    pub fn new(program: Program, cfg: EngineConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                program,
                config: cfg,
            }),
        }
    }

    /// A builder starting from the default registry, for a host that does
    /// not quarantine: its `build` fails if any workflow is rejected.
    pub fn builder() -> EngineBuilder {
        EngineBuilder::new()
    }

    /// Runs `msg` with [`RunOptions::default`].
    pub async fn run(&self, msg: &mut Message) -> Run {
        // Borrowed for the whole run: a temporary would make the run's
        // future carry the options.
        static DEFAULT: RunOptions<'static> = RunOptions::DEFAULT;
        self.run_with(msg, &DEFAULT).await
    }

    /// Runs `msg` through the program's Active workflows, those of
    /// `opts.channel` when it names one, in priority order.
    ///
    /// The run reads the clock once: `metadata.processed_at`, every audit
    /// entry and every error record carry that reading. Only an observer
    /// asking for [`TIMINGS`](crate::observe::Interest::TIMINGS), or a trace
    /// asking for [`timings`](crate::observe::TraceOptions::timings()), has the
    /// clock read again, for the elapsed times it is given. It writes
    /// `metadata.engine_version`, and `metadata.channel` under a channel,
    /// before any workflow runs, and then, with an [`ErrorContext`]
    /// configured, removes its path's key: the records there are the run's
    /// own. A workflow whose rollout does not admit `opts.routing_bucket` is
    /// skipped; no bucket is admitted by every rollout. A workflow that stops
    /// the run ends it, and the [`Run`] says how:
    /// [`TimedOut`](RunStatus::TimedOut) once `opts.deadline` passed,
    /// [`Cancelled`](RunStatus::Cancelled) once `opts.cancel` was cancelled,
    /// and otherwise [`Stopped`](RunStatus::Stopped) with the error that
    /// stopped it, whatever its kind: a child run's timeout that a function
    /// returns before this run's own deadline passed is its task's failure,
    /// not this run's. An `Err` a function returns once `opts.deadline` has
    /// passed, or `opts.cancel` was cancelled, is the run's limit's.
    ///
    /// With [`opts.trace`](RunOptions::trace()) set, the [`Run`] carries the
    /// run's [`Trace`](crate::observe::Trace): the context after the stamps
    /// and the reset, then a step for every recorded call, skipped call or
    /// workflow and sweep, the step that stopped the run included: the call
    /// a limit cut short, or, for a stop as a sweep starts, the engine's
    /// step for the counter it wrote. Its replay is the message's context on
    /// every status.
    pub async fn run_with(&self, msg: &mut Message, opts: &RunOptions<'_>) -> Run {
        let program = &self.inner.program;
        // Built in a block: a local the run borrowed would stay in its
        // future beside the `Watch` it moved into.
        let mut watch = {
            let observers = Observers::attach(self.inner.config.observer.as_deref(), opts.observer);
            Watch {
                timed: observers.timed_tasks(),
                observers,
                trace: None,
                error_context: self.inner.config.error_context.as_ref(),
            }
        };
        let started = watch.observers.timed().then(record::now);
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let mut now_text: Option<Arc<str>> = None;
        let parts = msg.parts();
        stamp(parts.context, now_str, opts.channel);
        // A message brings no records in at the error context's path.
        if let Some(ec) = &self.inner.config.error_context {
            ec.reset(parts.context);
        }
        // `initial` is the context as the stamps and the reset left it.
        if let Some(options) = &opts.trace {
            let trace = Capture::new(options.clone(), parts.context);
            watch.timed |= trace.timings();
            watch.trace = Some(Box::new(trace));
        }

        let mut rc = RunContext::new(parts.context, Tuning::default());
        let active = program.active(opts.channel);
        watch.observers.emit(Event::RunStarted {
            program: program.id(),
            message_id: parts.id,
            channel: opts.channel,
            considered: active.len(),
        });
        // A call's changes are worth recording only in a mode that keeps
        // an entry for them.
        let capture_audit = opts.capture_changes && keeps_entries(opts.audit);
        let mut records = Vec::with_capacity(active.len());
        // Every 64th op of the run checks the limits, across workflows.
        let mut ops = 0;
        // Whether this run's runtime can race a deadline with a timer: the
        // runtime context is the run's, so the answer is the run's.
        let timers = opts.deadline.is_some() && timers_available();
        // A bucket above 99 is served as 99, as in 3.x: the bucket space is
        // `0..100`, and `[x, 100)` is the range that serves the rest.
        let bucket = opts.routing_bucket.map(|b| b.min(Rollout::MAX_BUCKET));
        let condition_error_policy = opts
            .condition_error_policy
            .unwrap_or(self.inner.config.condition_error_policy);
        let budget = TaskBudget::new(opts.task_budget, opts.expr_ops_budget, opts.accountant);
        // How the run ended, once a workflow stopped it: the loop's value,
        // so the run's future holds no status across its workflows.
        let stopped = 'run: {
            for &ix in active {
                let wf = program.compiled(ix);
                if let (Some(rollout), Some(bucket)) = (wf.rollout, bucket)
                    && !rollout.accepts(bucket)
                {
                    if let Some(trace) = watch.trace.as_deref_mut() {
                        let (workflow_id, _) = program.ids(ix);
                        trace.push(TraceStep::new(ix, workflow_id, StepResult::Skipped));
                    }
                    records.push(Record {
                        ix,
                        selection: Selection::SkippedByRollout,
                        sweeps: 0,
                        exit: None,
                    });
                    continue;
                }
                let (workflow_id, task_ids) = program.ids(ix);
                let mut env = RunEnv {
                    program: program.id(),
                    workflow_ix: ix,
                    task_base: program.task_base(ix),
                    channel: opts.channel,
                    routing_bucket: bucket,
                    layers: opts.layers,
                    limits: Limits {
                        deadline: opts.deadline,
                        cancel: opts.cancel,
                        timers,
                    },
                    condition_error_policy,
                    budget: &budget,
                    ops,
                    exprs: program.registry().exprs(),
                    payload: parts.payload,
                    errors: &mut *parts.errors,
                    audit: &mut *parts.audit,
                    mode: opts.audit,
                    capture: CaptureMode::new(capture_audit, &watch),
                    watch: &mut watch,
                    now,
                    now_text: &mut now_text,
                    changes: Vec::new(),
                    workflow_id,
                    task_ids,
                    failed: false,
                    frame: Frame::default(),
                };
                let (exit, selection) = run_workflow(&mut rc, wf, &mut env).await;
                ops = env.ops;
                let ran = matches!(selection, Selection::Ran);
                let (exit, stop) = match exit {
                    Exit::Completed => (WorkflowExit::Completed, None),
                    Exit::Halted => (WorkflowExit::Halted, None),
                    Exit::Failed => (WorkflowExit::Failed, None),
                    Exit::Stopped(e) => (WorkflowExit::Stopped, Some(RunStatus::Stopped(e))),
                    // Only the run's own limits make it `TimedOut` or
                    // `Cancelled`: a task's `Err` of either kind, a child run's
                    // propagated, stops it as any failure does, unless one of
                    // them had passed when the call returned it.
                    Exit::Limited(stop) => (WorkflowExit::Stopped, Some(stop.status())),
                };
                let considered = Record {
                    ix,
                    selection,
                    sweeps: env.frame.sweeps,
                    exit: ran.then_some(exit),
                };
                let workflow_started = env.frame.started;
                if ran && watch.observers.workflows() {
                    let view = WorkflowRecord {
                        id: workflow_id,
                        index: ix,
                        selection: &considered.selection,
                        sweeps: considered.sweeps,
                        exit: considered.exit,
                    };
                    watch.observers.emit(Event::WorkflowFinished {
                        record: &view,
                        elapsed: workflow_started.map(record::since),
                    });
                }
                records.push(considered);
                if let Some(status) = stop {
                    break 'run Some(status);
                }
            }
            None
        };
        let stats = rc.finish();
        finish_audit(parts.audit, opts.audit);
        let status = match stopped {
            Some(status) => status,
            None if !parts.errors.is_empty() => RunStatus::CompletedWithErrors,
            None => RunStatus::Completed,
        };
        watch.observers.emit(Event::RunFinished {
            message_id: parts.id,
            status: &status,
            errors: parts.errors.len(),
            elapsed: started.map(record::since),
        });
        Run::new(
            program.clone(),
            status,
            records,
            stats,
            watch.trace.map(|t| Box::new(t.finish())),
        )
    }

    /// A compiler with the program's registry and options, whose workflows
    /// link into a program this engine can run.
    pub fn compiler(&self) -> Compiler {
        let program = &self.inner.program;
        program.registry().compiler(program.options().clone())
    }

    /// An engine running `program` with this engine's config: a reload. This
    /// engine is unchanged.
    #[must_use]
    pub fn with_program(&self, program: Program) -> Self {
        Self::new(program, self.inner.config.clone())
    }

    /// The program this engine runs.
    pub fn program(&self) -> &Program {
        &self.inner.program
    }

    /// The config this engine runs with.
    #[doc(hidden)]
    pub fn config(&self) -> &EngineConfig {
        &self.inner.config
    }
}

/// Writes the run's stamps, owned: no view exists yet. Each replaces its key
/// in place or is appended, as v3's `set_processing_metadata`.
fn stamp(context: &mut OwnedDataValue, now: String, channel: Option<&str>) {
    let OwnedDataValue::Object(roots) = context else {
        return;
    };
    let Some((_, OwnedDataValue::Object(meta))) = roots.iter_mut().find(|(k, _)| k == "metadata")
    else {
        return;
    };
    let need = 2 + usize::from(channel.is_some());
    if meta.capacity() < meta.len() + need {
        meta.reserve(need);
    }
    match meta.iter_mut().find(|(k, _)| k == slots::PROCESSED_AT) {
        Some((_, slot)) => *slot = OwnedDataValue::String(now),
        None => meta.push((slots::PROCESSED_AT.to_owned(), OwnedDataValue::String(now))),
    }
    put_meta_str(meta, slots::ENGINE_VERSION, crate::VERSION);
    if let Some(channel) = channel {
        put_meta_str(meta, slots::CHANNEL, channel);
    }
}

fn put_meta_str(meta: &mut Vec<(String, OwnedDataValue)>, key: &str, text: &str) {
    match meta.iter_mut().find(|(k, _)| k == key) {
        Some((_, OwnedDataValue::String(s))) => {
            if s != text {
                s.clear();
                s.push_str(text);
            }
        }
        Some((_, slot)) => *slot = OwnedDataValue::String(text.to_owned()),
        None => meta.push((key.to_owned(), OwnedDataValue::String(text.to_owned()))),
    }
}

impl fmt::Debug for Engine {
    /// The program and the config.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("program", &self.inner.program)
            .field("config", &self.inner.config)
            .finish()
    }
}
