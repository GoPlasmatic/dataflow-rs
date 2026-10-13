//! Call layers: around-call middleware for host calls, and the rest of the
//! chain each one is handed.

use super::erased::BoundCall;
use super::{Ctx, Outcome};
use crate::base::Result;
use std::pin::Pin;
use std::sync::Arc;

/// A boxed, `Send` future: what a [`CallLayer`] returns.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Around-call middleware for host calls.
///
/// A layer on the [`Registry`](super::Registry) wraps every host call of
/// every run. The built-ins run inline and are never wrapped. A layer calls
/// [`Next::run`] to go on, or answers without calling it.
///
/// ```
/// # extern crate dataflow_rs_v4 as dataflow_rs;
/// use dataflow_rs::function::{BoxFuture, CallLayer, Next};
/// use dataflow_rs::{Ctx, Outcome, Registry, Result};
///
/// /// Turns any `5xx` into a `503`.
/// struct Unavailable;
///
/// impl CallLayer for Unavailable {
///     fn call<'a>(
///         &'a self,
///         ctx: &'a mut Ctx<'_>,
///         next: Next<'a>,
///     ) -> BoxFuture<'a, Result<Outcome>> {
///         Box::pin(async move {
///             Ok(match next.run(ctx).await? {
///                 Outcome::Status(500..) => Outcome::Status(503),
///                 outcome => outcome,
///             })
///         })
///     }
/// }
///
/// let mut registry = Registry::default();
/// registry.layer(Unavailable);
/// ```
pub trait CallLayer: Send + Sync + 'static {
    /// Wraps one call. `next` is the rest of the chain.
    fn call<'a>(&'a self, ctx: &'a mut Ctx<'_>, next: Next<'a>) -> BoxFuture<'a, Result<Outcome>>;
}

/// The rest of a call's chain: the run's layers, then the registry's, then
/// the function. Each list runs outermost first.
pub struct Next<'a> {
    run: &'a [Arc<dyn CallLayer>],
    registry: &'a [Arc<dyn CallLayer>],
    call: &'a dyn BoundCall,
}

impl<'a> Next<'a> {
    /// The chain of `run`'s layers, then `registry`'s, around `call`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used by the runtime, which starts each host call")
    )]
    pub(crate) fn new(
        run: &'a [Arc<dyn CallLayer>],
        registry: &'a [Arc<dyn CallLayer>],
        call: &'a dyn BoundCall,
    ) -> Self {
        Self {
            run,
            registry,
            call,
        }
    }

    /// Runs the rest of the chain: the next layer, or the function when no
    /// layer is left.
    pub fn run<'b>(self, ctx: &'b mut Ctx<'_>) -> BoxFuture<'b, Result<Outcome>>
    where
        'a: 'b,
    {
        if let Some((layer, run)) = self.run.split_first() {
            layer.call(ctx, Next { run, ..self })
        } else if let Some((layer, registry)) = self.registry.split_first() {
            layer.call(ctx, Next { registry, ..self })
        } else {
            self.call.call(ctx)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{ProgramId, TaskIx, WorkflowIx};
    use crate::expr::{CompileCx, ExprEngine};
    use crate::function::registry::Entry;
    use crate::function::{CallInfo, Function, Prepare, Registry};
    use serde_json::json;
    use std::sync::Mutex;
    use std::task::{Context, Poll, Waker};

    type Log = Arc<Mutex<Vec<&'static str>>>;

    /// Polls once: every future in these tests is ready at once.
    fn ready<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(out) => out,
            Poll::Pending => panic!("a future in these tests is never pending"),
        }
    }

    fn info() -> CallInfo<'static> {
        CallInfo {
            program: ProgramId::next(),
            workflow: WorkflowIx::new(0),
            task: TaskIx::new(1),
            function: "probe",
            workflow_id: "w",
            task_id: "t",
            channel: None,
            loop_counter: None,
            element_index: None,
        }
    }

    /// Logs its call, and answers 201.
    struct Probe(Log);

    impl Function for Probe {
        type Config = ();
        async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
            assert_eq!(ctx.info().task_id, "t");
            self.0.lock().unwrap().push("function");
            Ok(Outcome::Status(201))
        }
    }

    /// Logs its name around the rest of the chain.
    struct Named(&'static str, Log);

    impl CallLayer for Named {
        fn call<'a>(
            &'a self,
            ctx: &'a mut Ctx<'_>,
            next: Next<'a>,
        ) -> BoxFuture<'a, Result<Outcome>> {
            Box::pin(async move {
                self.1.lock().unwrap().push(self.0);
                let outcome = next.run(ctx).await;
                self.1.lock().unwrap().push(self.0);
                outcome
            })
        }
    }

    /// Answers without calling the rest of the chain.
    struct Answer(Log);

    impl CallLayer for Answer {
        fn call<'a>(
            &'a self,
            _ctx: &'a mut Ctx<'_>,
            _next: Next<'a>,
        ) -> BoxFuture<'a, Result<Outcome>> {
            self.0.lock().unwrap().push("answer");
            Box::pin(std::future::ready(Ok(Outcome::Skip)))
        }
    }

    /// The probe's call site, bound through the registry as the compiler
    /// binds one.
    fn bound(log: &Log) -> (Registry, Box<dyn BoundCall>) {
        let mut registry = Registry::default();
        registry.register("probe", Probe(Arc::clone(log)));
        registry.layer(Named("registry-outer", Arc::clone(log)));
        registry.layer(Named("registry-inner", Arc::clone(log)));
        let Some(Entry::Host(probe)) = registry.entry("probe") else {
            panic!("probe is registered");
        };
        let engine = ExprEngine::default();
        let mut issues = Vec::new();
        let mut cx = CompileCx::new(&engine, &mut issues);
        let spec = probe.spec().clone();
        let call = probe
            .compile(&json!({}), &mut cx, &Prepare::new("probe", "w", "t", &spec))
            .expect("an empty config compiles");
        drop(cx);
        assert!(issues.is_empty(), "{issues:?}");
        (registry, call)
    }

    #[test]
    fn the_run_layers_wrap_the_registry_layers_which_wrap_the_function() {
        let log = Log::default();
        let (registry, call) = bound(&log);
        let run: Vec<Arc<dyn CallLayer>> = vec![
            Arc::new(Named("run-outer", Arc::clone(&log))),
            Arc::new(Named("run-inner", Arc::clone(&log))),
        ];
        let mut ctx = Ctx::new(info());
        let next = Next::new(&run, registry.layers(), &*call);
        assert_eq!(ready(next.run(&mut ctx)).unwrap(), Outcome::Status(201));
        assert_eq!(
            *log.lock().unwrap(),
            [
                "run-outer",
                "run-inner",
                "registry-outer",
                "registry-inner",
                "function",
                "registry-inner",
                "registry-outer",
                "run-inner",
                "run-outer",
            ]
        );
    }

    #[test]
    fn with_no_layers_the_function_is_called() {
        let log = Log::default();
        let (_, call) = bound(&log);
        let mut ctx = Ctx::new(info());
        let next = Next::new(&[], &[], &*call);
        assert_eq!(ready(next.run(&mut ctx)).unwrap(), Outcome::Status(201));
        assert_eq!(*log.lock().unwrap(), ["function"]);
    }

    #[test]
    fn a_layer_that_answers_runs_nothing_inside_it() {
        let log = Log::default();
        let (registry, call) = bound(&log);
        let run: Vec<Arc<dyn CallLayer>> = vec![Arc::new(Answer(Arc::clone(&log)))];
        let mut ctx = Ctx::new(info());
        let next = Next::new(&run, registry.layers(), &*call);
        assert_eq!(ready(next.run(&mut ctx)).unwrap(), Outcome::Skip);
        assert_eq!(*log.lock().unwrap(), ["answer"]);
    }
}
