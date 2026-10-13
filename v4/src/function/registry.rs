//! `Registry`: the functions and layers a program compiles against.

use super::builtins::BuiltinKind;
use super::erased::{Erased, ErasedFunction};
use super::{CallLayer, Function, FunctionSpec};
use crate::expr::{ExprEngine, FieldSpec};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

/// Functions and layers, plus the [`ExprEngine`] they compile against.
///
/// `Arc` inside: a clone is cheap and shares everything. Registering on a
/// clone copies the table once and leaves the original as it was, which is
/// how a host reroutes a name for some programs and not others.
///
/// ```
/// # extern crate dataflow_rs_v4 as dataflow_rs;
/// use dataflow_rs::{Ctx, Function, Outcome, Registry, Result};
///
/// struct Ping;
///
/// impl Function for Ping {
///     type Config = ();
///     async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
///         Ok(Outcome::Ok)
///     }
/// }
///
/// let mut registry = Registry::default();
/// registry.register("ping", Ping);
/// let names: Vec<&str> = registry.catalogue().map(|f| f.name).collect();
/// assert_eq!(names[..2], ["map", "validation"]);
/// assert_eq!(names.last(), Some(&"ping"));
/// ```
#[derive(Clone)]
pub struct Registry {
    inner: Arc<Inner>,
}

#[derive(Clone)]
struct Inner {
    exprs: ExprEngine,
    /// Each built-in's spec, in [`BuiltinKind::ALL`] order.
    builtin_specs: [FunctionSpec; 8],
    hosts: BTreeMap<Box<str>, Arc<dyn ErasedFunction>>,
    /// The first registered is the outermost.
    layers: Vec<Arc<dyn CallLayer>>,
}

/// What a function name resolves to.
pub(crate) enum Entry<'r> {
    /// One of the eight built-ins, which run inline.
    Builtin(BuiltinKind),
    /// A registered host function.
    Host(&'r dyn ErasedFunction),
}

impl Registry {
    /// A registry holding the eight built-ins, compiling against `exprs`.
    pub fn new(exprs: ExprEngine) -> Self {
        Self {
            inner: Arc::new(Inner {
                exprs,
                builtin_specs: BuiltinKind::ALL.map(BuiltinKind::spec),
                hosts: BTreeMap::new(),
                layers: Vec::new(),
            }),
        }
    }

    /// Registers `f` under `name`, asking it for its [`spec`](Function::spec)
    /// once. A name registered before is replaced.
    ///
    /// # Panics
    ///
    /// On a reserved name: one in
    /// [`BUILTIN_FUNCTIONS`](crate::def::BUILTIN_FUNCTIONS), since a built-in
    /// cannot be shadowed, or `validate`, v3's alias for `validation`, which
    /// the compat reader rewrites before any name is looked up. A host that
    /// registers names from data checks those first.
    pub fn register<F: Function>(&mut self, name: &str, f: F) {
        assert!(
            !BuiltinKind::reserved(name),
            "cannot register `{name}`: the name is reserved for a built-in function \
             (see def::BUILTIN_FUNCTIONS; `validate` is v3's alias for `validation`)"
        );
        let erased: Arc<dyn ErasedFunction> = Arc::new(Erased::new(f));
        Arc::make_mut(&mut self.inner)
            .hosts
            .insert(Box::from(name), erased);
    }

    /// Adds a layer around every host call. The first registered is the
    /// outermost.
    pub fn layer<L: CallLayer>(&mut self, layer: L) {
        Arc::make_mut(&mut self.inner).layers.push(Arc::new(layer));
    }

    /// The engine expressions compile against.
    pub fn exprs(&self) -> &ExprEngine {
        &self.inner.exprs
    }

    /// Every function: the eight built-ins in
    /// [`BUILTIN_FUNCTIONS`](crate::def::BUILTIN_FUNCTIONS) order,
    /// then the host functions by name.
    pub fn catalogue(&self) -> impl Iterator<Item = FunctionInfo<'_>> {
        let builtins = BuiltinKind::ALL
            .into_iter()
            .zip(&self.inner.builtin_specs)
            .map(|(kind, spec)| FunctionInfo {
                name: kind.name(),
                builtin: true,
                spec,
                fields: kind.fields(),
            });
        let hosts = self.inner.hosts.iter().map(|(name, f)| FunctionInfo {
            name,
            builtin: false,
            spec: f.spec(),
            fields: f.fields(),
        });
        builtins.chain(hosts)
    }

    /// What `name` calls: a built-in, a host function, or nothing. v3's
    /// alias `validate` is nothing here; the compat reader rewrites it first.
    pub(crate) fn entry(&self, name: &str) -> Option<Entry<'_>> {
        match BuiltinKind::from_name(name) {
            Some(kind) => Some(Entry::Builtin(kind)),
            None => self.inner.hosts.get(name).map(|f| Entry::Host(&**f)),
        }
    }

    /// The registry's layers, outermost first.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used by the runtime, which starts each host call")
    )]
    pub(crate) fn layers(&self) -> &[Arc<dyn CallLayer>] {
        &self.inner.layers
    }
}

impl Default for Registry {
    /// The eight built-ins, compiling against [`ExprEngine::default`].
    fn default() -> Self {
        Self::new(ExprEngine::default())
    }
}

impl fmt::Debug for Registry {
    /// The function names, in catalogue order.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.catalogue().map(|info| info.name).collect();
        f.debug_struct("Registry")
            .field("functions", &names)
            .finish_non_exhaustive()
    }
}

/// One row of the [`Registry::catalogue`]: a function, as tooling sees it.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct FunctionInfo<'a> {
    /// The name a task calls it by.
    pub name: &'a str,
    /// Whether it is one of the eight built-ins.
    pub builtin: bool,
    /// What it says about itself.
    pub spec: &'a FunctionSpec,
    /// Its config's fields.
    pub fields: &'a [FieldSpec],
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::Result;
    use crate::def::BUILTIN_FUNCTIONS;
    use crate::function::{BoxFuture, Ctx, Next, Outcome};

    struct Ping;

    impl Function for Ping {
        type Config = ();
        fn spec(&self) -> FunctionSpec {
            FunctionSpec::default().with_description("ping")
        }
        async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
            Ok(Outcome::Ok)
        }
    }

    struct Layer;

    impl CallLayer for Layer {
        fn call<'a>(
            &'a self,
            ctx: &'a mut Ctx<'_>,
            next: Next<'a>,
        ) -> BoxFuture<'a, Result<Outcome>> {
            next.run(ctx)
        }
    }

    #[test]
    fn a_builtin_name_resolves_to_its_kind() {
        let registry = Registry::default();
        for name in BUILTIN_FUNCTIONS {
            match registry.entry(name) {
                Some(Entry::Builtin(kind)) => assert_eq!(kind.name(), name),
                _ => panic!("`{name}` is a built-in"),
            }
        }
        // v3's alias is the compat reader's to rewrite.
        assert!(registry.entry("validate").is_none());
        assert!(registry.entry("ping").is_none());
    }

    #[test]
    fn a_host_name_resolves_to_its_function() {
        let mut registry = Registry::default();
        registry.register("ping", Ping);
        match registry.entry("ping") {
            Some(Entry::Host(f)) => assert_eq!(f.spec().description, "ping"),
            _ => panic!("`ping` is registered"),
        }
    }

    #[test]
    fn the_builtin_specs_are_computed_in_catalogue_order() {
        let registry = Registry::default();
        for (kind, info) in BuiltinKind::ALL.into_iter().zip(registry.catalogue()) {
            assert_eq!(info.name, kind.name());
            assert_eq!(info.spec.description, kind.spec().description);
            assert_eq!(info.spec.category, kind.spec().category);
        }
    }

    #[test]
    fn a_clone_shares_the_table_until_it_is_written() {
        let mut registry = Registry::default();
        registry.layer(Layer);
        let mut clone = registry.clone();
        assert!(Arc::ptr_eq(&clone.inner, &registry.inner));
        clone.layer(Layer);
        assert!(!Arc::ptr_eq(&clone.inner, &registry.inner));
        assert_eq!(registry.layers().len(), 1);
        assert_eq!(clone.layers().len(), 2);
    }
}
