use crate::engine::error::{DataflowError, Result};
use crate::engine::task_context::TaskContext;
use crate::engine::task_outcome::TaskOutcome;
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::any::Any;

pub mod config;
pub use config::{
    BUILTIN_FUNCTION_NAMES, BuiltinKind, CompiledCustomInput, ConnectorName, DispatchableFunction,
    FunctionConfig, builtin_function_kind, is_builtin_function,
};

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
pub use integration::{EnrichConfig, HttpCallConfig, HttpMethod, PublishKafkaConfig};

pub mod template;
pub use template::{Template, TemplateCompiler};

pub mod path_template;
pub use path_template::{ContextRoot, DataRoot, PathRoot, PathTemplate, ResolvedPath};

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
    /// The engine calls [`Self::parse_input_with`], whose default delegates
    /// here; override that one instead when the parse depends on the instance.
    ///
    /// Built-in async function variants (`HttpCall`, `Enrich`,
    /// `PublishKafka`) bypass this method — their typed configs are already
    /// parsed by `serde(untagged)` on `FunctionConfig` and dispatched
    /// directly to the registered handler.
    fn parse_input(input: &Value) -> Result<Self::Input> {
        serde_json::from_value(input.clone()).map_err(DataflowError::from_serde)
    }

    /// Receiver-taking form of [`Self::parse_input`], and the one the engine
    /// calls — once per task at `Engine::new` / `Engine::builder().build()` /
    /// `Engine::with_new_workflows`, and from
    /// [`EngineBuilder::check_workflow`](crate::EngineBuilder::check_workflow).
    /// The default delegates to `parse_input`, so overriding both leaves
    /// `parse_input` unreached unless this calls it.
    ///
    /// Override it when the parse depends on `self`: one handler type
    /// registered under several names, each carrying its own schema — a
    /// plugin host with one instance per manifest function, say.
    ///
    /// # Errors
    ///
    /// As [`Self::parse_input`].
    fn parse_input_with(&self, input: &Value) -> Result<Self::Input> {
        <Self as AsyncFunctionHandler>::parse_input(input)
    }

    /// Compile the [`Template`] fields of a just-parsed input.
    ///
    /// Called once per task at engine construction, immediately after
    /// [`Self::parse_input_with`]. The default is a no-op, so a handler with no
    /// `Template` fields needs no implementation.
    ///
    /// The engine calls [`Self::compile_input_with`], whose default delegates
    /// here; override that one instead when *which* fields are templates
    /// depends on the instance.
    ///
    /// A malformed expression fails here — at `Engine::new` / `Engine::builder().build()`
    /// / `Engine::with_new_workflows` — rather than on the first message that
    /// reaches the task, matching the crate's existing stance for the built-in
    /// `*_logic` fields. A host that loads workflows from a database and must
    /// not let one bad row take the whole process down needs a per-row
    /// pre-check before activation; this method does not change that trade-off,
    /// only makes it apply to custom handlers too.
    ///
    /// # Errors
    ///
    /// Propagate whatever [`Template::compile`] returns — typically
    /// [`crate::DataflowError::LogicEvaluation`].
    fn compile_input(_input: &mut Self::Input, _c: &TemplateCompiler) -> Result<()> {
        Ok(())
    }

    /// Receiver-taking form of [`Self::compile_input`], same rule as
    /// [`Self::parse_input_with`]: the engine calls this one, the default
    /// delegates, and overriding both leaves `compile_input` unreached unless
    /// this calls it. Override it when the set of template positions is
    /// per-registration data rather than a property of `Self::Input`.
    ///
    /// # Errors
    ///
    /// As [`Self::compile_input`].
    fn compile_input_with(&self, input: &mut Self::Input, c: &TemplateCompiler) -> Result<()> {
        <Self as AsyncFunctionHandler>::compile_input(input, c)
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

    /// Compile the [`Template`] fields of an already-parsed boxed input, in
    /// place. Defaulted to a no-op so a hand-written impl of this
    /// `#[doc(hidden)]` trait — which is not expected to exist — keeps
    /// compiling regardless.
    fn compile_input_box(
        &self,
        _boxed: &mut (dyn Any + Send + Sync),
        _c: &TemplateCompiler,
    ) -> Result<()> {
        Ok(())
    }

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
        let typed = <F as AsyncFunctionHandler>::parse_input_with(self, input)?;
        Ok(Box::new(typed))
    }

    fn compile_input_box(
        &self,
        boxed: &mut (dyn Any + Send + Sync),
        c: &TemplateCompiler,
    ) -> Result<()> {
        let typed = boxed.downcast_mut::<F::Input>().ok_or_else(|| {
            DataflowError::Validation(format!(
                "Handler input type mismatch (expected {})",
                std::any::type_name::<F::Input>()
            ))
        })?;
        <F as AsyncFunctionHandler>::compile_input_with(self, typed, c)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::compiler::LogicCompiler;

    /// Overrides both forms of both hooks, with the associated ones refusing.
    /// The boxed dispatch is the only path the engine has to a handler, so
    /// reaching the receiver forms through it — and never the associated
    /// ones — is the whole precedence rule.
    struct BothForms;

    #[async_trait]
    impl AsyncFunctionHandler for BothForms {
        type Input = Value;

        fn parse_input(_input: &Value) -> Result<Self::Input> {
            Err(DataflowError::Validation(
                "the associated parse_input must not be reached".to_string(),
            ))
        }

        fn parse_input_with(&self, input: &Value) -> Result<Self::Input> {
            Ok(input.clone())
        }

        fn compile_input(_input: &mut Self::Input, _c: &TemplateCompiler) -> Result<()> {
            Err(DataflowError::Validation(
                "the associated compile_input must not be reached".to_string(),
            ))
        }

        fn compile_input_with(
            &self,
            _input: &mut Self::Input,
            _c: &TemplateCompiler,
        ) -> Result<()> {
            Ok(())
        }

        async fn execute(
            &self,
            _ctx: &mut TaskContext<'_>,
            _input: &Self::Input,
        ) -> Result<TaskOutcome> {
            Ok(TaskOutcome::Success)
        }
    }

    #[test]
    fn the_boxed_dispatch_calls_the_receiver_forms_and_never_the_associated_ones() {
        let handler: BoxedFunctionHandler = Box::new(BothForms);
        let compiler = TemplateCompiler::new(LogicCompiler::new().engine());

        let mut parsed = handler
            .parse_input_box(&Value::Null)
            .expect("parse_input_box routes to parse_input_with");
        handler
            .compile_input_box(&mut *parsed, &compiler)
            .expect("compile_input_box routes to compile_input_with");
    }
}
