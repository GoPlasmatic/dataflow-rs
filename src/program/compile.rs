//! The compiler: one walk over a definition that compiles every expression
//! and config, reports every issue at its authored path, fills the side
//! tables the ops index, and lowers each step into ops as it reaches it.

use super::checks::{Below, Checks, Hook, Site, Stamp};
use super::fingerprint::Fingerprint;
use super::lower::Lower;
use super::ops::{CallSite, Callee, CondKind, CondSite, FanSite, LoopSite, Op, Policy, TaskMeta};
use crate::base::any_rejected;
use crate::base::typo::near_miss;
use crate::base::{AuthoredPath, Issue, IssueCode, PathSeg, StepKind};
use crate::def::{
    self, ForEachDef, GroupDef, Logic, LoopDef, ParseOptions, Rollout, Status, StepDef, TaskDef,
    WorkflowDef, WorkflowFailurePolicy,
};
use crate::expr::path::MAX_PATH_SEGMENTS;
use crate::expr::{CompileCx, Expr, ExprEngine, Path};
use crate::function::{Entry, FunctionSpec, Prepare, Registry};
use datavalue::OwnedDataValue;
use serde_json::Value;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

/// Answers whether a definition that a config names exists, such as a
/// connector a deployment declares, and why not when it does not.
///
/// A [`Compiler`] given one through [`CompileOptions::resources`] checks each
/// [`Reference`](crate::function::Reference) a host function's spec declares.
/// A field holding a literal string the oracle does not resolve is an
/// `UNRESOLVED_REFERENCE` Defect, whose message leads with the oracle's
/// reason when it gives one. A computed or absent value is not checked.
///
/// ```
/// use dataflow_rs::program::ResourceOracle;
///
/// /// The deployment's connectors: `orders` a database, `sessions` a cache.
/// struct Connectors;
///
/// impl ResourceOracle for Connectors {
///     fn resolve(&self, _kind: &str, name: &str) -> Result<(), String> {
///         match name {
///             "orders" => Ok(()),
///             "sessions" => Err(format!("connector `{name}` is a cache, not a database")),
///             _ => Err(String::new()),
///         }
///     }
/// }
/// ```
pub trait ResourceOracle: Send + Sync + 'static {
    /// `Ok` when a `kind` named `name` exists: the connector `db1`, say.
    /// `Err(why)` when it does not. A `why` that is not empty is the issue's
    /// message, after what did not resolve: `` `sessions` does not resolve
    /// as a connector: connector `sessions` is a cache, not a database ``.
    /// An empty one gives the plain message: `` no connector named `db2`
    /// resolves ``.
    ///
    /// # Errors
    ///
    /// Why `name` does not resolve, or nothing more to say: an empty string.
    fn resolve(&self, kind: &str, name: &str) -> std::result::Result<(), String>;
}

/// How a [`Compiler`] compiles.
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct CompileOptions {
    /// The oracle the `Reference` fields are checked against. `None`, the
    /// default, checks nothing.
    pub resources: Option<Arc<dyn ResourceOracle>>,
}

impl fmt::Debug for CompileOptions {
    /// Whether an oracle is set; not the oracle.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompileOptions")
            .field("resources", &self.resources.is_some())
            .finish_non_exhaustive()
    }
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
#[derive(Clone, Debug)]
pub struct Compiler {
    pub(super) registry: Registry,
    pub(super) options: CompileOptions,
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
    /// path. A root that holds no steps, one neither ending in a `tasks` key
    /// nor `loop.setup`, is `PARSE_FAILED` at the root, and so is a fragment
    /// that is not an array.
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
        if !def::holds_steps(&root) {
            let message = format!(
                "`{root}` holds no steps: a fragment sits at a `tasks` list, such as `tasks` or \
                 `tasks[2].tasks`, or at `loop.setup`"
            );
            return vec![Issue::new(IssueCode::ParseFailed, root, message)];
        }
        let (parsed, mut issues) = def::parse_steps(steps, &root, ParseOptions::default());
        if let Some(steps) = parsed {
            let mut checks = Checks::new(self.registry.exprs());
            let mut walk = Walk::new(self, "", &mut checks);
            walk.fragment(&steps, &root);
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
        if any_rejected(&issues) {
            return Err(issues);
        }
        debug_assert!(
            workflow.check_invariants().is_ok(),
            "the lowering of `{}` broke an invariant: {:?}\n{}",
            workflow.id,
            workflow.check_invariants(),
            workflow.listing(),
        );
        Ok(Compiled { workflow, issues })
    }

    /// The compile walk, calling `hook` at every expression site.
    fn compile_with(
        &self,
        def: &WorkflowDef,
        hook: &mut dyn Hook,
    ) -> (CompiledWorkflow, Vec<Issue>) {
        let mut walk = Walk::new(self, &def.id, hook);
        let root = AuthoredPath::root();
        // 1. The workflow condition: `None` when it is absent or literally
        //    true, so it lowers to no `Cond`.
        let cond = def
            .condition
            .as_ref()
            .and_then(|raw| walk.cond(raw, root.join("condition"), None));
        // 2. The loop: `over`, then its slots. 3. Each step: setup first, then
        //    the body, pre-order, each lowered as the walk reaches it.
        let body = root.join("tasks");
        match &def.r#loop {
            None => {
                walk.lower.workflow_cond(cond);
                walk.steps(&def.tasks, &body, false);
            }
            Some(config) => {
                let lp = walk.r#loop(config);
                // The pre-phase, before the first sweep: only a loop with
                // setup or `over` has one.
                if !config.setup.is_empty() || config.over.is_some() {
                    walk.lower.workflow_cond(cond);
                }
                walk.steps(&config.setup, &root.join("loop").join("setup"), true);
                let sweep = walk.lower.loop_init(lp);
                walk.lower.workflow_cond(cond);
                walk.steps(&def.tasks, &body, false);
                walk.lower.loop_next(lp, sweep);
            }
        }
        // 4. The workflow lint, `UNGUARDED_VALIDATION`.
        walk.hook.workflow(def, &mut walk.issues);
        let workflow = CompiledWorkflow {
            registry: self.registry.clone(),
            id: def.id.clone(),
            channel: def.channel.clone(),
            priority: def.priority,
            status: def.status,
            version: def.version,
            rollout: def.rollout,
            on_failure: def.on_failure,
            ops: walk.lower.finish(),
            conds: walk.conds,
            calls: walk.calls,
            fans: walk.fans,
            loops: walk.loops,
            tasks: walk.tasks,
            fingerprint: walk.hook.fingerprint(),
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
pub struct CompiledWorkflow {
    /// The registry it was compiled under: [`Compiler::link`] links it under
    /// that one only. Holding it keeps its identity from being reused.
    pub(crate) registry: Registry,
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
    pub(crate) fingerprint: Fingerprint,
}

impl CompiledWorkflow {
    /// The workflow's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The workflow's fingerprint: the operators and data keys its
    /// expressions use.
    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
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

/// Where a task's function is compiled: what [`compile_callee`] needs
/// beside the function and its input.
pub(crate) struct CalleeAt<'a> {
    /// The function's registered name.
    pub(crate) name: &'a str,
    /// The workflow's id, which `prepare` is told; empty for a fragment.
    pub(crate) workflow_id: &'a str,
    /// The task's step, stamped on every issue.
    pub(crate) step: (StepKind, &'a str),
    /// Where the config sits: the task's `function.input`.
    pub(crate) input_at: &'a AuthoredPath,
}

/// `entry`'s config from `input`, compiled as a task's at `at`: a host
/// function's with its spec's runtime fields and a [`Prepare`] naming the
/// task; a config that refuses its input without recording why reported, as
/// [`refused`]; then, when it compiled, its expressions walked with `hook`.
/// The compiler compiles every task's function through it, and
/// `function::testing` the one function a unit test calls, so a check
/// added here reaches both.
pub(crate) fn compile_callee(
    exprs: &ExprEngine,
    entry: Entry<'_>,
    input: &Value,
    at: &CalleeAt<'_>,
    hook: &mut dyn Hook,
    issues: &mut Vec<Issue>,
) -> Option<Callee> {
    let callee = {
        let mut cx = CompileCx::new(exprs, issues)
            .at_path(at.input_at.clone())
            .for_step(at.step.0, at.step.1);
        let before = cx.rejected();
        let callee = match entry {
            Entry::Builtin(kind) => kind.compile(input, &mut cx).map(Callee::Builtin),
            Entry::Host(f) => {
                if let Some(fields) = &f.spec().runtime_fields {
                    cx = cx.with_runtime_fields(fields);
                }
                let prepare = Prepare::new(at.name, at.workflow_id, at.step.1, f.spec());
                f.compile(input, &mut cx, &prepare).map(Callee::Host)
            }
        };
        // A hand-written config can refuse without recording why. Its task
        // would then have no call, so the refusal is reported.
        if callee.is_none() && cx.rejected() == before {
            refused(&mut cx, at.name);
        }
        callee
    }?;
    callee.visit(&mut Below {
        hook,
        issues,
        root: at.input_at,
        step: Some(at.step),
    });
    Some(callee)
}

/// A config of `name` that refused its input without recording why,
/// reported, so a rejection always carries an issue.
pub(crate) fn refused(cx: &mut CompileCx<'_>, name: &str) {
    cx.issue(
        IssueCode::InputParse,
        format!("the config of `{name}` refused its input without reporting why"),
    );
}

/// A table's next index.
pub(super) fn next(len: usize) -> u32 {
    u32::try_from(len).expect("a workflow holds fewer than 2^32 steps")
}

/// The value at `path` below `value`, if there is one.
fn lookup<'v>(value: &'v Value, path: &AuthoredPath) -> Option<&'v Value> {
    path.segs().iter().try_fold(value, |value, seg| match seg {
        PathSeg::Key(key) => value.get(&**key),
        PathSeg::Index(index) => value.get(*index),
    })
}

/// One compile's state: the issues so far, in walk order, the side tables,
/// in walk order, and the ops so far.
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
    lower: Lower,
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
            lower: Lower::default(),
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
                let site = Site {
                    at,
                    records,
                    step,
                    field: None,
                };
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

    /// A condition at `at`: compiled and visited as not recorded. Its row in
    /// `conds`, or `None` when it did not compile or folds to the literal
    /// `true`, which keeps no row and lowers to no `Cond`, so a trivially
    /// true condition costs nothing at run time.
    fn cond(&mut self, raw: &Logic, at: AuthoredPath, step: Stamp<'_>) -> Option<u32> {
        let expr = self.expr(raw, &at, false, step)?;
        if expr.constant() == Some(&OwnedDataValue::Bool(true)) {
            return None;
        }
        let row = next(self.conds.len());
        self.conds.push(CondSite {
            expr,
            path: at,
            step: step.map(|(_, id)| id.to_owned()),
        });
        Some(row)
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

    /// The loop's bounds, checked again, then its `over`, recorded,
    /// then its slots, each `temp_data.<slot>`. Returns its row in `loops`.
    ///
    /// `parse` refuses a loop that never advances or never sweeps, but a host
    /// may edit a parsed definition, and the interpreter's termination rests
    /// on `increment >= 1`.
    fn r#loop(&mut self, config: &LoopDef) -> u32 {
        let at = AuthoredPath::root().join("loop");
        for (code, path, message) in
            def::bound_issues(config.init, Some(config.increment), Some(config.max))
        {
            self.report(Issue::new(code, path, message), None);
        }
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
        let row = next(self.loops.len());
        self.loops.push(LoopSite {
            init: config.init,
            increment: config.increment,
            max: config.max,
            counter,
            over,
            r#as: item,
            scratch,
        });
        row
    }

    /// A fragment's steps at `root`, as a workflow's would be walked: lowered
    /// with every failure to the end, and with no workflow condition or loop.
    fn fragment(&mut self, steps: &[StepDef], root: &AuthoredPath) {
        self.steps(steps, root, def::fragment_place(root).0);
    }

    /// The steps of the list at `at`, pre-order: each group before its
    /// members, every one a setup step when `setup`. This is
    /// [`WorkflowDef::walk`]'s order.
    fn steps(&mut self, steps: &[StepDef], at: &AuthoredPath, setup: bool) {
        for (i, step) in steps.iter().enumerate() {
            let at = at.join(i);
            let kind = step.kind(setup);
            match step {
                StepDef::Group(group) => self.group(&at, kind, group, setup),
                StepDef::Task(task) => self.task(&at, kind, task),
            }
        }
    }

    /// A group: its condition, its members, then a `Halt` when it is
    /// terminal. A false condition jumps past the `Halt`.
    fn group(&mut self, at: &AuthoredPath, kind: StepKind, group: &GroupDef, setup: bool) {
        let cond = group
            .condition
            .as_ref()
            .and_then(|raw| self.cond(raw, at.join("condition"), Some((kind, &group.id))));
        let open = self.lower.open(cond, CondKind::Group);
        self.steps(&group.tasks, &at.join("tasks"), setup);
        if group.terminal {
            self.lower.halt();
        }
        self.lower.close(open);
    }

    /// A task: its condition, its fan-out, then its call, lowered to its
    /// `Cond` and its `Call` or `FanOut`.
    fn task(&mut self, at: &AuthoredPath, kind: StepKind, task: &TaskDef) {
        let step = (kind, task.id.as_str());
        let cond = task
            .condition
            .as_ref()
            .and_then(|raw| self.cond(raw, at.join("condition"), Some(step)));
        let fan = task.for_each.as_ref().and_then(|config| {
            let fan = self.fan(config, &at.join("for_each"), step)?;
            let row = next(self.fans.len());
            self.fans.push(fan);
            Some(row)
        });
        let row = next(self.tasks.len());
        self.tasks.push(TaskMeta {
            id: task.id.clone(),
            function: task.function.name.clone(),
            kind,
            path: at.clone(),
        });
        let site = self.callee(task, at, step).map(|callee| {
            let site = next(self.calls.len());
            self.calls.push(CallSite {
                task: row,
                callee,
                policy: Policy {
                    on_failure: task.on_failure,
                    on_rejection: task.on_rejection,
                    terminal: task.terminal,
                },
            });
            site
        });
        let open = self.lower.open(cond, CondKind::Task);
        match (site, task.for_each.is_some(), fan) {
            (Some(site), false, _) => self.lower.call(site),
            (Some(site), true, Some(fan)) => self.lower.fan_out(site, fan),
            // A Rejected issue is recorded, so the workflow is never lowered
            // for use: the task emits nothing.
            _ => {}
        }
        self.lower.close(open);
    }

    /// A fan-out at `at`: its rules, checked again, then its `over`,
    /// recorded, then its bindings as paths, `into` with room for the
    /// index each element's result lands at.
    ///
    /// `parse` refuses a fan-out that breaks a rule, but a host may edit a
    /// parsed definition, and the interpreter trusts them: no call starts
    /// under `max_concurrency: 0`, and an `into` at a root would replace it.
    fn fan(
        &mut self,
        config: &ForEachDef,
        at: &AuthoredPath,
        step: (StepKind, &str),
    ) -> Option<FanSite> {
        if let Some((code, path, message)) = def::for_each_issue(config, at) {
            self.report(Issue::new(code, path, message), Some(step));
            return None;
        }
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
        let into = context(&config.into, "into").and_then(|into| {
            if into.index_child(0).is_some() {
                return Some(into);
            }
            let message = format!(
                "path `{into}` has {MAX_PATH_SEGMENTS} segments, and each element's result \
                 lands one segment below it; a path holds at most {MAX_PATH_SEGMENTS}"
            );
            let issue = Issue::new(IssueCode::InvalidPath, at.join("into"), message);
            self.report(issue, Some(step));
            None
        });
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
    /// `<at>.function.input` and visited ([`compile_callee`]), and its
    /// references checked. `None` when the name is unknown, which is
    /// `UNKNOWN_FUNCTION` naming the nearest catalogue name if one is close,
    /// or the config was Rejected.
    fn callee(
        &mut self,
        task: &TaskDef,
        at: &AuthoredPath,
        step: (StepKind, &str),
    ) -> Option<Callee> {
        let registry = self.registry;
        let name = &task.function.name;
        let Some(entry) = registry.entry(name) else {
            let names: Vec<&str> = registry.catalogue().map(|f| f.name).collect();
            let message = match near_miss(name, &names) {
                Some(near) => format!("no function `{name}` is registered; did you mean `{near}`?"),
                None => format!("no function `{name}` is registered"),
            };
            let issue = Issue::new(
                IssueCode::UnknownFunction,
                at.join("function").join("name"),
                message,
            );
            self.report(issue, Some(step));
            return None;
        };
        let input_at = at.join("function").join("input");
        let input = &task.function.input;
        let site = CalleeAt {
            name,
            workflow_id: self.workflow_id,
            step,
            input_at: &input_at,
        };
        let callee = compile_callee(
            registry.exprs(),
            entry,
            input,
            &site,
            &mut *self.hook,
            &mut self.issues,
        )?;
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
                && let Err(why) = oracle.resolve(&reference.kind, name)
            {
                let kind = &reference.kind;
                let message = if why.is_empty() {
                    format!("no {kind} named `{name}` resolves")
                } else {
                    format!("`{name}` does not resolve as a {kind}: {why}")
                };
                cx.adopt(Issue::new(IssueCode::UnresolvedReference, field, message));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Result, Severity};
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
    fn the_compile_walk_visits_the_steps_in_def_walk_order() {
        // `UNGUARDED_VALIDATION` reads its chain from `WorkflowDef::walk`, and
        // everything else from the compile walk: the two orders are one.
        // Every step has a condition, so the hook sees each step once.
        let c = |id: &str| json!({"var": format!("data.{id}")});
        let t = |id: &str| {
            json!({"id": id, "condition": c(id),
                   "function": {"name": "log", "input": {"message": "m"}}})
        };
        let json = json!({"id": "w", "loop": {"max": 2, "setup": [
                t("s1"),
                {"id": "sg", "condition": c("sg"), "tasks": [t("s2"), t("s3")]},
            ]},
            "tasks": [
                {"id": "a", "condition": c("a"), "tasks": [
                    {"id": "b", "condition": c("b"), "tasks": [t("t1")]},
                    t("t2"),
                    {"id": "c", "condition": c("c"), "tasks": [
                        {"id": "d", "condition": c("d"), "tasks": [t("t3")]},
                    ]},
                ]},
                t("t4"),
            ],
        });
        let def = parse(&json, ParseOptions::default()).def.unwrap();
        let mut recorder = Recorder::default();
        let (_, issues) = compiler().compile_with(&def, &mut recorder);
        assert!(issues.is_empty(), "{issues:?}");
        let walked: Vec<(String, Step)> = recorder
            .0
            .into_iter()
            .filter_map(|(at, _, step)| Some((at.strip_suffix(".condition")?.to_owned(), step)))
            .collect();
        let expected: Vec<(String, Step)> = def
            .walk()
            .map(|(path, kind, step)| (path.to_string(), Some((kind, step.id().to_owned()))))
            .collect();
        assert_eq!(walked.len(), 12);
        assert_eq!(walked, expected);
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
        walk.fragment(&parsed, &root);
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

        // The lowering: a setup step, a loop over a group holding a
        // conditional fan-out, and a terminal task.
        let listing = [
            " 0 Cond workflow else->end",
            " 1 Cond task s else->3",
            " 2 Call s log builtin fail->end",
            " 3 LoopInit exit->end",
            " 4 Cond workflow else->end",
            " 5 Cond group g else->8",
            " 6 Cond task f else->8",
            " 7 FanOut f echo host as=item fail->9",
            " 8 Call m map builtin fail->9 terminal",
            " 9 LoopNext body->4 exit->end",
        ];
        let expected: String = listing.iter().map(|line| format!("{line}\n")).collect();
        assert_eq!(w.listing(), expected, "\n{}", w.listing());
        assert_eq!(w.check_invariants(), Ok(()));

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
                (
                    c.task,
                    callee,
                    c.policy.on_failure,
                    c.policy.on_rejection,
                    c.policy.terminal,
                )
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

    /// An `echo` task.
    fn echo_task() -> Value {
        json!({"id": "t", "function": {"name": "echo", "input": {"text": "a"}}})
    }

    /// A counter loop from `0` to `3`, parsed, then edited by `edit`, as a
    /// host may edit a parsed definition: the issues `compile_one` rejects
    /// it with, as `(code, severity, path, message)`.
    fn edited_loop(edit: impl FnOnce(&mut LoopDef)) -> Vec<(IssueCode, Severity, String, String)> {
        let json = json!({"id": "w", "loop": {"counter": "i", "max": 3}, "tasks": [echo_task()]});
        let mut def = parse(&json, ParseOptions::default()).def.unwrap();
        edit(def.r#loop.as_mut().unwrap());
        compiler()
            .compile_one(&def)
            .expect_err("compile refuses the edited loop")
            .into_iter()
            .map(|i| (i.code, i.severity, i.path.to_string(), i.message))
            .collect()
    }

    /// What `parse` reports for `json`'s loop, as `edited_loop` lists it.
    fn parse_issues(json: &Value) -> Vec<(IssueCode, Severity, String, String)> {
        parse(json, ParseOptions::default())
            .issues
            .into_iter()
            .map(|i| (i.code, i.severity, i.path.to_string(), i.message))
            .collect()
    }

    #[test]
    fn compile_refuses_an_edited_loop_increment_below_one() {
        // I78: `increment = 0` would never advance, so the loop would never end.
        for increment in [0, -2] {
            let issues = edited_loop(|lp| lp.increment = increment);
            assert_eq!(
                issues,
                [(
                    IssueCode::LoopIncrementTooSmall,
                    Severity::Rejected,
                    "loop.increment".to_owned(),
                    format!(
                        "loop increment must be >= 1, got {increment} (a non-advancing counter \
                         would never reach max)"
                    ),
                )]
            );
            // The code, path and message `parse` gives the authored spelling.
            let authored = json!({"id": "w", "loop": {"max": 3, "increment": increment},
                "tasks": [echo_task()]});
            assert_eq!(parse_issues(&authored), issues);
        }
    }

    #[test]
    fn compile_refuses_an_edited_loop_bound_at_or_below_init() {
        for (init, max) in [(0, 0), (2, 1)] {
            let issues = edited_loop(|lp| {
                lp.init = init;
                lp.max = max;
            });
            assert_eq!(
                issues,
                [(
                    IssueCode::LoopBoundEmpty,
                    Severity::Rejected,
                    "loop.max".to_owned(),
                    format!(
                        "loop max ({max}) must be greater than init ({init}) — the bound is \
                         half-open, so this could never run a sweep"
                    ),
                )]
            );
            let authored =
                json!({"id": "w", "loop": {"init": init, "max": max}, "tasks": [echo_task()]});
            assert_eq!(parse_issues(&authored), issues);
        }
    }

    /// An `echo` fan-out, parsed, then edited by `edit`, as a host may edit a
    /// parsed definition: the issues `compile_one` rejects it with, as
    /// `edited_loop` lists them.
    fn edited_fan(
        edit: impl FnOnce(&mut ForEachDef),
    ) -> Vec<(IssueCode, Severity, String, String)> {
        let mut task = echo_task();
        task["for_each"] = json!({"over": {"var": "data.items"}, "as": "item",
            "collect": "temp_data.r", "into": "data.rs"});
        let json = json!({"id": "w", "tasks": [task]});
        let mut def = parse(&json, ParseOptions::default()).def.unwrap();
        let StepDef::Task(task) = &mut def.tasks[0] else {
            panic!("a task");
        };
        edit(task.for_each.as_mut().unwrap());
        compiler()
            .compile_one(&def)
            .expect_err("compile refuses the edited fan-out")
            .into_iter()
            .map(|i| (i.code, i.severity, i.path.to_string(), i.message))
            .collect()
    }

    #[test]
    fn compile_refuses_an_edited_fan_out_the_parser_would_refuse() {
        // `max_concurrency: 0` would start no call, and an `into` or a
        // `collect` at a root would replace the root with an array.
        type Edit = fn(&mut ForEachDef);
        let cases: [(Edit, Value, &str, &str); 3] = [
            (
                |fan| fan.max_concurrency = 0,
                json!({"max_concurrency": 0}),
                "max_concurrency",
                "for_each max_concurrency must be at least 1",
            ),
            (
                |fan| fan.into = Some("data".to_owned()),
                json!({"into": "data"}),
                "into",
                "for_each into must be a context path below data, metadata or temp_data, \
                 got \"data\"",
            ),
            (
                |fan| fan.collect = Some("temp_data".to_owned()),
                json!({"collect": "temp_data"}),
                "collect",
                "for_each collect must be a context path below data, metadata or temp_data, \
                 got \"temp_data\"",
            ),
        ];
        for (edit, spelled, field, message) in cases {
            let issues = edited_fan(edit);
            assert_eq!(
                issues,
                [(
                    IssueCode::InvalidForEach,
                    Severity::Rejected,
                    format!("tasks[0].for_each.{field}"),
                    message.to_owned(),
                )]
            );
            // The code, path and message `parse` gives the authored spelling.
            let mut task = echo_task();
            task["for_each"] = json!({"over": {"var": "data.items"}, "as": "item",
                "collect": "temp_data.r", "into": "data.rs"});
            for (key, value) in spelled.as_object().unwrap() {
                task["for_each"][key] = value.clone();
            }
            assert_eq!(parse_issues(&json!({"id": "w", "tasks": [task]})), issues);
        }
    }

    #[test]
    fn compile_refuses_an_into_with_no_room_for_an_index() {
        // Each element's result lands at `into.<index>`, one segment below
        // `into`, and a path holds at most 64.
        for (depth, refused) in [(64, true), (63, false)] {
            let into = format!("data{}", ".k".repeat(depth - 1));
            let mut task = echo_task();
            task["for_each"] = json!({"over": {"var": "data.items"}, "as": "item",
                "collect": "temp_data.r", "into": into});
            let def = parse(
                &json!({"id": "w", "tasks": [task]}),
                ParseOptions::default(),
            )
            .def
            .unwrap();
            let issues = compiler().compile_one(&def).err().unwrap_or_default();
            let issues: Vec<_> = issues
                .into_iter()
                .map(|i| (i.code, i.path.to_string(), i.message))
                .collect();
            if refused {
                assert_eq!(
                    issues,
                    [(
                        IssueCode::InvalidPath,
                        "tasks[0].for_each.into".to_owned(),
                        format!(
                            "path `{into}` has 64 segments, and each element's result lands \
                             one segment below it; a path holds at most 64"
                        ),
                    )]
                );
            } else {
                assert!(issues.is_empty(), "{issues:?}");
            }
        }
    }

    #[test]
    fn debug_shows_the_id_the_op_count_and_the_table_sizes() {
        let compiled = compiler().compile_one(&every_site()).unwrap();
        assert_eq!(
            format!("{:?}", compiled.workflow),
            "CompiledWorkflow { id: \"w\", ops: 10, conds: 4, calls: 3, fans: 1, loops: 1, \
             tasks: 3, .. }"
        );
    }

    #[test]
    fn debug_shows_the_registry_and_whether_an_oracle_is_set() {
        assert_eq!(
            format!("{:?}", CompileOptions::default()),
            "CompileOptions { resources: false, .. }"
        );
        let compiler = format!("{:?}", compiler());
        assert!(
            compiler.starts_with("Compiler { registry: Registry { functions: [\"map\""),
            "{compiler}"
        );
        assert!(compiler.contains("\"echo\"]"), "{compiler}");
    }

    #[test]
    fn a_compiled_workflow_crosses_threads() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<CompiledWorkflow>();
        send_sync::<Compiler>();
    }
}
