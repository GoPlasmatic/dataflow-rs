//! # PathTemplate
//!
//! A config field naming a *write destination* — where a value lands in the
//! message context — expressed as JSONLogic.
//!
//! Split from [`Template`] because a destination needs a different resolved
//! form than a value does. Every write in this crate goes through the
//! `*_parts` tree walkers, which take a pre-split `&[Arc<str>]`; the dotted
//! string is carried alongside because [`Change::path`](crate::engine::message::Change)
//! records it on the audit trail. Splitting per write cost measurable
//! throughput on the `map` hot path, which is why the split has always been
//! precomputed at engine construction.
//!
//! That precompute is preserved exactly: a destination authored the static way
//! — `"data.output"` — folds to a constant, and its `(dotted, parts)` pair is
//! computed once at compile time and handed back as two refcount bumps. Only a
//! destination that actually reads the message pays to split per write.

use crate::engine::error::Result;
use crate::engine::functions::template::{Template, TemplateCompiler};
use crate::engine::task_context::TaskContext;
use crate::engine::utils::compute_data_path;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::borrow::Cow;
use std::marker::PhantomData;
use std::sync::Arc;

/// What a built-in needs to resolve its parameters from inside an arena scope.
///
/// The sync built-ins (`map`, `parse_*`, `publish_*`, `validation`) run against
/// an [`ArenaContext`](crate::engine::executor::ArenaContext) that earlier tasks
/// in the same stretch already populated, and hold no
/// [`TaskContext`](crate::TaskContext). Bundling the three pieces keeps their
/// signatures to one extra argument, and keeps every parameter resolving
/// against the *same* context view the task's own logic sees — including
/// mutations an earlier mapping in the same task made.
#[derive(Clone, Copy)]
pub struct ParamCtx<'a> {
    engine: &'a datalogic_rs::Engine,
    context: datavalue::DataValue<'a>,
    arena: &'a datalogic_rs::bumpalo::Bump,
}

impl<'a> ParamCtx<'a> {
    pub(crate) fn new(
        engine: &'a datalogic_rs::Engine,
        context: datavalue::DataValue<'a>,
        arena: &'a datalogic_rs::bumpalo::Bump,
    ) -> Self {
        Self {
            engine,
            context,
            arena,
        }
    }

    /// Build one from an arena context. `DataValue` and `&Bump` both carry the
    /// arena's `'a`, not a borrow of `arena_ctx`, so the caller may keep
    /// mutating the context afterwards.
    pub(crate) fn from_arena(
        engine: &'a datalogic_rs::Engine,
        arena_ctx: &crate::engine::executor::ArenaContext<'a>,
    ) -> Self {
        Self::new(engine, arena_ctx.as_data_value(), arena_ctx.arena())
    }

    pub(crate) fn engine(&self) -> &'a datalogic_rs::Engine {
        self.engine
    }

    pub(crate) fn context(&self) -> &datavalue::DataValue<'a> {
        &self.context
    }

    pub(crate) fn arena(&self) -> &'a datalogic_rs::bumpalo::Bump {
        self.arena
    }
}

/// A resolved write destination: the dotted path, and its pre-split parts.
///
/// Both halves are needed at every write. The parts drive the `*_parts` tree
/// walkers; the dotted form is what [`Change::path`](crate::Change) records on
/// the audit trail. They travel together so the two can never disagree about
/// where a value went.
pub type ResolvedPath = (Arc<str>, Arc<[Arc<str>]>);

/// Where a resolved path string is rooted.
///
/// A marker rather than a runtime field so the rooting is fixed by the type of
/// the config field that holds it, and cannot be lost when a config is built
/// directly instead of deserialized.
pub trait PathRoot: Default + Clone + Copy + std::fmt::Debug {
    /// Compute the `(dotted, parts)` pair for an already-resolved path string.
    ///
    /// Parts keep any `#` prefix: it is the explicit "object key, not array
    /// index" hint that `set_nested_value` consumes when deciding container
    /// shape, and `strip_hash_prefix` is applied at lookup time inside the
    /// `*_parts` helpers.
    fn compute(path: &str) -> ResolvedPath;
}

/// Rooted at the whole message context: the authored path names its own root,
/// as in `data.user.name`, `metadata.progress` or `temp_data.i`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContextRoot;

impl PathRoot for ContextRoot {
    fn compute(path: &str) -> ResolvedPath {
        (
            Arc::from(path),
            path.split('.').map(Arc::from).collect::<Vec<_>>().into(),
        )
    }
}

/// Rooted inside `data`: the authored `"orders"` means `data.orders`. What
/// `parse_json`, `parse_xml`, `publish_json` and `publish_xml` call `target`.
#[derive(Debug, Clone, Copy, Default)]
pub struct DataRoot;

impl PathRoot for DataRoot {
    fn compute(path: &str) -> ResolvedPath {
        // `compute_data_path` is the same helper `precompute_target_path` has
        // always used, so a constant target resolves byte-identically to the
        // pre-3.9 static field.
        compute_data_path(path)
    }
}

/// A config field naming a write destination, expressed as JSONLogic.
///
/// `R` fixes the rooting: [`ContextRoot`] for a path that names its own root,
/// [`DataRoot`] for one relative to `data`.
///
/// The static spelling is a JSON string and keeps its precomputed fast path:
///
/// ```json
/// { "path": "data.total" }
/// ```
///
/// A dynamic destination is any expression resolving to a path string:
///
/// ```json
/// { "path": {"cat": ["data.accounts.", {"var": "data.account_id"}, ".balance"]} }
/// ```
///
/// The expression resolves to the *name* of a location, never to the value at
/// one. It is evaluated against the message context like any other parameter.
#[derive(Debug, Clone)]
pub struct PathTemplate<R: PathRoot = ContextRoot> {
    template: Template,
    /// `Some` when the template folded to a compile-time constant — the
    /// precomputed pair every resolve hands back as two refcount bumps.
    precomputed: Option<ResolvedPath>,
    root: PhantomData<R>,
}

impl<'de, R: PathRoot> Deserialize<'de> for PathTemplate<R> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Ok(Self::from_template(Template::deserialize(d)?))
    }
}

impl<R: PathRoot> Default for PathTemplate<R> {
    fn default() -> Self {
        Self::from_template(Template::from(Value::String(String::new())))
    }
}

impl<R: PathRoot> From<Value> for PathTemplate<R> {
    fn from(raw: Value) -> Self {
        Self::from_template(Template::from(raw))
    }
}

impl<R: PathRoot> From<&str> for PathTemplate<R> {
    fn from(path: &str) -> Self {
        Self::from(Value::String(path.to_string()))
    }
}

impl<R: PathRoot> PathTemplate<R> {
    fn from_template(template: Template) -> Self {
        Self {
            template,
            precomputed: None,
            root: PhantomData,
        }
    }

    /// Compile the expression and, when it folded to a constant, precompute its
    /// `(dotted, parts)` pair. Called once at engine construction by
    /// `LogicCompiler`.
    ///
    /// # Errors
    ///
    /// As [`Template::compile`], plus a resolution failure if the constant does
    /// not coerce to a path string.
    pub fn compile(&mut self, c: &TemplateCompiler, label: &str) -> Result<()> {
        self.template.compile(c, label)?;
        // Reading the constant needs no message, so `constant_string` can do
        // the work `resolve` would otherwise defer to the first message.
        self.precomputed = self.template.constant_string().map(|s| R::compute(&s));
        Ok(())
    }

    /// The destination for this message as `(dotted path, split parts)`.
    ///
    /// Constant destinations return the precomputed pair — two `Arc` clones,
    /// no splitting, no allocation. Dynamic ones resolve to a plain string and
    /// split it.
    ///
    /// # Errors
    ///
    /// [`crate::DataflowError::LogicEvaluation`] if the expression fails to
    /// evaluate, or if it was never compiled and is not a literal string.
    pub fn resolve(&self, ctx: &TaskContext<'_>) -> Result<ResolvedPath> {
        if let Some((dotted, parts)) = &self.precomputed {
            return Ok((Arc::clone(dotted), Arc::clone(parts)));
        }
        // A config built directly rather than through `LogicCompiler` — the
        // test surface and a few in-tree helpers — never had its literal
        // compiled. Fall back to reading it straight off the authored JSON:
        // same semantics, one extra allocation per call, which is the contract
        // `MapConfig`, `ParseConfig` and `PublishConfig` each had before 3.9.
        if !self.template.is_compiled() {
            if let Value::String(s) = self.template.as_json() {
                return Ok(R::compute(s));
            }
        }
        Ok(R::compute(&self.template.resolve_string(ctx)?))
    }

    /// As [`Self::resolve`], against a context already resident in `arena`.
    ///
    /// What the built-in sync executors call: they run inside an arena scope
    /// whose context earlier tasks already populated, and hold no
    /// [`TaskContext`]. A constant destination — the overwhelmingly common
    /// case — returns before any of that matters.
    ///
    /// # Errors
    ///
    /// As [`Self::resolve`].
    pub(crate) fn resolve_in_arena(&self, p: ParamCtx<'_>) -> Result<Cow<'_, ResolvedPath>> {
        // Borrowed, not cloned. Two `Arc` clones per write is two atomic
        // increments (and two decrements) on the hottest loop in the engine,
        // for a pair the caller only reads.
        if let Some(pair) = &self.precomputed {
            return Ok(Cow::Borrowed(pair));
        }
        if !self.template.is_compiled() {
            if let Value::String(s) = self.template.as_json() {
                return Ok(Cow::Owned(R::compute(s)));
            }
        }
        Ok(Cow::Owned(R::compute(
            &self.template.resolve_str_in_arena(p)?,
        )))
    }

    /// The authored JSON, unchanged — for authoring checks and for handlers
    /// that re-serialize their own config.
    pub fn as_json(&self) -> &Value {
        self.template.as_json()
    }

    /// Whether the destination folded to a compile-time constant, so
    /// [`Self::resolve`] returns the precomputed pair.
    pub fn is_constant(&self) -> bool {
        self.precomputed.is_some()
    }

    /// The constant destination as a dotted string, when it folded to one.
    ///
    /// For diagnostics that want to name the destination without a message in
    /// hand — error labels, the workflow visualizer, authoring issues. `None`
    /// for a dynamic destination, which has no single answer.
    pub fn constant_path(&self) -> Option<&str> {
        self.precomputed.as_ref().map(|(dotted, _)| &**dotted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::compiler::datalogic_engine_builder;
    use crate::engine::message::Message;
    use crate::engine::utils::compute_path_parts;
    use serde_json::json;

    fn compiler() -> TemplateCompiler {
        TemplateCompiler::new(Arc::new(datalogic_engine_builder().build()))
    }

    fn parts_of(parts: &Arc<[Arc<str>]>) -> Vec<String> {
        parts.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn a_static_context_path_is_precomputed_and_split_as_before() {
        let mut p: PathTemplate<ContextRoot> = PathTemplate::from("data.user.name");
        p.compile(&compiler(), "lbl").unwrap();

        assert!(p.is_constant(), "a literal path must fold to a constant");
        assert_eq!(p.constant_path(), Some("data.user.name"));

        let mut m = Message::from_value(&json!({}));
        let dl = Arc::new(datalogic_engine_builder().build());
        let ctx = TaskContext::new(&mut m, &dl);
        let (dotted, parts) = p.resolve(&ctx).unwrap();
        assert_eq!(&*dotted, "data.user.name");
        assert_eq!(parts_of(&parts), ["data", "user", "name"]);
    }

    #[test]
    fn a_static_data_rooted_target_gains_the_data_prefix() {
        let mut p: PathTemplate<DataRoot> = PathTemplate::from("orders");
        p.compile(&compiler(), "lbl").unwrap();

        // Byte-identical to what `precompute_target_path` produced pre-3.9.
        assert_eq!(p.constant_path(), Some("data.orders"));
        let (dotted, parts) = compute_data_path("orders");
        assert_eq!(p.constant_path(), Some(&*dotted));
        assert_eq!(parts_of(&parts), ["data", "orders"]);
    }

    #[test]
    fn a_dynamic_path_resolves_against_the_message() {
        let dl = Arc::new(datalogic_engine_builder().build());
        let mut p: PathTemplate<ContextRoot> =
            PathTemplate::from(json!({"cat": ["data.accounts.", {"var": "data.id"}, ".balance"]}));
        p.compile(&compiler(), "lbl").unwrap();

        assert!(!p.is_constant());
        assert_eq!(p.constant_path(), None, "a dynamic path names no one place");

        let mut m = Message::from_value(&json!({}));
        crate::engine::utils::set_nested_value(
            &mut m.context,
            "data.id",
            datavalue::OwnedDataValue::String("ACC7".to_string()),
        );
        let ctx = TaskContext::new(&mut m, &dl);
        let (dotted, parts) = p.resolve(&ctx).unwrap();
        assert_eq!(&*dotted, "data.accounts.ACC7.balance");
        assert_eq!(parts_of(&parts), ["data", "accounts", "ACC7", "balance"]);
    }

    #[test]
    fn an_uncompiled_literal_still_resolves_for_directly_built_configs() {
        // The pre-3.9 fallback: a config constructed by hand, never passed
        // through `LogicCompiler`, still writes to the right place.
        let dl = Arc::new(datalogic_engine_builder().build());
        let p: PathTemplate<ContextRoot> = PathTemplate::from("temp_data.x");
        let mut m = Message::from_value(&json!({}));
        let ctx = TaskContext::new(&mut m, &dl);

        let (dotted, parts) = p.resolve(&ctx).unwrap();
        assert_eq!(&*dotted, "temp_data.x");
        assert_eq!(parts_of(&parts), ["temp_data", "x"]);
    }

    #[test]
    fn a_hash_prefixed_segment_survives_the_split() {
        // `#` is the "object key, not array index" hint; parts keep it and
        // `strip_hash_prefix` runs at lookup. Splitting must not eat it.
        let mut p: PathTemplate<ContextRoot> = PathTemplate::from("data.rows.#20.total");
        p.compile(&compiler(), "lbl").unwrap();

        let dl = Arc::new(datalogic_engine_builder().build());
        let mut m = Message::from_value(&json!({}));
        let ctx = TaskContext::new(&mut m, &dl);
        let (_, parts) = p.resolve(&ctx).unwrap();
        assert_eq!(parts_of(&parts), ["data", "rows", "#20", "total"]);
    }

    #[test]
    fn compute_path_parts_is_the_shared_split_for_data_rooting() {
        // Pins that `DataRoot` reuses the existing helper rather than growing
        // a second splitter that could drift from it.
        let (_, parts) = DataRoot::compute("a.b");
        assert_eq!(
            parts_of(&parts),
            parts_of(&compute_path_parts("data", "a.b"))
        );
    }
}
