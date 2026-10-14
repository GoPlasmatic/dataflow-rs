//! Linking: compiled workflows into a [`Program`], the immutable thing the
//! runtime executes.

use super::compile::{CompileOptions, CompiledWorkflow, Compiler};
use super::fingerprint::Fingerprint;
use crate::base::{AuthoredPath, ProgramId, StepKind, TaskIx, WorkflowIx};
use crate::def::Status;
use crate::function::Registry;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

impl Compiler {
    /// Links compiled workflows into a [`Program`]. It never fails.
    ///
    /// Workflows sort by `priority`, ascending, with input order breaking
    /// ties; the sorted position is the [`WorkflowIx`]. Tasks are numbered
    /// from zero over every workflow's task table in that order, setup first,
    /// so a workflow's tasks are contiguous. Workflows sharing an id are all
    /// kept, as v3 keeps them.
    ///
    /// # Panics
    ///
    /// When a workflow was compiled under another registry: its functions,
    /// operators and secrets need not be the program's. A registry and its
    /// clones are one registry, so every compiler made from them links the
    /// others' workflows, until one of them registers a function or a layer,
    /// which gives it a table of its own.
    ///
    /// ```
    /// # extern crate dataflow_rs_v4 as dataflow_rs;
    /// use dataflow_rs::Registry;
    /// use dataflow_rs::def::{ParseOptions, parse};
    /// use dataflow_rs::program::CompileOptions;
    /// use serde_json::json;
    ///
    /// let compiler = Registry::default().compiler(CompileOptions::default());
    /// let json = json!({"id": "w", "tasks": [
    ///     {"id": "t", "function": {"name": "log", "input": {"message": "hi"}}}
    /// ]});
    /// let def = parse(&json, ParseOptions::default()).def.unwrap();
    /// let program = compiler.link([compiler.compile_one(&def).unwrap().workflow]);
    /// assert_eq!(program.workflows()[0].id, "w");
    /// assert_eq!(program.tasks()[0].id, "t");
    /// ```
    pub fn link(&self, workflows: impl IntoIterator<Item = CompiledWorkflow>) -> Program {
        let mut workflows: Vec<CompiledWorkflow> = workflows.into_iter().collect();
        for w in &workflows {
            assert!(
                w.registry.ptr_eq(&self.registry),
                "cannot link workflow `{}`: it was compiled under a different registry from \
                 this compiler's",
                w.id
            );
        }
        workflows.sort_by_key(|w| w.priority);
        let count =
            u32::try_from(workflows.len()).expect("a program holds at most u32::MAX workflows");

        let mut infos = Vec::with_capacity(workflows.len());
        let mut workflow_ids = Vec::with_capacity(workflows.len());
        let mut tasks = Vec::new();
        let mut task_ids = Vec::new();
        let mut task_base = Vec::with_capacity(workflows.len());
        let mut all = Vec::new();
        let mut by_channel: HashMap<String, Vec<WorkflowIx>> = HashMap::new();
        for (ix, w) in (0..count).map(WorkflowIx::new).zip(&workflows) {
            task_base
                .push(u32::try_from(tasks.len()).expect("a program holds at most u32::MAX tasks"));
            workflow_ids.push(Arc::from(w.id.as_str()));
            infos.push(WorkflowInfo {
                ix,
                id: w.id.clone(),
                channel: w.channel.clone(),
                priority: w.priority,
                status: w.status,
                version: w.version,
                fingerprint: w.fingerprint,
            });
            for meta in &w.tasks {
                let task = TaskIx::new(
                    u32::try_from(tasks.len()).expect("a program holds at most u32::MAX tasks"),
                );
                task_ids.push(Arc::from(meta.id.as_str()));
                tasks.push(TaskInfo {
                    ix: task,
                    id: meta.id.clone(),
                    function: meta.function.clone(),
                    workflow: ix,
                    kind: meta.kind,
                    path: meta.path.clone(),
                });
            }
            if w.status == Status::Active {
                all.push(ix);
                by_channel.entry(w.channel.clone()).or_default().push(ix);
            }
        }

        Program {
            inner: Arc::new(Inner {
                id: ProgramId::next(),
                registry: self.registry.clone(),
                options: self.options.clone(),
                workflows,
                infos,
                workflow_ids,
                tasks,
                task_ids,
                task_base,
                all,
                by_channel,
            }),
        }
    }
}

struct Inner {
    id: ProgramId,
    registry: Registry,
    /// What the program was compiled with, for [`Engine::compiler`](crate::run::Engine::compiler).
    options: CompileOptions,
    workflows: Vec<CompiledWorkflow>,
    infos: Vec<WorkflowInfo>,
    /// Each workflow's id, shared by the audit entries a run records.
    workflow_ids: Vec<Arc<str>>,
    tasks: Vec<TaskInfo>,
    /// Each task's id, indexed by [`TaskIx`], shared likewise.
    task_ids: Vec<Arc<str>>,
    task_base: Vec<u32>,
    /// Every Active workflow, in `WorkflowIx` order.
    all: Vec<WorkflowIx>,
    /// Each channel's Active workflows, in `WorkflowIx` order.
    by_channel: HashMap<String, Vec<WorkflowIx>>,
}

/// Compiled workflows linked together: immutable, with dense indices.
///
/// An `Arc` inside, so cloning is cheap and the program is `Send + Sync`.
/// [`Compiler::link`] makes one.
#[derive(Clone)]
pub struct Program {
    inner: Arc<Inner>,
}

impl Program {
    /// This program's identity, distinct from every other link's.
    pub fn id(&self) -> ProgramId {
        self.inner.id
    }

    /// Every linked workflow, indexed by [`WorkflowIx`]: the Paused and
    /// Archived ones too.
    pub fn workflows(&self) -> &[WorkflowInfo] {
        &self.inner.infos
    }

    /// Every task of every workflow, indexed by [`TaskIx`].
    pub fn tasks(&self) -> &[TaskInfo] {
        &self.inner.tasks
    }

    /// The task at `ix`.
    ///
    /// # Panics
    ///
    /// When `ix` belongs to another program and is out of range.
    pub fn task(&self, ix: TaskIx) -> &TaskInfo {
        &self.inner.tasks[ix.index() as usize]
    }

    /// The registry the program's functions resolve in.
    pub fn registry(&self) -> &Registry {
        &self.inner.registry
    }

    /// The compiled workflow at `ix`.
    pub(crate) fn compiled(&self, ix: WorkflowIx) -> &CompiledWorkflow {
        &self.inner.workflows[ix.index() as usize]
    }

    /// The Active workflows, in `WorkflowIx` order: those on `channel`, or
    /// every one for `None`. An unknown channel has none.
    pub(crate) fn active(&self, channel: Option<&str>) -> &[WorkflowIx] {
        match channel {
            None => &self.inner.all,
            Some(c) => self.inner.by_channel.get(c).map_or(&[], Vec::as_slice),
        }
    }

    /// The `TaskIx` of the workflow's first task.
    pub(crate) fn task_base(&self, ix: WorkflowIx) -> u32 {
        self.inner.task_base[ix.index() as usize]
    }

    /// The workflow's id and its tasks' ids, by task row, as the shared
    /// strings an audit entry holds: cloning one is a count, not a copy.
    pub(crate) fn ids(&self, ix: WorkflowIx) -> (&Arc<str>, &[Arc<str>]) {
        let base = self.task_base(ix) as usize;
        let count = self.compiled(ix).tasks.len();
        (
            &self.inner.workflow_ids[ix.index() as usize],
            &self.inner.task_ids[base..base + count],
        )
    }

    /// The options the program was compiled with.
    pub(crate) fn options(&self) -> &CompileOptions {
        &self.inner.options
    }
}

impl fmt::Debug for Program {
    /// The id and the workflow ids, in order.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Program")
            .field("id", &self.inner.id)
            .field(
                "workflows",
                &self.inner.infos.iter().map(|w| &w.id).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

/// What a [`Program`] lists about one workflow.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct WorkflowInfo {
    /// Its position in the program.
    pub ix: WorkflowIx,
    /// Its id.
    pub id: String,
    /// The channel it runs on.
    pub channel: String,
    /// Its priority: lower runs first.
    pub priority: u32,
    /// Its lifecycle status.
    pub status: Status,
    /// Its version.
    pub version: u32,
    /// The operators and data keys its expressions use.
    pub fingerprint: Fingerprint,
}

/// What a [`Program`] lists about one task.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct TaskInfo {
    /// Its position in the program.
    pub ix: TaskIx,
    /// Its id.
    pub id: String,
    /// The registered name of the function it calls.
    pub function: String,
    /// The workflow it belongs to.
    pub workflow: WorkflowIx,
    /// Whether it is a setup step or a body task.
    pub kind: StepKind,
    /// Where it sits in the authored workflow.
    pub path: AuthoredPath,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::{ParseOptions, parse};
    use serde_json::{Value, json};

    fn workflow(id: &str, channel: &str, status: &str, tasks: usize) -> Value {
        let tasks: Vec<Value> = (0..tasks)
            .map(|i| json!({"id": format!("t{i}"), "function": {"name": "log", "input": {"message": "m"}}}))
            .collect();
        json!({"id": id, "channel": channel, "status": status, "tasks": tasks})
    }

    fn link(defs: &[Value]) -> Program {
        let c = Registry::default().compiler(CompileOptions::default());
        c.link(defs.iter().map(|d| {
            let def = parse(d, ParseOptions::default()).def.expect("parses");
            c.compile_one(&def).expect("compiles").workflow
        }))
    }

    fn ixs(list: &[WorkflowIx]) -> Vec<u32> {
        list.iter().map(|ix| ix.index()).collect()
    }

    #[test]
    fn active_lists_split_by_channel_and_status() {
        let program = link(&[
            workflow("a", "a", "active", 1),
            workflow("b", "b", "active", 1),
            workflow("p", "a", "paused", 1),
            workflow("d", "default", "active", 1),
            workflow("x", "default", "archived", 1),
            workflow("a2", "a", "active", 1),
        ]);
        assert_eq!(program.workflows().len(), 6);
        assert_eq!(ixs(program.active(Some("a"))), [0, 5]);
        assert_eq!(ixs(program.active(Some("b"))), [1]);
        assert_eq!(ixs(program.active(Some("default"))), [3]);
        assert_eq!(ixs(program.active(None)), [0, 1, 3, 5]);
        assert!(program.active(Some("zzz")).is_empty());
    }

    #[test]
    fn tasks_of_inactive_workflows_still_have_indices() {
        let program = link(&[
            workflow("a", "x", "paused", 2),
            workflow("b", "x", "active", 3),
            workflow("e", "x", "active", 1),
        ]);
        let bases: Vec<u32> = (0..3)
            .map(|i| program.task_base(WorkflowIx::new(i)))
            .collect();
        assert_eq!(bases, [0, 2, 5]);
        assert_eq!(program.tasks().len(), 6);
        let b = WorkflowIx::new(1);
        assert_eq!(program.compiled(b).id(), "b");
        assert_eq!(program.task(TaskIx::new(program.task_base(b))).id, "t0");
        assert_eq!(program.task(TaskIx::new(program.task_base(b))).workflow, b);
    }
}
