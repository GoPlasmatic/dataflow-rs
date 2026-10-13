//! The definition types: plain data, with raw expressions.

use super::Rollout;
use crate::base::{AuthoredPath, StepKind};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

/// The value `parse` reads for an absent key, which `Serialize` leaves out.
pub(super) mod defaults {
    /// A workflow's `priority`.
    pub(crate) const PRIORITY: u32 = 0;
    /// A workflow's `version`.
    pub(crate) const VERSION: u32 = 1;
    /// A workflow's `channel`.
    pub(crate) const CHANNEL: &str = "default";
    /// A loop's `init`.
    pub(crate) const INIT: i64 = 0;
    /// A loop's `increment`.
    pub(crate) const INCREMENT: i64 = 1;
    /// A `for_each`'s `max_concurrency`.
    pub(crate) const MAX_CONCURRENCY: usize = 1;
}

/// Raw JSONLogic as authored. Only a compiler turns it into an
/// [`Expr`](crate::Expr).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Logic(pub Value);

impl Logic {
    /// Wraps an authored expression.
    pub fn new(value: Value) -> Self {
        Self(value)
    }
}

/// The keys every node shares: a workflow, a task and a group. `name` is
/// optional everywhere, which relaxes v3.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct NodeHeader {
    /// A human-readable name.
    pub name: Option<String>,
    /// What the node is for.
    pub description: Option<String>,
    /// Host and UI data, kept verbatim and never read: the authored `ext`
    /// object, then the unknown keys the compat reader moved there.
    pub ext: Map<String, Value>,
}

/// A workflow definition: a condition and a tree of steps.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct WorkflowDef {
    /// The workflow's id: a non-empty string.
    pub id: String,
    /// The name, description and `ext`.
    pub header: NodeHeader,
    /// The run order: lower runs first, and equal priorities keep their
    /// order. Default 0.
    pub priority: u32,
    /// Whether a message enters the workflow. Absent means true.
    pub condition: Option<Logic>,
    /// The steps, as authored: tasks and groups.
    pub tasks: Vec<StepDef>,
    /// What a task's failure that reaches the workflow does. Default `fail`.
    pub on_failure: WorkflowFailurePolicy,
    /// The routing channel. Default `"default"`, as in v3.
    pub channel: String,
    /// The lifecycle status. Default `active`.
    pub status: Status,
    /// The definition's version. Default 1.
    pub version: u32,
    /// The workflow's slice of the traffic on its channel. `None` runs for
    /// every message.
    pub rollout: Option<Rollout>,
    /// The bounded loop over the task list, if any.
    pub r#loop: Option<LoopDef>,
    /// Tags for the host's filtering.
    pub tags: Vec<String>,
    /// When the definition was created. Kept for stored definitions, never
    /// read.
    pub created_at: Option<DateTime<Utc>>,
    /// When the definition was last updated. Kept, never read.
    pub updated_at: Option<DateTime<Utc>>,
}

impl WorkflowDef {
    /// Every step with its authored path and kind, pre-order: `loop.setup`
    /// first, then `tasks`, each group before its members.
    ///
    /// A task's kind is [`StepKind::Task`] and a group's [`StepKind::Group`];
    /// every step under `loop.setup`, group or task, is [`StepKind::Setup`].
    pub fn walk(&self) -> impl Iterator<Item = (AuthoredPath, StepKind, &StepDef)> {
        let mut out = Vec::new();
        if let Some(config) = &self.r#loop {
            let at = AuthoredPath::root().join("loop").join("setup");
            collect(&config.setup, &at, true, &mut out);
        }
        collect(
            &self.tasks,
            &AuthoredPath::root().join("tasks"),
            false,
            &mut out,
        );
        out.into_iter()
    }
}

/// The steps of a fragment that [`parse_steps`](super::parse_steps) read at
/// `root`, as [`WorkflowDef::walk`] yields a workflow's: pre-order, each group
/// before its members, and every step a setup step when `root` is under
/// `loop.setup`.
pub(crate) fn walk_steps<'d>(
    steps: &'d [StepDef],
    root: &AuthoredPath,
) -> impl Iterator<Item = (AuthoredPath, StepKind, &'d StepDef)> {
    let (setup, _) = super::parse::fragment_place(root);
    let mut out = Vec::new();
    collect(steps, root, setup, &mut out);
    out.into_iter()
}

/// Appends `steps` and their members to `out`, pre-order.
fn collect<'d>(
    steps: &'d [StepDef],
    at: &AuthoredPath,
    setup: bool,
    out: &mut Vec<(AuthoredPath, StepKind, &'d StepDef)>,
) {
    for (i, step) in steps.iter().enumerate() {
        let path = at.join(i);
        let kind = match step {
            _ if setup => StepKind::Setup,
            StepDef::Task(_) => StepKind::Task,
            StepDef::Group(_) => StepKind::Group,
        };
        out.push((path.clone(), kind, step));
        if let StepDef::Group(group) = step {
            collect(&group.tasks, &path.join("tasks"), setup, out);
        }
    }
}

/// One element of a step list. An element carrying a `tasks` key is a group.
///
/// Not `#[non_exhaustive]`: the step grammar is closed, and a match on it is
/// a decision about every kind of step.
#[derive(Clone, Debug, PartialEq)]
pub enum StepDef {
    /// A task: one function call.
    Task(TaskDef),
    /// A group: a span of steps sharing one condition.
    Group(GroupDef),
}

impl StepDef {
    /// The step's id. Tasks and groups share one namespace.
    pub fn id(&self) -> &str {
        match self {
            Self::Task(task) => &task.id,
            Self::Group(group) => &group.id,
        }
    }
}

/// A task: one function call, with its outcome policies.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct TaskDef {
    /// The step id, unique across the workflow's steps and setup.
    pub id: String,
    /// The name, description and `ext`.
    pub header: NodeHeader,
    /// Whether the task runs. Absent means true.
    pub condition: Option<Logic>,
    /// The function the task runs, and its raw input.
    pub function: FunctionRef,
    /// What a failure (a 5xx or an `Err`) does. Default `fail`.
    pub on_failure: FailurePolicy,
    /// What a rejection (a 4xx) does. Default `continue`.
    pub on_rejection: RejectionPolicy,
    /// Whether running the task ends the workflow, applied after the
    /// policies. Default false.
    pub terminal: bool,
    /// Run the function once per element of an array. Refused on a built-in.
    pub for_each: Option<ForEachDef>,
}

/// A group: a span of steps sharing one condition, evaluated once on entry.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct GroupDef {
    /// The step id, unique across the workflow's steps and setup.
    pub id: String,
    /// The name, description and `ext`.
    pub header: NodeHeader,
    /// Whether the span runs. Absent means true.
    pub condition: Option<Logic>,
    /// Whether reaching the end of the group ends the workflow.
    pub terminal: bool,
    /// The members: never empty, nested at most
    /// [`MAX_GROUP_DEPTH`](super::MAX_GROUP_DEPTH) deep.
    pub tasks: Vec<StepDef>,
}

/// A workflow's bounded loop: the task list runs once per sweep while
/// `counter < max`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LoopDef {
    /// The `temp_data` slot the engine writes the counter to, if any.
    pub counter: Option<String>,
    /// The first counter value. Default 0.
    pub init: i64,
    /// Added to the counter after each sweep: at least 1. Default 1.
    pub increment: i64,
    /// The exclusive upper bound, above `init`. Required.
    pub max: i64,
    /// Steps run once, before the first sweep, with no counter.
    pub setup: Vec<StepDef>,
    /// The array iterated, evaluated once after setup.
    pub over: Option<Logic>,
    /// The `temp_data` slot holding `over[counter]`. Requires `over`.
    pub r#as: Option<String>,
    /// The `temp_data` slot reset to `{}` before every sweep.
    pub scratch: Option<String>,
}

/// A task's fan-out: its function runs once per element of an array.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ForEachDef {
    /// The array, evaluated once after the task's condition.
    pub over: Logic,
    /// The `temp_data` slot holding the element; its index is at
    /// `temp_data.<as>_index`.
    pub r#as: String,
    /// Calls in flight at once: timing only, never results. Default 1.
    pub max_concurrency: usize,
    /// The context path each call writes its result to. Requires `into`.
    pub collect: Option<String>,
    /// The context path receiving the results in element order. Requires
    /// `collect`.
    pub into: Option<String>,
}

/// The function a task runs.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct FunctionRef {
    /// The registered name.
    pub name: String,
    /// The config, raw until a compiler reads it. Default `{}`.
    pub input: Value,
}

/// A spelling table for an enum read from a string.
pub(super) trait Spelling: Copy + Default + 'static {
    /// Every variant, in the order the messages list them.
    const ALL: &'static [Self];

    /// The variant's authored spelling.
    fn spelling(self) -> &'static str;

    /// The variant spelled `text`, if any.
    fn from_spelling(text: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|v| v.spelling() == text)
    }
}

/// A workflow's lifecycle status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Status {
    /// Runs. The default.
    #[default]
    Active,
    /// Kept, not run.
    Paused,
    /// Retired.
    Archived,
}

impl Status {
    /// The authored spelling: `"active"`, `"paused"` or `"archived"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Archived => "archived",
        }
    }
}

impl Spelling for Status {
    const ALL: &'static [Self] = &[Self::Active, Self::Paused, Self::Archived];
    fn spelling(self) -> &'static str {
        self.as_str()
    }
}

/// What a task's failure (a 5xx or an `Err`) does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FailurePolicy {
    /// The failure passes to the workflow's
    /// [`on_failure`](WorkflowDef::on_failure). The default.
    #[default]
    Fail,
    /// The workflow stops, and the run goes on.
    Halt,
    /// The failure is recorded and the next task runs.
    Continue,
}

impl FailurePolicy {
    /// The authored spelling: `"fail"`, `"halt"` or `"continue"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::Halt => "halt",
            Self::Continue => "continue",
        }
    }
}

impl Spelling for FailurePolicy {
    const ALL: &'static [Self] = &[Self::Fail, Self::Halt, Self::Continue];
    fn spelling(self) -> &'static str {
        self.as_str()
    }
}

/// What a task's rejection (a 4xx) does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RejectionPolicy {
    /// The rejection is recorded and the next task runs. The default.
    #[default]
    Continue,
    /// The workflow stops, and the run goes on.
    Halt,
}

impl RejectionPolicy {
    /// The authored spelling: `"continue"` or `"halt"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Halt => "halt",
        }
    }
}

impl Spelling for RejectionPolicy {
    const ALL: &'static [Self] = &[Self::Continue, Self::Halt];
    fn spelling(self) -> &'static str {
        self.as_str()
    }
}

/// What a failure that reaches the workflow does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WorkflowFailurePolicy {
    /// The run stops. The default.
    #[default]
    Fail,
    /// The next sweep, or the next workflow, runs.
    Continue,
}

impl WorkflowFailurePolicy {
    /// The authored spelling: `"fail"` or `"continue"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::Continue => "continue",
        }
    }
}

impl Spelling for WorkflowFailurePolicy {
    const ALL: &'static [Self] = &[Self::Fail, Self::Continue];
    fn spelling(self) -> &'static str {
        self.as_str()
    }
}
