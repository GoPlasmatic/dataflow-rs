//! A host function, erased once at `register`, and a host call site's
//! callee, bound once at compile (I42).

use super::{BoxFuture, Ctx, Function, FunctionConfig, FunctionSpec, Outcome, Prepare};
use crate::base::Result;
use crate::expr::{CompileCx, ExprVisitor, FieldSpec};
use serde_json::Value;
use std::sync::Arc;

/// A registry entry: one host function, erased.
pub(crate) trait ErasedFunction: Send + Sync {
    /// [`Function::spec`], as it was when the function was registered.
    fn spec(&self) -> &FunctionSpec;

    /// The config's fields: the spec's runtime fields when it declares them,
    /// else the config's own.
    fn fields(&self) -> &[FieldSpec];

    /// Compiles the config at `cx` and, when it compiled with no Rejected
    /// issue, calls [`Function::prepare`], adopting each issue it returns
    /// below `cx`'s path. `None` whenever anything Rejected was recorded,
    /// by the config or by `prepare`, and whenever the config refused.
    ///
    /// The caller roots `cx` at the task's `function.input`, stamps the step,
    /// and gives it the spec's runtime fields.
    fn compile(
        &self,
        input: &Value,
        cx: &mut CompileCx<'_>,
        p: &Prepare<'_>,
    ) -> Option<Box<dyn BoundCall>>;
}

/// A host call site's callee: the function and its typed config, so a call
/// is one virtual call with no downcast.
pub(crate) trait BoundCall: Send + Sync {
    /// Runs one call of the function with its config.
    fn call<'a>(&'a self, ctx: &'a mut Ctx<'_>) -> BoxFuture<'a, Result<Outcome>>;

    /// Walks the config's expressions; paths are relative to the config.
    fn visit(&self, v: &mut dyn ExprVisitor);
}

/// A registered function with the spec it gave at `register`.
pub(crate) struct Erased<F: Function> {
    f: Arc<F>,
    spec: FunctionSpec,
}

impl<F: Function> Erased<F> {
    /// Erases `f`, asking it for its spec once.
    pub(crate) fn new(f: F) -> Self {
        let spec = f.spec();
        Self {
            f: Arc::new(f),
            spec,
        }
    }
}

impl<F: Function> ErasedFunction for Erased<F> {
    fn spec(&self) -> &FunctionSpec {
        &self.spec
    }

    fn fields(&self) -> &[FieldSpec] {
        match &self.spec.runtime_fields {
            Some(fields) => fields,
            None => F::Config::fields(),
        }
    }

    fn compile(
        &self,
        input: &Value,
        cx: &mut CompileCx<'_>,
        p: &Prepare<'_>,
    ) -> Option<Box<dyn BoundCall>> {
        // The baseline comes first, so a hand-written config that records a
        // Rejected issue and still returns a config is not prepared or bound.
        let before = cx.rejected();
        let mut cfg = <F::Config as FunctionConfig>::compile(input, cx)
            .filter(|_| cx.rejected() == before)?;
        if let Err(issues) = self.f.prepare(&mut cfg, p) {
            for issue in issues {
                cx.adopt(issue);
            }
        }
        (cx.rejected() == before).then(|| {
            Box::new(Bound {
                f: Arc::clone(&self.f),
                cfg,
            }) as Box<dyn BoundCall>
        })
    }
}

/// A host function bound to one call site's config.
struct Bound<F: Function> {
    f: Arc<F>,
    cfg: F::Config,
}

impl<F: Function> BoundCall for Bound<F> {
    fn call<'a>(&'a self, ctx: &'a mut Ctx<'_>) -> BoxFuture<'a, Result<Outcome>> {
        Box::pin(self.f.call(ctx, &self.cfg))
    }

    fn visit(&self, v: &mut dyn ExprVisitor) {
        self.cfg.visit(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{AuthoredPath, Issue, IssueCode, Severity, StepKind};
    use crate::expr::{Expr, ExprEngine, FieldKind, Output};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(FunctionConfig)]
    struct FetchConfig {
        connector: String,
        #[config(records)]
        url: Expr<String>,
        #[output(default = "data.response")]
        output: Output,
    }

    /// Counts its `prepare` calls, and refuses the connector `"db2"`.
    #[derive(Default)]
    struct Fetch {
        prepared: AtomicUsize,
    }

    impl Function for Fetch {
        type Config = FetchConfig;

        fn spec(&self) -> FunctionSpec {
            FunctionSpec::default().with_category("io")
        }

        fn prepare(
            &self,
            cfg: &mut FetchConfig,
            p: &Prepare<'_>,
        ) -> std::result::Result<(), Vec<Issue>> {
            self.prepared.fetch_add(1, Ordering::Relaxed);
            assert_eq!(
                (p.name(), p.workflow_id(), p.task_id()),
                ("fetch", "w", "t")
            );
            assert_eq!(p.spec().category, "io");
            let mut issues = vec![Issue::new(
                IssueCode::UnknownKey,
                "retries".parse().unwrap(),
                "advisory only",
            )];
            if cfg.connector == "db2" {
                issues.push(Issue::new(
                    IssueCode::InputParse,
                    "connector".parse().unwrap(),
                    "no connector `db2`",
                ));
            }
            Err(issues)
        }

        async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &FetchConfig) -> Result<Outcome> {
            Ok(Outcome::Ok)
        }
    }

    struct Compiled {
        bound: Option<Box<dyn BoundCall>>,
        issues: Vec<Issue>,
        prepared: usize,
    }

    fn compile(input: &Value) -> Compiled {
        let erased = Erased::new(Fetch::default());
        let engine = ExprEngine::default();
        let mut issues = Vec::new();
        let bound = {
            let mut cx = CompileCx::new(&engine, &mut issues)
                .at_path("tasks[0].function.input".parse().unwrap())
                .for_step(StepKind::Task, "t");
            let p = Prepare::new("fetch", "w", "t", erased.spec());
            erased.compile(input, &mut cx, &p)
        };
        let prepared = erased.f.prepared.load(Ordering::Relaxed);
        Compiled {
            bound,
            issues,
            prepared,
        }
    }

    #[derive(Default)]
    struct Paths(Vec<(String, bool)>);

    impl ExprVisitor for Paths {
        fn expr(&mut self, at: &AuthoredPath, _raw: &Value, records: bool) {
            self.0.push((at.to_string(), records));
        }
    }

    #[test]
    fn a_compiled_config_is_prepared_and_bound() {
        let out = compile(&json!({"connector": "db1", "url": {"var": "data.url"}}));
        assert_eq!(out.prepared, 1);
        // The Advisory is adopted below the input, stamped, and does not fail
        // the call site.
        assert_eq!(out.issues.len(), 1, "{:?}", out.issues);
        let issue = &out.issues[0];
        assert_eq!(issue.severity, Severity::Advisory);
        assert_eq!(issue.path.to_string(), "tasks[0].function.input.retries");
        assert_eq!(issue.field.as_deref(), Some("retries"));
        assert_eq!(issue.step_id.as_deref(), Some("t"));

        let mut paths = Paths::default();
        out.bound.expect("bound").visit(&mut paths);
        assert_eq!(
            paths.0,
            [("url".to_owned(), true), ("output".to_owned(), false)]
        );
    }

    #[test]
    fn a_rejected_prepare_issue_fails_the_call_site() {
        let out = compile(&json!({"connector": "db2", "url": "https://x"}));
        assert_eq!(out.prepared, 1);
        assert!(out.bound.is_none());
        let rejected: Vec<_> = out
            .issues
            .iter()
            .filter(|i| i.severity == Severity::Rejected)
            .collect();
        assert_eq!(rejected.len(), 1, "{:?}", out.issues);
        assert_eq!(rejected[0].code, IssueCode::InputParse);
        assert_eq!(
            rejected[0].path.to_string(),
            "tasks[0].function.input.connector"
        );
        assert_eq!(rejected[0].field.as_deref(), Some("connector"));
        assert_eq!(rejected[0].step_kind, Some(StepKind::Task));
    }

    #[test]
    fn prepare_is_not_called_when_the_config_fails() {
        let out = compile(&json!({"connector": "db1"}));
        assert_eq!(out.prepared, 0);
        assert!(out.bound.is_none());
        assert_eq!(out.issues.len(), 1, "{:?}", out.issues);
        assert_eq!(out.issues[0].code, IssueCode::InputParse);
        assert_eq!(
            out.issues[0].path.to_string(),
            "tasks[0].function.input.url"
        );
    }

    /// A hand-written config that breaks its contract the other way: it
    /// records a Rejected issue and still returns a config.
    struct Loud;

    impl FunctionConfig for Loud {
        fn compile(_input: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
            cx.issue(IssueCode::InputParse, "refused");
            Some(Self)
        }
        fn visit(&self, _v: &mut dyn ExprVisitor) {}
        fn fields() -> &'static [FieldSpec] {
            &[]
        }
    }

    /// Counts its `prepare` calls.
    #[derive(Default)]
    struct Counted {
        prepared: AtomicUsize,
    }

    impl Function for Counted {
        type Config = Loud;
        fn prepare(
            &self,
            _cfg: &mut Loud,
            _p: &Prepare<'_>,
        ) -> std::result::Result<(), Vec<Issue>> {
            self.prepared.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &Loud) -> Result<Outcome> {
            Ok(Outcome::Ok)
        }
    }

    #[test]
    fn a_config_that_records_a_rejection_is_neither_prepared_nor_bound() {
        let erased = Erased::new(Counted::default());
        let engine = ExprEngine::default();
        let mut issues = Vec::new();
        let bound = {
            let mut cx = CompileCx::new(&engine, &mut issues)
                .at_path("tasks[0].function.input".parse().unwrap())
                .for_step(StepKind::Task, "t");
            let p = Prepare::new("counted", "w", "t", erased.spec());
            erased.compile(&json!({}), &mut cx, &p)
        };
        assert!(bound.is_none());
        assert_eq!(erased.f.prepared.load(Ordering::Relaxed), 0);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].code, IssueCode::InputParse);
    }

    #[test]
    fn the_fields_are_the_runtime_fields_when_the_spec_declares_them() {
        struct Plugin;
        impl Function for Plugin {
            type Config = FetchConfig;
            fn spec(&self) -> FunctionSpec {
                FunctionSpec::default().with_runtime_fields(vec![FieldSpec::runtime(
                    "only",
                    FieldKind::Interpolated,
                    true,
                )])
            }
            async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &FetchConfig) -> Result<Outcome> {
                Ok(Outcome::Ok)
            }
        }
        let names = |fields: &[FieldSpec]| -> Vec<String> {
            fields.iter().map(|f| f.name.to_string()).collect()
        };
        assert_eq!(names(Erased::new(Plugin).fields()), ["only"]);
        assert_eq!(
            names(Erased::new(Fetch::default()).fields()),
            ["connector", "url", "output"]
        );
    }
}
