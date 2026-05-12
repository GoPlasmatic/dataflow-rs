use crate::engine::error::{DataflowError, Result};
use crate::engine::task_context::TaskContext;
use crate::engine::task_outcome::TaskOutcome;
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::any::Any;

pub mod config;
pub use config::{CompiledCustomInput, FunctionConfig};

pub mod validation;
pub use validation::{ValidationConfig, ValidationRule};

pub mod map;
pub use map::{MapConfig, MapMapping};

pub mod parse;
pub use parse::ParseConfig;

pub mod publish;
pub use publish::PublishConfig;

pub mod filter;
pub use filter::{FilterConfig, RejectAction};

pub mod log;
pub use log::{LogConfig, LogLevel};

pub mod integration;
pub use integration::{EnrichConfig, HttpCallConfig, PublishKafkaConfig};

/// Async interface for task functions that operate on messages.
///
/// Implement this trait for custom processing logic. The trait associates a
/// typed `Input` deserialized from the task's `FunctionConfig` so that
/// handlers receive their config already parsed — no `match
/// FunctionConfig::Custom { input, .. }` boilerplate, no per-call
/// `serde_json::from_value` cost in the hot path. The engine deserializes the
/// `Custom.input` JSON exactly once at `Engine::new()` time and caches the
/// typed value alongside the task; mismatched config shapes therefore fail
/// at startup rather than on first message.
///
/// Handlers mutate the message via [`TaskContext`] — its `set` family records
/// changes on the audit trail automatically when `message.capture_changes`
/// is enabled, so handlers don't have to hand-build [`crate::engine::message::Change`]
/// entries.
///
/// ## Example
///
/// ```rust,no_run
/// use async_trait::async_trait;
/// use dataflow_rs::{
///     AsyncFunctionHandler, Result, TaskContext, TaskOutcome,
/// };
/// use datavalue::OwnedDataValue;
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct StatsInput {
///     data_path: String,
///     output_path: String,
/// }
///
/// struct StatisticsFunction;
///
/// #[async_trait]
/// impl AsyncFunctionHandler for StatisticsFunction {
///     type Input = StatsInput;
///
///     async fn execute(
///         &self,
///         ctx: &mut TaskContext<'_>,
///         input: &StatsInput,
///     ) -> Result<TaskOutcome> {
///         let count = ctx.data()
///             .get(input.data_path.as_str())
///             .and_then(|v| v.as_array())
///             .map(|a| a.len())
///             .unwrap_or(0);
///         ctx.set(
///             &format!("data.{}.count", input.output_path),
///             OwnedDataValue::from(&serde_json::json!(count)),
///         );
///         Ok(TaskOutcome::Success)
///     }
/// }
/// ```
#[async_trait]
pub trait AsyncFunctionHandler: Send + Sync + 'static {
    /// Typed configuration shape for this handler. Use
    /// `serde_json::Value` for handlers that take freeform JSON.
    type Input: DeserializeOwned + Send + Sync + 'static;

    /// Parse the raw `FunctionConfig::Custom { input }` JSON into
    /// `Self::Input`. Default impl uses `serde_json::from_value`. Override
    /// only if you need custom validation beyond what serde provides.
    ///
    /// Built-in async function variants (`HttpCall`, `Enrich`,
    /// `PublishKafka`) bypass this method — their typed configs are already
    /// parsed by `serde(untagged)` on `FunctionConfig` and dispatched
    /// directly to the registered handler.
    fn parse_input(input: &Value) -> Result<Self::Input> {
        serde_json::from_value(input.clone()).map_err(DataflowError::from_serde)
    }

    /// Execute the handler. The `ctx` accumulates audit-trail changes
    /// pushed via its `set` family; the workflow executor folds them into
    /// the audit trail when this method returns.
    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &Self::Input) -> Result<TaskOutcome>;
}

/// Object-safe sibling of [`AsyncFunctionHandler`]. Engine-internal — users
/// should not implement this directly; the blanket impl below derives it
/// for any `AsyncFunctionHandler`. Exposed (rather than `pub(crate)`) only
/// because [`BoxedFunctionHandler`] mentions it in its public type alias.
#[doc(hidden)]
#[async_trait]
pub trait DynAsyncFunctionHandler: Send + Sync + 'static {
    /// Pre-parse the raw JSON input into the handler's typed shape and box
    /// it as `dyn Any`. Called once per task at `Engine::new()` time.
    fn parse_input_box(&self, input: &Value) -> Result<Box<dyn Any + Send + Sync>>;

    /// Execute against an already-parsed typed input. The implementation
    /// downcasts `input` to `<Self as AsyncFunctionHandler>::Input`; the
    /// downcast is infallible in the engine's call paths because
    /// `parse_input_box` produced the very same type.
    async fn dyn_execute(
        &self,
        ctx: &mut TaskContext<'_>,
        input: &(dyn Any + Send + Sync),
    ) -> Result<TaskOutcome>;
}

#[async_trait]
impl<F: AsyncFunctionHandler> DynAsyncFunctionHandler for F {
    fn parse_input_box(&self, input: &Value) -> Result<Box<dyn Any + Send + Sync>> {
        let typed = <F as AsyncFunctionHandler>::parse_input(input)?;
        Ok(Box::new(typed))
    }

    async fn dyn_execute(
        &self,
        ctx: &mut TaskContext<'_>,
        input: &(dyn Any + Send + Sync),
    ) -> Result<TaskOutcome> {
        let typed = input.downcast_ref::<F::Input>().ok_or_else(|| {
            DataflowError::Validation(format!(
                "Handler input type mismatch (expected {})",
                std::any::type_name::<F::Input>()
            ))
        })?;
        AsyncFunctionHandler::execute(self, ctx, typed).await
    }
}

/// Boxed handler stored in the engine's function registry. Users construct
/// these with `Box::new(MyHandler)` — the blanket impl above auto-coerces
/// any `AsyncFunctionHandler` into `Box<dyn DynAsyncFunctionHandler + Send + Sync>`.
pub type BoxedFunctionHandler = Box<dyn DynAsyncFunctionHandler + Send + Sync>;
