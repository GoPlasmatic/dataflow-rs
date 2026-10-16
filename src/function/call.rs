//! `Function`: the trait a host function implements, and what `prepare`
//! is told.

use super::{Ctx, FunctionConfig, FunctionSpec, Outcome};
use crate::base::{Issue, Result};

/// A host function: a typed config compiled once at build, and a call per
/// task run.
///
/// `call` returns a `Send` future, so an implementation may be written with
/// `async fn`. [`testing::call`](super::testing::call) runs one call of it
/// outside any workflow:
///
/// ```
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// use dataflow_rs::datavalue::OwnedDataValue;
/// use dataflow_rs::expr::Output;
/// use dataflow_rs::function::{FunctionConfig, FunctionSpec, testing};
/// use dataflow_rs::{Ctx, Expr, Function, Outcome, Registry, Result};
/// use serde_json::json;
///
/// #[derive(FunctionConfig)]
/// struct FetchConfig {
///     url: Expr<String>,
///     #[output(default = "data.response")]
///     output: Output,
/// }
///
/// struct Fetch;
///
/// impl Function for Fetch {
///     type Config = FetchConfig;
///
///     fn spec(&self) -> FunctionSpec {
///         FunctionSpec::default().with_category("io")
///     }
///
///     async fn call(&self, ctx: &mut Ctx<'_>, cfg: &FetchConfig) -> Result<Outcome> {
///         let url = ctx.eval(&cfg.url)?;
///         // A real fetch awaits its response here.
///         let response = OwnedDataValue::from(&json!({"url": &*url, "status": 200}));
///         ctx.emit(&cfg.output, response)?;
///         Ok(Outcome::Ok)
///     }
/// }
///
/// let mut registry = Registry::default();
/// registry.register("fetch", Fetch);
///
/// let called = testing::call(Fetch, json!({"url": "https://example.com"})).await;
/// assert_eq!(called.outcome.unwrap(), Outcome::Ok);
/// let status = called.message.context().get(&"data.response.status".parse().unwrap());
/// assert_eq!(status, Some(&OwnedDataValue::from(&json!(200))));
/// # });
/// ```
pub trait Function: Send + Sync + 'static {
    /// The function's `input`, compiled at build.
    type Config: FunctionConfig;

    /// What the function says about itself, for tooling. Called once, when
    /// it is registered.
    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default()
    }

    /// Checks or completes a compiled config at build, with what the config
    /// alone cannot know. It runs only when the config compiled.
    ///
    /// Issue paths are relative to the config. The compiler prefixes the
    /// task's `function.input` path and stamps the step, and a Rejected
    /// issue fails the task's compile.
    fn prepare(
        &self,
        _cfg: &mut Self::Config,
        _p: &Prepare<'_>,
    ) -> std::result::Result<(), Vec<Issue>> {
        Ok(())
    }

    /// Runs one call.
    fn call(
        &self,
        ctx: &mut Ctx<'_>,
        cfg: &Self::Config,
    ) -> impl Future<Output = Result<Outcome>> + Send;
}

/// What [`Function::prepare`] is told about the call site it prepares.
///
/// Its fields are private, behind getters, so it can grow.
#[derive(Debug)]
pub struct Prepare<'a> {
    name: &'a str,
    workflow_id: &'a str,
    task_id: &'a str,
    spec: &'a FunctionSpec,
}

impl<'a> Prepare<'a> {
    /// What `prepare` is told about the task `task_id` of the workflow
    /// `workflow_id`, which calls the function registered as `name` with
    /// `spec`.
    pub(crate) fn new(
        name: &'a str,
        workflow_id: &'a str,
        task_id: &'a str,
        spec: &'a FunctionSpec,
    ) -> Self {
        Self {
            name,
            workflow_id,
            task_id,
            spec,
        }
    }

    /// The name the function is registered under.
    pub fn name(&self) -> &'a str {
        self.name
    }

    /// The id of the workflow the call site is in.
    pub fn workflow_id(&self) -> &'a str {
        self.workflow_id
    }

    /// The id of the task that calls the function.
    pub fn task_id(&self) -> &'a str {
        self.task_id
    }

    /// The function's spec, as it was when registered.
    pub fn spec(&self) -> &'a FunctionSpec {
        self.spec
    }
}
