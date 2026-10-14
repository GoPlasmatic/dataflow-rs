//! Ready-made [`CallLayer`]s.

use super::{BoxFuture, CallInfo, CallLayer, Ctx, Next, Outcome};
use crate::base::Result;
use datavalue::OwnedDataValue;
use std::fmt;

/// What an [`Answer`] asks of each call.
type Answering = dyn Fn(&CallInfo<'_>) -> Option<OwnedDataValue> + Send + Sync;

/// Answers calls from a closure, for a dry run.
///
/// The closure is asked about each call it wraps. `Some(value)` is written
/// with [`Ctx::emit_default`], through the callee's primary output, and the
/// call returns [`Outcome::Ok`] without reaching the layers inside this one
/// or the function. `None` passes the call on. A callee with no primary
/// output fails the call with a `Config` error.
///
/// ```
/// # extern crate dataflow_rs_v4 as dataflow_rs;
/// use dataflow_rs::datavalue::OwnedDataValue;
/// use dataflow_rs::function::{CallLayer, layers::Answer};
/// use dataflow_rs::RunOptions;
/// use std::sync::Arc;
///
/// // Every `http_call` answers `{"status": "dry run"}`; the rest run.
/// let dry_run: Vec<Arc<dyn CallLayer>> = vec![Arc::new(Answer::new(|info| {
///     (info.function == "http_call").then(|| {
///         OwnedDataValue::Object(vec![("status".into(), "dry run".into())])
///     })
/// }))];
/// let opts = RunOptions::default().layers(&dry_run);
/// ```
pub struct Answer {
    answer: Box<Answering>,
}

impl Answer {
    /// A layer answering each call `f` gives a value for.
    pub fn new(
        f: impl Fn(&CallInfo<'_>) -> Option<OwnedDataValue> + Send + Sync + 'static,
    ) -> Self {
        Self {
            answer: Box::new(f),
        }
    }
}

impl CallLayer for Answer {
    fn call<'a>(&'a self, ctx: &'a mut Ctx<'_>, next: Next<'a>) -> BoxFuture<'a, Result<Outcome>> {
        match (self.answer)(ctx.info()) {
            Some(value) => Box::pin(std::future::ready(
                ctx.emit_default(value).map(|()| Outcome::Ok),
            )),
            None => next.run(ctx),
        }
    }
}

impl fmt::Debug for Answer {
    /// Nothing of the closure.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Answer").finish_non_exhaustive()
    }
}
