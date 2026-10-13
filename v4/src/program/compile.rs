//! The compiler: one walk over a definition that compiles every expression
//! and config, reports every issue at its authored path, and fills the side
//! tables the ops index.

use super::checks::{Below, Checks, Hook, Site, Stamp};
use super::ops::{CallSite, Callee, CondSite, FanSite, LoopSite, Op, TaskMeta};
use crate::base::{AuthoredPath, Issue, IssueCode, PathSeg, Severity, StepKind};
use crate::def::{
    self, ForEachDef, Logic, LoopDef, ParseOptions, Rollout, Status, StepDef, TaskDef, WorkflowDef,
    WorkflowFailurePolicy,
};
use crate::expr::{CompileCx, Expr, Path};
use crate::function::{Entry, FunctionSpec, Prepare, Registry};
use datavalue::OwnedDataValue;
use serde_json::Value;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

/// Answers whether a definition that a config names exists, such as a
/// connector a deployment declares.
///
/// A [`Compiler`] given one through [`CompileOptions::resources`] checks each
/// [`Reference`](crate::function::Reference) a host function's spec declares.
/// A field holding a literal string the oracle does not resolve is an
/// `UNRESOLVED_REFERENCE` Defect. A computed or absent value is not checked.
pub trait ResourceOracle: Send + Sync + 'static {
    /// Whether a `kind` named `name` exists: the connector `db1`, say.
    fn resolve(&self, kind: &str, name: &str) -> bool;
}

/// How a [`Compiler`] compiles.
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct CompileOptions {
    /// The oracle the `Reference` fields are checked against. `None`, the
    /// default, checks nothing.
    pub resources: Option<Arc<dyn ResourceOracle>>,
}

impl CompileOptions {
    /// Checks the `Reference` fields against `oracle`.
    #[must_use]
    pub fn resources(mut self, oracle: Arc<dyn ResourceOracle>) -> Self {
        self.resources = Some(oracle);
        self
    }
}

impl Registry {
    /// A compiler for this registry's functions, with `options`.
    pub fn compiler(&self, options: CompileOptions) -> Compiler {
        Compiler {
            registry: self.clone(),
            options,
        }
    }
}

/// Compiles definitions against one [`Registry`], so everything it compiles
/// shares one set of functions and one vocabulary.
///
/// Each workflow compiles on its own: an unregistered function, or any other
/// Rejected issue, rejects only its own workflow.
///
/// ```
/// # extern crate dataflow_rs_v4 as dataflow_rs;
/// use dataflow_rs::Registry;
/// use dataflow_rs::def::{ParseOptions, parse};
/// use dataflow_rs::program::{CompileOptions, IssueCode};
/// use serde_json::json;
///
/// let compiler = Registry::default().compiler(CompileOptions::default());
/// let json = json!({"id": "w", "tasks": [
///     {"id": "t", "function": {"name": "fetch"}}
/// ]});
/// let def = parse(&json, ParseOptions::default()).def.unwrap();
/// let issues = compiler.compile_one(&def).unwrap_err();
/// assert_eq!(issues[0].code, IssueCode::UnknownFunction);
/// assert_eq!(issues[0].path.to_string(), "tasks[0].function.name");
/// assert_eq!(compiler.check(&def), issues);
/// ```
#[derive(Clone)]
pub struct Compiler {
    registry: Registry,
    options: CompileOptions,
}

impl Compiler {
    /// The registry the compiler resolves functions in.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Every issue [`compile_one`](Self::compile_one) finds in `def`, whether
    /// or not it rejects the workflow.
    pub fn check(&self, def: &WorkflowDef) -> Vec<Issue> {
        match self.compile_one(def) {
            Ok(compiled) => compiled.issues,
            Err(issues) => issues,
        }
    }

    /// Every issue in a fragment of steps that would sit at `root` in a
    /// workflow, such as `tasks[2].tasks` or `loop.setup`: its parse issues,
    /// then its compile issues, every path under `root`.
    ///
    /// The fragment is parsed as [`parse`](crate::def::parse) reads a
    /// workflow's steps, with the compat reader on. A root under `loop.setup`
    /// makes setup steps, and each `tasks` key in `root` past the first adds
    /// one group of depth. The steps compile only when nothing in the parse is
    /// Rejected, and a fragment has no workflow condition, loop or workflow
    /// lint. A host function's `prepare` sees an empty workflow id.
    ///
    /// A `root` that is not an authored path is `PARSE_FAILED` at the empty
    /// path, and a fragment that is not an array is `PARSE_FAILED` at `root`.
    pub fn check_steps(&self, steps: &Value, root: &str) -> Vec<Issue> {
        let root = match AuthoredPath::from_str(root) {
            Ok(root) => root,
            Err(error) => {
                return vec![Issue::new(
                    IssueCode::ParseFailed,
                    AuthoredPath::root(),
                    error.to_string(),
                )];
            }
        };
        let (parsed, mut issues) = def::parse_steps(steps, &root, ParseOptions::default());
        if let Some(steps) = parsed {
            let mut checks = Checks::new(self.registry.exprs());
            let mut walk = Walk::new(self, "", &mut checks);
            for (at, kind, step) in def::walk_steps(&steps, &root) {
                walk.step(&at, kind, step);
            }
            issues.append(&mut walk.issues);
        }
        issues
    }

    /// Compiles one workflow.
    ///
    /// `Err` holds every issue, in walk order, when any is Rejected. `Ok`
    /// holds the workflow and its Defects and Advisories, so seeing them
    /// takes no second compile.
    pub fn compile_one(&self, def: &WorkflowDef) -> std::result::Result<Compiled, Vec<Issue>> {
        let mut checks = Checks::new(self.registry.exprs());
        let (workflow, issues) = self.compile_with(def, &mut checks);
        if issues.iter().any(|i| i.severity == Severity::Rejected) {
            Err(issues)
        } else {
            Ok(Compiled { workflow, issues })
        }
    }

    /// The compile walk, calling `hook` at every expression site.
    fn compile_with(
        &self,
        def: &WorkflowDef,
        hook: &mut dyn Hook,
    ) -> (CompiledWorkflow, Vec<Issue>) {
        let mut walk = Walk::new(self, &def.id, hook);
        // 1. The workflow condition.
        if let Some(raw) = &def.condition {
            walk.cond(raw, AuthoredPath::root().join("condition"), None);
        }
        // 2. The loop: `over`, then its slots.
        if let Some(config) = &def.r#loop {
            walk.r#loop(config);
        }
        // 3. Each step: setup first, then the body, pre-order.
        for (at, kind, step) in def.walk() {
            walk.step(&at, kind, step);
        }
        // 4. The workflow lint, `UNGUARDED_VALIDATION`, arrives with the
        //    checks' rules (Task 6).
        let workflow = CompiledWorkflow {
            id: def.id.clone(),
            channel: def.channel.clone(),
            priority: def.priority,
            status: def.status,
            version: def.version,
            rollout: def.rollout,
            on_failure: def.on_failure,
            ops: Vec::new(),
            conds: walk.conds,
            calls: walk.calls,
            fans: walk.fans,
            loops: walk.loops,
            tasks: walk.tasks,
        };
        (workflow, walk.issues)
    }
}

/// A workflow [`Compiler::compile_one`] accepted, and what it found.
#[derive(Debug)]
#[non_exhaustive]
pub struct Compiled {
    /// The compiled workflow.
    pub workflow: CompiledWorkflow,
    /// Its Defects and Advisories, in walk order. None is Rejected.
    pub issues: Vec<Issue>,
}

/// One workflow, compiled and immutable. `Send + Sync`.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the facts are read by link and the runtime")
)]
pub struct CompiledWorkflow {
    pub(crate) id: String,
    pub(crate) channel: String,
    pub(crate) priority: u32,
    pub(crate) status: Status,
    pub(crate) version: u32,
    pub(crate) rollout: Option<Rollout>,
    pub(crate) on_failure: WorkflowFailurePolicy,
    pub(crate) ops: Vec<Op>,
    pub(crate) conds: Vec<CondSite>,
    pub(crate) calls: Vec<CallSite>,
    pub(crate) fans: Vec<FanSite>,
    pub(crate) loops: Vec<LoopSite>,
    pub(crate) tasks: Vec<TaskMeta>,
}

impl CompiledWorkflow {
    /// The workflow's id.
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl fmt::Debug for CompiledWorkflow {
    /// The id, the op count and the size of each side table.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledWorkflow")
            .field("id", &self.id)
            .field("ops", &self.ops.len())
            .field("conds", &self.conds.len())
            .field("calls", &self.calls.len())
            .field("fans", &self.fans.len())
            .field("loops", &self.loops.len())
            .field("tasks", &self.tasks.len())
            .finish_non_exhaustive()
    }
}

/// A table's next index.
fn next(len: usize) -> u32 {
    u32::try_from(len).expect("a workflow holds fewer than 2^32 steps")
}

/// The value at `path` below `value`, if there is one.
fn lookup<'v>(value: &'v Value, path: &AuthoredPath) -> Option<&'v Value> {
    path.segs().iter().try_fold(value, |value, seg| match seg {
        PathSeg::Key(key) => value.get(&**key),
        PathSeg::Index(index) => value.get(*index),
    })
}

/// One compile's state: the issues so far, in walk order, and the side
/// tables, in walk order.
struct Walk<'w> {
    registry: &'w Registry,
    resources: Option<&'w dyn ResourceOracle>,
    /// What `prepare` is told; empty for a fragment.
    workflow_id: &'w str,
    hook: &'w mut dyn Hook,
    issues: Vec<Issue>,
    conds: Vec<CondSite>,
    calls: Vec<CallSite>,
    fans: Vec<FanSite>,
    loops: Vec<LoopSite>,
    tasks: Vec<TaskMeta>,
}

impl<'w> Walk<'w> {
    fn new(compiler: &'w Compiler, workflow_id: &'w str, hook: &'w mut dyn Hook) -> Self {
        Self {
            registry: &compiler.registry,
            resources: compiler.options.resources.as_deref(),
            workflow_id,
            hook,
            issues: Vec::new(),
            conds: Vec::new(),
            calls: Vec::new(),
            fans: Vec::new(),
            loops: Vec::new(),
            tasks: Vec::new(),
        }
    }

    /// Records `issue`, stamped with `step`.
    fn report(&mut self, issue: Issue, step: Stamp<'_>) {
        self.issues.push(match step {
            Some((kind, id)) => issue.with_step(kind, Some(id)),
            None => issue,
        });
    }

    /// Compiles the expression at `at` and, when it compiled, hands it to the
    /// hook. A failure is `TEMPLATE_COMPILE` at `at`, and is not visited.
    fn expr(
        &mut self,
        raw: &Logic,
        at: &AuthoredPath,
        records: bool,
        step: Stamp<'_>,
    ) -> Option<Expr> {
        match self.registry.exprs().compile::<OwnedDataValue>(&raw.0) {
            Ok(expr) => {
                let site = Site { at, records, step };
                self.hook.expr(site, expr.raw(), &mut self.issues);
                Some(expr)
            }
            Err(error) => {
                let issue = Issue::new(IssueCode::TemplateCompile, at.clone(), error.message());
                self.report(issue, step);
                None
            }
        }
    }

    /// A condition at `at`: compiled, visited as not recorded, and kept.
    fn cond(&mut self, raw: &Logic, at: AuthoredPath, step: Stamp<'_>) {
        if let Some(expr) = self.expr(raw, &at, false, step) {
            self.conds.push(CondSite {
                expr,
                path: at,
                step: step.map(|(_, id)| id.to_owned()),
            });
        }
    }

    /// `dotted` as a context path, else `INVALID_PATH` at `at`. Parse has
    /// already checked every binding, so this is defensive.
    fn path(&mut self, dotted: &str, at: AuthoredPath, step: Stamp<'_>) -> Option<Path> {
        match Path::from_str(dotted) {
            Ok(path) => Some(path),
            Err(error) => {
                let issue = Issue::new(IssueCode::InvalidPath, at, error.message());
                self.report(issue, step);
                None
            }
        }
    }

    /// The loop's `over`, recorded, then its slots, each `temp_data.<slot>`.
    fn r#loop(&mut self, config: &LoopDef) {
        let at = AuthoredPath::root().join("loop");
        let over = config
            .over
            .as_ref()
            .and_then(|raw| self.expr(raw, &at.join("over"), true, None));
        let mut slot = |slot: &Option<String>, key: &str| {
            slot.as_ref()
                .and_then(|slot| self.path(&format!("temp_data.{slot}"), at.join(key), None))
        };
        let counter = slot(&config.counter, "counter");
        let item = slot(&config.r#as, "as");
        let scratch = slot(&config.scratch, "scratch");
        self.loops.push(LoopSite {
            init: config.init,
            increment: config.increment,
            max: config.max,
            counter,
            over,
            r#as: item,
            scratch,
        });
    }

    /// One step at `at`, stamped with its kind and id.
    fn step(&mut self, at: &AuthoredPath, kind: StepKind, step: &StepDef) {
        match step {
            StepDef::Group(group) => {
                if let Some(raw) = &group.condition {
                    self.cond(raw, at.join("condition"), Some((kind, &group.id)));
                }
            }
            StepDef::Task(task) => self.task(at, kind, task),
        }
    }

    /// A task: its condition, its fan-out, then its call.
    fn task(&mut self, at: &AuthoredPath, kind: StepKind, task: &TaskDef) {
        let step = (kind, task.id.as_str());
        if let Some(raw) = &task.condition {
            self.cond(raw, at.join("condition"), Some(step));
        }
        if let Some(config) = &task.for_each
            && let Some(fan) = self.fan(config, &at.join("for_each"), step)
        {
            self.fans.push(fan);
        }
        let row = next(self.tasks.len());
        self.tasks.push(TaskMeta {
            id: task.id.clone(),
            function: task.function.name.clone(),
            kind,
            path: at.clone(),
        });
        if let Some(callee) = self.callee(task, at, step) {
            self.calls.push(CallSite {
                task: row,
                callee,
                on_failure: task.on_failure,
                on_rejection: task.on_rejection,
                terminal: task.terminal,
            });
        }
    }

    /// A fan-out at `at`: its `over`, recorded, then its bindings as paths.
    fn fan(
        &mut self,
        config: &ForEachDef,
        at: &AuthoredPath,
        step: (StepKind, &str),
    ) -> Option<FanSite> {
        let over = self.expr(&config.over, &at.join("over"), true, Some(step));
        let item = format!("temp_data.{}", config.r#as);
        let bound = self
            .path(&item, at.join("as"), Some(step))
            .and_then(|item_path| {
                let index = self.path(&format!("{item}_index"), at.join("as"), Some(step))?;
                Some((item_path, index))
            });
        let mut context = |path: &Option<String>, key: &str| {
            path.as_ref()
                .and_then(|path| self.path(path, at.join(key), Some(step)))
        };
        let collect = context(&config.collect, "collect");
        let into = context(&config.into, "into");
        let (over, (item, index)) = over.zip(bound)?;
        Some(FanSite {
            over,
            r#as: item,
            index,
            max_concurrency: config.max_concurrency,
            collect,
            into,
        })
    }

    /// The task's function: resolved, its config compiled at
    /// `<at>.function.input` and visited, and its references checked.
    /// `None` when the name is unknown or the config was Rejected.
    fn callee(
        &mut self,
        task: &TaskDef,
        at: &AuthoredPath,
        step: (StepKind, &str),
    ) -> Option<Callee> {
        let registry = self.registry;
        let name = &task.function.name;
        let Some(entry) = registry.entry(name) else {
            let issue = Issue::new(
                IssueCode::UnknownFunction,
                at.join("function").join("name"),
                format!("no function `{name}` is registered"),
            );
            self.report(issue, Some(step));
            return None;
        };
        let input_at = at.join("function").join("input");
        let input = &task.function.input;
        let callee = {
            let mut cx = CompileCx::new(registry.exprs(), &mut self.issues)
                .at_path(input_at.clone())
                .for_step(step.0, step.1);
            match entry {
                Entry::Builtin(kind) => kind.compile(input, &mut cx).map(Callee::Builtin),
                Entry::Host(f) => {
                    if let Some(fields) = &f.spec().runtime_fields {
                        cx = cx.with_runtime_fields(fields);
                    }
                    let prepare = Prepare::new(name, self.workflow_id, step.1, f.spec());
                    f.compile(input, &mut cx, &prepare).map(Callee::Host)
                }
            }
        }?;
        callee.visit(&mut Below {
            hook: &mut *self.hook,
            issues: &mut self.issues,
            root: &input_at,
            step: Some(step),
        });
        if let Entry::Host(f) = entry {
            self.references(f.spec(), input, &input_at, step);
        }
        Some(callee)
    }

    /// The oracle, over each `Reference` in `spec`: a literal string at the
    /// field below `input` that it does not resolve is
    /// `UNRESOLVED_REFERENCE` at the field. A field that is not an authored
    /// path names nothing, and is not checked.
    fn references(
        &mut self,
        spec: &FunctionSpec,
        input: &Value,
        input_at: &AuthoredPath,
        step: (StepKind, &str),
    ) {
        let Some(oracle) = self.resources else {
            return;
        };
        let mut cx = CompileCx::new(self.registry.exprs(), &mut self.issues)
            .at_path(input_at.clone())
            .for_step(step.0, step.1);
        for reference in &spec.references {
            let Ok(field) = AuthoredPath::from_str(&reference.field) else {
                continue;
            };
            if let Some(Value::String(name)) = lookup(input, &field)
                && !oracle.resolve(&reference.kind, name)
            {
                let kind = &reference.kind;
                let message = format!("no {kind} named `{name}` resolves");
                cx.adopt(Issue::new(IssueCode::UnresolvedReference, field, message));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::Result;
    use crate::def::{FailurePolicy, RejectionPolicy, parse};
    use crate::function::builtins::{Builtin, BuiltinKind};
    use crate::function::{Ctx, Function, FunctionConfig, Outcome};
    use serde_json::json;

    #[derive(FunctionConfig)]
    struct EchoConfig {
        #[config(records)]
        text: Expr<String>,
        label: Option<Expr<String>>,
    }

    struct Echo;

    impl Function for Echo {
        type Config = EchoConfig;
        async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &EchoConfig) -> Result<Outcome> {
            Ok(Outcome::Ok)
        }
    }

    /// A step stamp, owned.
    type Step = Option<(StepKind, String)>;

    /// Every site the hook was called at: its path, `records`, and stamp.
    #[derive(Default)]
    struct Recorder(Vec<(String, bool, Step)>);

    impl Hook for Recorder {
        fn expr(&mut self, site: Site<'_>, _raw: &Value, _issues: &mut Vec<Issue>) {
            let step = site.step.map(|(kind, id)| (kind, id.to_owned()));
            self.0.push((site.at.to_string(), site.records, step));
        }
    }

    fn compiler() -> Compiler {
        let mut registry = Registry::default();
        registry.register("echo", Echo);
        registry.compiler(CompileOptions::default())
    }

    /// A workflow with an expression at every kind of site, and every
    /// workflow fact set.
    fn every_site() -> WorkflowDef {
        let json = json!({
            "id": "w", "channel": "orders", "priority": 4, "status": "paused", "version": 3,
            "on_failure": "continue", "rollout": {"bucket_start": 10, "bucket_end": 60},
            "condition": {"var": "metadata.go"},
            "loop": {
                "counter": "i", "init": 1, "increment": 2, "max": 9,
                "over": {"var": "data.batches"}, "as": "batch", "scratch": "acc",
                "setup": [{"id": "s", "condition": {"var": "data.s"},
                           "function": {"name": "log", "input": {"message": "start"}}}],
            },
            "tasks": [
                {"id": "g", "condition": {"var": "data.g"}, "tasks": [
                    {"id": "f", "condition": {"var": "data.f"},
                     "for_each": {"over": {"var": "data.items"}, "as": "item",
                                  "max_concurrency": 2, "collect": "temp_data.price",
                                  "into": "temp_data.acc.prices"},
                     "on_failure": "continue", "on_rejection": "halt",
                     "function": {"name": "echo", "input": {
                         "text": {"var": "temp_data.item"}, "label": "x"}}},
                ]},
                {"id": "m", "terminal": true, "function": {"name": "map", "input": {
                    "mappings": [{"path": "data.x", "logic": {"var": "data.y"}}]}}},
            ],
        });
        let parsed = parse(&json, ParseOptions::default());
        parsed
            .def
            .unwrap_or_else(|| panic!("the definition parses: {:?}", parsed.issues))
    }

    fn task(id: &str) -> Step {
        Some((StepKind::Task, id.to_owned()))
    }

    #[test]
    fn the_hook_sees_every_site_at_its_absolute_path() {
        let mut recorder = Recorder::default();
        let (_, issues) = compiler().compile_with(&every_site(), &mut recorder);
        assert!(issues.is_empty(), "{issues:?}");
        let site = |at: &str, records: bool, step: Step| (at.to_owned(), records, step);
        let setup = Some((StepKind::Setup, "s".to_owned()));
        let group = Some((StepKind::Group, "g".to_owned()));
        assert_eq!(
            recorder.0,
            [
                site("condition", false, None),
                site("loop.over", true, None),
                site("loop.setup[0].condition", false, setup.clone()),
                site("loop.setup[0].function.input.message", true, setup),
                site("tasks[0].condition", false, group),
                site("tasks[0].tasks[0].condition", false, task("f")),
                site("tasks[0].tasks[0].for_each.over", true, task("f")),
                site("tasks[0].tasks[0].function.input.text", true, task("f")),
                site("tasks[0].tasks[0].function.input.label", false, task("f")),
                site("tasks[1].function.input.mappings[0].path", true, task("m")),
                site("tasks[1].function.input.mappings[0].logic", true, task("m")),
            ]
        );
    }

    #[test]
    fn check_steps_reaches_the_same_sites_under_its_root() {
        // `check_steps` compiles through the same per-step walk.
        let steps = json!([{"id": "f", "condition": {"var": "data.f"},
            "function": {"name": "echo", "input": {"text": "a", "label": {"var": "data.l"}}}}]);
        let root: AuthoredPath = "tasks[2].tasks".parse().unwrap();
        let (parsed, issues) = def::parse_steps(&steps, &root, ParseOptions::default());
        assert!(issues.is_empty(), "{issues:?}");
        let parsed = parsed.unwrap();
        let compiler = compiler();
        let mut recorder = Recorder::default();
        let mut walk = Walk::new(&compiler, "", &mut recorder);
        for (at, kind, step) in def::walk_steps(&parsed, &root) {
            walk.step(&at, kind, step);
        }
        assert!(walk.issues.is_empty(), "{:?}", walk.issues);
        let paths: Vec<_> = recorder.0.iter().map(|(at, ..)| at.as_str()).collect();
        assert_eq!(
            paths,
            [
                "tasks[2].tasks[0].condition",
                "tasks[2].tasks[0].function.input.text",
                "tasks[2].tasks[0].function.input.label",
            ]
        );
    }

    #[test]
    fn the_side_tables_are_filled_in_walk_order() {
        let compiled = compiler().compile_one(&every_site()).unwrap();
        assert!(compiled.issues.is_empty(), "{:?}", compiled.issues);
        let w = &compiled.workflow;

        // The workflow's facts.
        assert_eq!(w.id(), "w");
        assert_eq!(w.channel, "orders");
        assert_eq!((w.priority, w.version), (4, 3));
        assert_eq!(w.status, Status::Paused);
        assert_eq!(w.rollout, Some(Rollout::new(10, 60)));
        assert_eq!(w.on_failure, WorkflowFailurePolicy::Continue);
        assert!(w.ops.is_empty());

        // One row per task, never per group: setup first.
        let rows: Vec<_> = w
            .tasks
            .iter()
            .map(|t| (&*t.id, &*t.function, t.kind, t.path.to_string()))
            .collect();
        assert_eq!(
            rows,
            [
                ("s", "log", StepKind::Setup, "loop.setup[0]".to_owned()),
                ("f", "echo", StepKind::Task, "tasks[0].tasks[0]".to_owned()),
                ("m", "map", StepKind::Task, "tasks[1]".to_owned()),
            ]
        );

        // Each call site keeps its task row and the task's policies.
        let calls: Vec<_> = w
            .calls
            .iter()
            .map(|c| {
                let callee = match &c.callee {
                    Callee::Builtin(b) => Some(b.kind()),
                    Callee::Host(_) => None,
                };
                (c.task, callee, c.on_failure, c.on_rejection, c.terminal)
            })
            .collect();
        assert_eq!(
            calls,
            [
                (
                    0,
                    Some(BuiltinKind::Log),
                    FailurePolicy::Fail,
                    RejectionPolicy::Continue,
                    false
                ),
                (
                    1,
                    None,
                    FailurePolicy::Continue,
                    RejectionPolicy::Halt,
                    false
                ),
                (
                    2,
                    Some(BuiltinKind::Map),
                    FailurePolicy::Fail,
                    RejectionPolicy::Continue,
                    true
                ),
            ]
        );
        assert!(matches!(
            &w.calls[2].callee,
            Callee::Builtin(Builtin::Map(_))
        ));

        // Every condition, with its path and its step.
        let conds: Vec<_> = w
            .conds
            .iter()
            .map(|c| (c.path.to_string(), c.step.as_deref(), c.expr.raw().clone()))
            .collect();
        assert_eq!(
            conds,
            [
                ("condition".to_owned(), None, json!({"var": "metadata.go"})),
                (
                    "loop.setup[0].condition".to_owned(),
                    Some("s"),
                    json!({"var": "data.s"})
                ),
                (
                    "tasks[0].condition".to_owned(),
                    Some("g"),
                    json!({"var": "data.g"})
                ),
                (
                    "tasks[0].tasks[0].condition".to_owned(),
                    Some("f"),
                    json!({"var": "data.f"})
                ),
            ]
        );

        // The fan-out's bindings are `temp_data` paths; collect and into are
        // context paths.
        let [fan] = &w.fans[..] else {
            panic!("one fan-out");
        };
        assert_eq!(fan.over.raw(), &json!({"var": "data.items"}));
        assert_eq!(fan.r#as.to_string(), "temp_data.item");
        assert_eq!(fan.index.to_string(), "temp_data.item_index");
        assert_eq!(fan.max_concurrency, 2);
        assert_eq!(fan.collect.as_ref().unwrap().to_string(), "temp_data.price");
        assert_eq!(
            fan.into.as_ref().unwrap().to_string(),
            "temp_data.acc.prices"
        );

        // The loop's slots are `temp_data` paths.
        let [lp] = &w.loops[..] else {
            panic!("one loop");
        };
        assert_eq!((lp.init, lp.increment, lp.max), (1, 2, 9));
        assert_eq!(lp.counter.as_ref().unwrap().to_string(), "temp_data.i");
        assert_eq!(
            lp.over.as_ref().unwrap().raw(),
            &json!({"var": "data.batches"})
        );
        assert_eq!(lp.r#as.as_ref().unwrap().to_string(), "temp_data.batch");
        assert_eq!(lp.scratch.as_ref().unwrap().to_string(), "temp_data.acc");
    }

    #[test]
    fn a_workflow_without_a_loop_or_slots_has_none() {
        let json = json!({"id": "w", "loop": {"max": 2},
            "tasks": [{"id": "t", "function": {"name": "echo", "input": {"text": "a"}}}]});
        let def = parse(&json, ParseOptions::default()).def.unwrap();
        let compiled = compiler().compile_one(&def).unwrap();
        let [lp] = &compiled.workflow.loops[..] else {
            panic!("one loop");
        };
        assert!(lp.counter.is_none() && lp.over.is_none());
        assert!(lp.r#as.is_none() && lp.scratch.is_none());
        assert!(compiled.workflow.conds.is_empty());
        assert!(compiled.workflow.fans.is_empty());
    }

    #[test]
    fn debug_shows_the_id_the_op_count_and_the_table_sizes() {
        let compiled = compiler().compile_one(&every_site()).unwrap();
        assert_eq!(
            format!("{:?}", compiled.workflow),
            "CompiledWorkflow { id: \"w\", ops: 0, conds: 4, calls: 3, fans: 1, loops: 1, \
             tasks: 3, .. }"
        );
    }

    #[test]
    fn a_compiled_workflow_crosses_threads() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<CompiledWorkflow>();
        send_sync::<Compiler>();
    }
}
