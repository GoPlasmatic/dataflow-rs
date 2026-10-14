//! The engine: a [`Program`] and its config, run over messages.

use super::builder::EngineBuilder;
use super::interp::{Exit, Frame, RunEnv, run_workflow};
use super::message::{AuditMode, Message, finish_audit};
use super::options::RunOptions;
use super::report::{Record, Run};
use crate::expr::Path;
use crate::observe::{RunStatus, Selection, WorkflowExit};
use crate::program::{Compiler, Program};
use crate::state::{RunContext, Tuning};
use chrono::Utc;
use datavalue::OwnedDataValue;
use std::fmt;
use std::sync::{Arc, LazyLock};

/// What an [`Engine`] is configured with beside its program. Phase 6 adds
/// the observer and the error context.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct EngineConfig {}

/// A program and its config: what runs messages.
///
/// An `Arc` inside, so cloning is cheap and a clone runs the same program.
/// [`with_program`](Self::with_program) is a reload: the engine it returns
/// runs the new program, and this one, with any run in flight, keeps the old.
///
/// ```
/// # extern crate dataflow_rs_v4 as dataflow_rs;
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

/// The run's stamps under `metadata`, written before any op.
static PROCESSED_AT: LazyLock<Path> = LazyLock::new(|| metadata("processed_at"));
static ENGINE_VERSION: LazyLock<Path> = LazyLock::new(|| metadata("engine_version"));
static CHANNEL: LazyLock<Path> = LazyLock::new(|| metadata("channel"));

fn metadata(key: &str) -> Path {
    format!("metadata.{key}")
        .parse()
        .expect("a metadata key is a path")
}

/// Whether `mode` keeps any entry, so that a call's changes are worth
/// recording (I67).
fn keeps(mode: AuditMode) -> bool {
    !matches!(mode, AuditMode::Off | AuditMode::Last(0))
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
        self.run_with(msg, &RunOptions::default()).await
    }

    /// Runs `msg` through the program's Active workflows, those of
    /// `opts.channel` when it names one, in priority order.
    ///
    /// The run reads the clock once: `metadata.processed_at`, every audit
    /// entry and every error record carry that reading. It writes
    /// `metadata.engine_version`, and `metadata.channel` under a channel,
    /// before any workflow runs. A workflow whose rollout does not admit
    /// `opts.routing_bucket` is skipped; no bucket is admitted by every
    /// rollout. A workflow that stops the run ends it, and the [`Run`] says
    /// how.
    pub async fn run_with(&self, msg: &mut Message, opts: &RunOptions<'_>) -> Run {
        let program = &self.inner.program;
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let parts = msg.parts();
        stamp(parts.context, &now_text, opts.channel);

        let mut rc = RunContext::new(parts.context, Tuning::default());
        let active = program.active(opts.channel);
        let capture = opts.capture_changes && keeps(opts.audit);
        let mut records = Vec::with_capacity(active.len());
        let mut stopped = None;
        for &ix in active {
            let wf = program.compiled(ix);
            if let (Some(rollout), Some(bucket)) = (wf.rollout, opts.routing_bucket)
                && !rollout.accepts(bucket)
            {
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
                exprs: program.registry().exprs(),
                payload: parts.payload,
                errors: &mut *parts.errors,
                audit: &mut *parts.audit,
                mode: opts.audit,
                capture,
                now,
                now_text: &now_text,
                changes: Vec::new(),
                workflow_id,
                task_ids,
                failed: false,
                frame: Frame::default(),
            };
            let (exit, selection) = run_workflow(&mut rc, wf, &mut env).await;
            let ran = matches!(selection, Selection::Ran);
            let (exit, stop) = match exit {
                Exit::Completed => (WorkflowExit::Completed, None),
                Exit::Halted => (WorkflowExit::Halted, None),
                Exit::Failed => (WorkflowExit::Failed, None),
                Exit::Stopped(e) => (WorkflowExit::Stopped, Some(e)),
            };
            records.push(Record {
                ix,
                selection,
                sweeps: env.frame.sweeps,
                exit: ran.then_some(exit),
            });
            if let Some(e) = stop {
                stopped = Some(e);
                break;
            }
        }
        let stats = rc.finish();
        finish_audit(parts.audit, opts.audit);
        let status = match stopped {
            Some(e) => RunStatus::Stopped(e),
            None if !parts.errors.is_empty() => RunStatus::CompletedWithErrors,
            None => RunStatus::Completed,
        };
        Run::new(program.clone(), status, records, stats)
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
}

/// Writes the run's stamps, owned: no view exists yet. Each replaces its key
/// in place or is appended, as v3's `set_processing_metadata`.
fn stamp(context: &mut OwnedDataValue, now: &str, channel: Option<&str>) {
    PROCESSED_AT.assign(context, OwnedDataValue::String(now.to_owned()));
    ENGINE_VERSION.assign(context, OwnedDataValue::String(crate::VERSION.to_owned()));
    if let Some(channel) = channel {
        CHANNEL.assign(context, OwnedDataValue::String(channel.to_owned()));
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
