// The call surface and the registry: what a host sees of the functions it
// registers, and that the types it hands across threads can cross them.

use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::def::BUILTIN_FUNCTIONS;
use dataflow_rs::expr::{ExprEngine, Output, Secrets};
use dataflow_rs::function::{
    BoxFuture, CallInfo, CallLayer, FieldKind, FieldSpec, FunctionConfig, FunctionSpec, Next,
    Reference, RetrySafety, Templated, ValueKind,
};
use dataflow_rs::{Ctx, Expr, Function, Outcome, Registry, Result};
use serde_json::json;
use std::marker::PhantomData;

#[derive(FunctionConfig)]
struct FetchConfig {
    url: Expr<String>,
    #[output(default = "data.response")]
    output: Output,
}

/// A derived config whose declared default its field's kind refuses: the
/// author's `INPUT_PARSE` would otherwise wait for every compile that
/// leaves `limit` out.
#[derive(FunctionConfig)]
struct BadDefault {
    #[config(default = "abc")]
    limit: Expr<u64>,
}

/// A host function whose `call` returns a future without `async`.
struct Fetch;

impl Function for Fetch {
    type Config = FetchConfig;

    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default()
            .with_category("io")
            .with_description("Fetches a URL.")
            .with_retry(RetrySafety::Idempotent)
    }

    fn call(
        &self,
        _ctx: &mut Ctx<'_>,
        _cfg: &FetchConfig,
    ) -> impl Future<Output = Result<Outcome>> + Send {
        std::future::ready(Ok(Outcome::Ok))
    }
}

/// A host function written with `async fn`, which holds the context and the
/// config across an await: the trait's `Send` bound needs both to cross
/// threads.
struct Echo;

impl Function for Echo {
    type Config = FetchConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &FetchConfig) -> Result<Outcome> {
        let task = ctx.info().task_id;
        std::future::ready(()).await;
        Ok(match cfg.url.constant() {
            Some(_) if !task.is_empty() => Outcome::Ok,
            _ => Outcome::Status(202),
        })
    }
}

/// A host function that says only its description.
struct Described(&'static str);

impl Function for Described {
    type Config = ();

    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default().with_description(self.0)
    }

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        Ok(Outcome::Skip)
    }
}

/// A plugin whose fields its manifest declares at runtime.
struct Plugin;

impl Function for Plugin {
    type Config = Templated;

    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default().with_runtime_fields(vec![
            FieldSpec::runtime("record", FieldKind::Interpolated, true),
            FieldSpec::runtime("out", FieldKind::Output, false).with_default("\"data.out\""),
        ])
    }

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &Templated) -> Result<Outcome> {
        Ok(Outcome::Halt)
    }
}

fn names(registry: &Registry) -> Vec<&str> {
    registry.catalogue().map(|row| row.name).collect()
}

fn with_builtins<'a>(hosts: &[&'a str]) -> Vec<&'a str> {
    BUILTIN_FUNCTIONS.iter().chain(hosts).copied().collect()
}

fn field_names(fields: &[FieldSpec]) -> Vec<&str> {
    fields.iter().map(|f| &*f.name).collect()
}

#[test]
fn the_default_catalogue_is_the_eight_builtins_in_order() {
    let registry = Registry::default();
    let rows: Vec<_> = registry.catalogue().collect();
    assert_eq!(rows.len(), 8);
    assert_eq!(names(&registry), BUILTIN_FUNCTIONS);
    assert!(rows.iter().all(|row| row.builtin));
    assert!(rows.iter().all(|row| !row.spec.description.is_empty()));
    assert!(rows.iter().all(|row| !row.spec.category.is_empty()));
    assert_eq!(field_names(rows[0].fields), ["mappings"]);
    assert_eq!(rows[0].spec.category, "transform");
}

#[test]
fn a_registered_host_function_lists_after_the_builtins() {
    let mut registry = Registry::default();
    registry.register("fetch", Fetch);
    assert_eq!(names(&registry), with_builtins(&["fetch"]));

    let fetch = registry.catalogue().last().unwrap();
    assert!(!fetch.builtin);
    assert_eq!(fetch.spec.category, "io");
    assert_eq!(fetch.spec.description, "Fetches a URL.");
    assert_eq!(fetch.spec.retry, RetrySafety::Idempotent);
    assert_eq!(field_names(fetch.fields), ["url", "output"]);
    assert!(matches!(
        fetch.fields[0].kind,
        FieldKind::Expr(ValueKind::String)
    ));
    assert!(matches!(fetch.fields[1].kind, FieldKind::Output));
    assert_eq!(
        fetch.fields[1].default.as_deref(),
        Some("\"data.response\"")
    );
    assert_eq!(fetch.fields.len(), FetchConfig::fields().len());
}

#[test]
fn host_functions_list_by_name() {
    let mut registry = Registry::default();
    registry.register("fetch", Fetch);
    registry.register("echo", Echo);
    assert_eq!(names(&registry), with_builtins(&["echo", "fetch"]));
    assert!(registry.catalogue().skip(8).all(|row| !row.builtin));
}

#[test]
fn a_templated_function_lists_its_runtime_fields() {
    let mut registry = Registry::default();
    registry.register("plugin", Plugin);
    let plugin = registry.catalogue().last().unwrap();
    assert_eq!(plugin.name, "plugin");
    assert_eq!(field_names(plugin.fields), ["record", "out"]);
    assert!(plugin.fields[0].required);
    assert_eq!(plugin.fields[1].default.as_deref(), Some("\"data.out\""));
    // The fields come from the spec: `Templated` declares none of its own.
    assert!(Templated::fields().is_empty());
}

#[test]
#[should_panic(expected = "`map`")]
fn registering_map_panics_naming_it() {
    Registry::default().register("map", Fetch);
}

#[test]
#[should_panic(expected = "`publish_xml`")]
fn registering_any_builtin_name_panics() {
    Registry::default().register("publish_xml", Described("shadow"));
}

#[test]
#[should_panic(expected = "`validate`")]
fn registering_v3s_alias_validate_panics() {
    Registry::default().register("validate", Described("shadow"));
}

/// A host function with config `C` that says `spec` about itself.
struct Specced<C>(FunctionSpec, PhantomData<fn() -> C>);

impl<C: FunctionConfig> Function for Specced<C> {
    type Config = C;

    fn spec(&self) -> FunctionSpec {
        self.0.clone()
    }

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &C) -> Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

fn specced<C>(spec: FunctionSpec) -> Specced<C> {
    Specced(spec, PhantomData)
}

fn referencing(field: &'static str) -> FunctionSpec {
    FunctionSpec::default().with_references(vec![Reference::new(field, "connector")])
}

fn runtime_fields() -> FunctionSpec {
    FunctionSpec::default().with_runtime_fields(vec![FieldSpec::runtime(
        "connector",
        FieldKind::Literal(ValueKind::String),
        true,
    )])
}

#[test]
#[should_panic(expected = "`unparsed`")]
fn a_reference_field_that_is_not_an_authored_path_panics() {
    Registry::default().register(
        "unparsed",
        specced::<FetchConfig>(referencing("con nector[")),
    );
}

#[test]
#[should_panic(expected = "`dangling`")]
fn a_reference_naming_no_field_of_the_config_panics() {
    Registry::default().register("dangling", specced::<FetchConfig>(referencing("connector")));
}

#[test]
#[should_panic(expected = "`not_templated`")]
fn runtime_fields_on_a_config_that_is_not_templated_panic() {
    Registry::default().register("not_templated", specced::<FetchConfig>(runtime_fields()));
}

#[test]
#[should_panic(expected = "`no_fields`")]
fn a_templated_config_without_runtime_fields_panics() {
    Registry::default().register("no_fields", specced::<Templated>(FunctionSpec::default()));
}

#[test]
fn try_register_refuses_a_reserved_name_without_panicking() {
    let mut registry = Registry::default();
    let error = registry.try_register("map", Fetch).unwrap_err();
    assert!(error.message().contains("`map`"), "{}", error.message());
    assert!(
        registry
            .catalogue()
            .all(|row| row.name != "map" || row.builtin)
    );
}

#[test]
fn try_register_refuses_a_bad_spec_without_panicking() {
    let mut registry = Registry::default();
    let error = registry
        .try_register("not_templated", specced::<FetchConfig>(runtime_fields()))
        .unwrap_err();
    assert!(
        error.message().contains("not `Templated`"),
        "{}",
        error.message()
    );
}

#[test]
fn try_register_registers_what_register_does() {
    let mut registry = Registry::default();
    registry.try_register("fetch", Fetch).unwrap();
    assert!(
        registry
            .catalogue()
            .any(|row| row.name == "fetch" && !row.builtin)
    );
}

#[test]
fn duplicate_runtime_fields_are_refused_without_replacing_a_registered_function() {
    let mut registry = Registry::default();
    registry.register("plugin", Plugin);
    let spec = FunctionSpec::default().with_runtime_fields(vec![
        FieldSpec::runtime("value", FieldKind::Expr(ValueKind::Any), false),
        FieldSpec::runtime("value", FieldKind::Literal(ValueKind::String), false),
    ]);
    let error = registry
        .try_register("plugin", specced::<Templated>(spec))
        .unwrap_err();
    assert_eq!(
        error.message(),
        "cannot register `plugin`: its field `value` is declared twice"
    );
    let plugin = registry
        .catalogue()
        .find(|row| row.name == "plugin")
        .unwrap();
    assert_eq!(field_names(plugin.fields), ["record", "out"]);
}

#[test]
fn runtime_field_kinds_that_templated_cannot_compile_are_refused_at_registration() {
    for kind in [
        FieldKind::Templated,
        FieldKind::Nested(|| &[]),
        FieldKind::List(|| FieldKind::Literal(ValueKind::Any)),
        FieldKind::Map(|| FieldKind::Literal(ValueKind::Any)),
    ] {
        let spec = FunctionSpec::default()
            .with_runtime_fields(vec![FieldSpec::runtime("value", kind, false)]);
        let mut registry = Registry::default();
        let error = registry
            .try_register("plugin", specced::<Templated>(spec))
            .unwrap_err();
        assert!(
            error
                .message()
                .contains("runtime field `value` has an unsupported kind")
        );
        assert!(registry.catalogue().all(|row| row.name != "plugin"));
    }
}

/// Runtime fields holding the outputs `names`, those in `primary` marked
/// primary.
fn outputs(names: &[&'static str], primary: &[&str]) -> FunctionSpec {
    let fields = names
        .iter()
        .map(|&name| {
            let mut field = FieldSpec::runtime(name, FieldKind::Output, false);
            field.primary = primary.contains(&name);
            field
        })
        .collect();
    FunctionSpec::default().with_runtime_fields(fields)
}

#[test]
#[should_panic(
    expected = "cannot register `unmarked`: its runtime fields hold 2 outputs, 0 of \
                           them primary: mark exactly one of them `primary`"
)]
fn several_runtime_outputs_none_primary_panic() {
    Registry::default().register("unmarked", specced::<Templated>(outputs(&["a", "b"], &[])));
}

#[test]
#[should_panic(
    expected = "cannot register `twice`: its runtime fields hold 3 outputs, 2 of \
                           them primary: mark exactly one of them `primary`"
)]
fn several_runtime_outputs_more_than_one_primary_panic() {
    let spec = outputs(&["a", "b", "c"], &["a", "c"]);
    Registry::default().register("twice", specced::<Templated>(spec));
}

#[test]
fn runtime_outputs_register_with_one_primary_or_alone() {
    // The derive's rule: several outputs mark exactly one, and a lone
    // output is the primary without saying so.
    let mut registry = Registry::default();
    registry.register("marked", specced::<Templated>(outputs(&["a", "b"], &["b"])));
    registry.register("lone", specced::<Templated>(outputs(&["a"], &[])));
    assert_eq!(names(&registry), with_builtins(&["lone", "marked"]));
}

#[test]
#[should_panic(expected = "cannot register `bad_runtime`: its field `limit`'s default is invalid")]
fn a_runtime_field_default_its_kind_refuses_panics() {
    let field = FieldSpec::runtime("limit", FieldKind::Literal(ValueKind::Number), false)
        .with_default("\"five\"");
    let spec = FunctionSpec::default().with_runtime_fields(vec![field]);
    Registry::default().register("bad_runtime", specced::<Templated>(spec));
}

#[test]
#[should_panic(expected = "cannot register `bad_derived`: its field `limit`'s default is invalid")]
fn a_derived_field_default_its_kind_refuses_panics() {
    Registry::default().register(
        "bad_derived",
        specced::<BadDefault>(FunctionSpec::default()),
    );
}

#[test]
fn a_reference_names_a_field_of_the_config_or_of_the_runtime_fields() {
    let mut registry = Registry::default();
    registry.register("fetch", specced::<FetchConfig>(referencing("url")));
    let spec = runtime_fields().with_references(vec![Reference::new("connector", "connector")]);
    registry.register("plugin", specced::<Templated>(spec));
    assert_eq!(names(&registry), with_builtins(&["fetch", "plugin"]));
}

/// A connector's settings, nested in a config.
#[derive(FunctionConfig)]
struct AuthConfig {
    connector: String,
}

/// A field of every kind a reference can sit in or below.
#[derive(FunctionConfig)]
struct DeepConfig {
    auth: AuthConfig,
    auths: Vec<AuthConfig>,
    hosts: Vec<String>,
    pools: std::collections::BTreeMap<String, String>,
    url: Expr<String>,
    connector: String,
    extra: serde_json::Value,
}

/// A host function over [`DeepConfig`] whose reference is `field`.
fn deep(field: &'static str) -> Specced<DeepConfig> {
    specced(referencing(field))
}

#[test]
#[should_panic(
    expected = "cannot register `typo`: its reference field `auth.conector` does not reach a \
                field of its config: `auth` has no field `conector`"
)]
fn a_reference_into_a_nested_config_names_one_of_its_fields() {
    Registry::default().register("typo", deep("auth.conector"));
}

#[test]
#[should_panic(
    expected = "cannot register `indexed`: its reference field `auth[0]` does not reach a field \
                of its config: `auth` is not a list, so it has no `[0]`"
)]
fn a_reference_indexing_what_is_not_a_list_panics() {
    Registry::default().register("indexed", deep("auth[0]"));
}

#[test]
#[should_panic(
    expected = "cannot register `keyed`: its reference field `hosts.primary` does not reach a \
                field of its config: `hosts` is a list, so `primary` is not an index"
)]
fn a_reference_keying_into_a_list_panics() {
    Registry::default().register("keyed", deep("hosts.primary"));
}

#[test]
#[should_panic(
    expected = "cannot register `scalar`: its reference field `connector.name` does not reach a \
                field of its config: `connector` holds no fields, so it has no `name`"
)]
fn a_reference_below_a_scalar_panics() {
    Registry::default().register("scalar", deep("connector.name"));
}

#[test]
#[should_panic(
    expected = "cannot register `expr`: its reference field `url.host` does not reach a field \
                of its config: `url` holds no fields, so it has no `host`"
)]
fn a_reference_below_an_expression_panics() {
    Registry::default().register("expr", deep("url.host"));
}

#[test]
fn a_reference_reaches_a_nested_field_a_list_element_and_a_map_value() {
    // Each segment through the field kinds: a nested config's field, a
    // list's index, a map's key, and anything below a literal of any JSON.
    let mut registry = Registry::default();
    for (name, field) in [
        ("nested", "auth.connector"),
        ("element", "hosts[0]"),
        ("nested_element", "auths[1].connector"),
        ("value", "pools.primary"),
        ("literal", "extra.a[2].b"),
        ("top", "connector"),
    ] {
        registry.register(name, deep(field));
    }
    assert_eq!(
        names(&registry),
        with_builtins(&[
            "element",
            "literal",
            "nested",
            "nested_element",
            "top",
            "value"
        ])
    );
}

#[test]
fn registering_a_name_twice_keeps_the_second() {
    let mut registry = Registry::default();
    registry.register("fetch", Fetch);
    registry.register("fetch", Described("rerouted"));
    assert_eq!(names(&registry), with_builtins(&["fetch"]));
    let fetch = registry.catalogue().last().unwrap();
    assert_eq!(fetch.spec.description, "rerouted");
    assert!(fetch.fields.is_empty());
}

#[test]
fn a_registration_on_a_clone_leaves_the_original_alone() {
    let mut original = Registry::default();
    original.register("fetch", Fetch);
    let mut clone = original.clone();
    clone.register("echo", Echo);
    clone.register("fetch", Described("rerouted"));
    clone.layer(Forward);

    assert_eq!(names(&original), with_builtins(&["fetch"]));
    let fetch = original.catalogue().last().unwrap();
    assert_eq!(fetch.spec.description, "Fetches a URL.");

    assert_eq!(names(&clone), with_builtins(&["echo", "fetch"]));
    let fetch = clone.catalogue().last().unwrap();
    assert_eq!(fetch.spec.description, "rerouted");
}

#[test]
fn the_registry_keeps_the_engine_it_was_given() {
    let secrets = Secrets::new(OwnedDataValue::from(&json!({"api_key": "k"}))).unwrap();
    let engine = ExprEngine::builder().secrets(secrets).build().unwrap();
    let registry = Registry::new(engine);
    let names: Vec<&str> = registry.exprs().secrets().names().collect();
    assert_eq!(names, ["api_key"]);
    assert_eq!(Registry::default().exprs().secrets().names().count(), 0);
}

#[test]
fn debug_lists_the_names_in_catalogue_order() {
    let mut registry = Registry::default();
    registry.register("fetch", Fetch);
    registry.register("echo", Echo);
    let text = format!("{registry:?}");
    let listed = format!("{:?}", with_builtins(&["echo", "fetch"]));
    assert!(text.contains(&listed), "{text}");
}

fn assert_send_sync<T: Send + Sync>() {}
fn assert_send<T: Send>() {}
fn assert_clone<T: Clone>() {}

#[test]
fn the_call_surface_crosses_threads() {
    assert_send_sync::<Registry>();
    assert_send_sync::<Outcome>();
    assert_send_sync::<CallInfo<'static>>();
    assert_send_sync::<Ctx<'static>>();
    assert_send::<Next<'static>>();
    assert_clone::<Registry>();
}

#[test]
fn an_async_fn_function_registers() {
    let mut registry = Registry::default();
    registry.register("echo", Echo);
    let echo = registry.catalogue().last().unwrap();
    assert_eq!(echo.name, "echo");
    assert_eq!(echo.spec.retry, RetrySafety::Unknown);
}

/// A layer that only forwards.
struct Forward;

impl CallLayer for Forward {
    fn call<'a>(&'a self, ctx: &'a mut Ctx<'_>, next: Next<'a>) -> BoxFuture<'a, Result<Outcome>> {
        next.run(ctx)
    }
}

/// A layer that awaits the rest of the chain inside its own future, which
/// the `Send` bound on that future lets only a `Send` `Next` and context do.
struct Awaiting;

impl CallLayer for Awaiting {
    fn call<'a>(&'a self, ctx: &'a mut Ctx<'_>, next: Next<'a>) -> BoxFuture<'a, Result<Outcome>> {
        Box::pin(async move { next.run(ctx).await })
    }
}

#[test]
fn a_forwarding_layer_registers() {
    let mut registry = Registry::default();
    registry.layer(Forward);
    registry.layer(Awaiting);
    // A layer is not a function.
    assert_eq!(names(&registry), BUILTIN_FUNCTIONS);
}
