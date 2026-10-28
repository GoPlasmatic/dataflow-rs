//! `FunctionConfig`: a function's input, compiled once at build.

use crate::base::{IssueCode, Result};
use crate::expr::{CompileCx, ExprVisitor, FieldSpec, Output, Resolver};
use datavalue::OwnedDataValue;
use serde_json::Value;

/// A function's `input`, compiled at build so a malformed one fails before any
/// message runs. `#[derive(FunctionConfig)]` writes the implementation.
///
/// ```
/// use dataflow_rs::Expr;
/// use dataflow_rs::expr::{CompileCx, ExprEngine, Output};
/// use dataflow_rs::function::FunctionConfig;
/// use dataflow_rs::program::IssueCode;
/// use serde_json::json;
///
/// #[derive(FunctionConfig)]
/// struct Fetch {
///     /// The URL to fetch.
///     #[config(records)]
///     url: Expr<String>,
///     #[config(default = 5000)]
///     timeout_ms: Expr<u64>,
///     #[output(default = "data.response")]
///     output: Output,
/// }
///
/// let engine = ExprEngine::default();
/// let mut issues = Vec::new();
/// let mut cx = CompileCx::new(&engine, &mut issues);
/// let fetch = Fetch::compile(&json!({"url": {"var": "data.url"}}), &mut cx).unwrap();
/// assert_eq!(fetch.timeout_ms.constant(), Some(&5000));
/// assert_eq!(fetch.output.constant().unwrap().to_string(), "data.response");
/// assert!(fetch.url.constant().is_none());
///
/// assert!(Fetch::compile(&json!({"uri": "x"}), &mut cx).is_none());
/// drop(cx);
/// let codes: Vec<IssueCode> = issues.iter().map(|i| i.code).collect();
/// assert_eq!(codes, [IssueCode::LikelyTypo, IssueCode::InputParse]);
/// assert_eq!(issues[1].path.to_string(), "url");
/// ```
///
/// Two outputs need one of them marked `#[output(primary)]`:
///
/// ```compile_fail
/// use dataflow_rs::expr::Output;
/// use dataflow_rs::function::FunctionConfig;
///
/// #[derive(FunctionConfig)]
/// struct Split {
///     ok: Output,
///     failed: Output,
/// }
/// ```
///
/// The primary output, marked or the only one, is an `Output` or an
/// `Option<Output>`, since `primary_output` lends it:
///
/// ```compile_fail
/// use dataflow_rs::expr::Output;
/// use dataflow_rs::function::FunctionConfig;
///
/// #[derive(FunctionConfig)]
/// struct Fan {
///     #[output(primary)]
///     many: Vec<Output>,
///     failed: Output,
/// }
/// ```
///
/// A field merely *named* `Output` is not one: the derive asserts the type's
/// `ConfigField::IS_OUTPUT`, so an implementor that leaves it `false` is
/// refused instead of silently compiling as a destination:
///
/// ```compile_fail
/// use dataflow_rs::expr::{CompileCx, ConfigField, FieldKind, Resolver};
/// use dataflow_rs::function::FunctionConfig;
///
/// struct Output;
///
/// impl ConfigField for Output {
///     fn compile(_raw: &serde_json::Value, _cx: &mut CompileCx<'_>) -> Option<Self> {
///         Some(Self)
///     }
///     fn resolve(
///         &self,
///         _r: &mut dyn Resolver,
///     ) -> dataflow_rs::Result<Option<dataflow_rs::datavalue::OwnedDataValue>> {
///         Ok(None)
///     }
///     fn kind() -> FieldKind {
///         FieldKind::Output
///     }
/// }
///
/// #[derive(FunctionConfig)]
/// struct Writes {
///     out: Output,
/// }
/// ```
///
/// The derive refuses what it cannot honour. An unknown `config` key:
///
/// ```compile_fail
/// # use dataflow_rs::function::FunctionConfig;
/// #[derive(FunctionConfig)]
/// struct Unknown {
///     #[config(renamed = "k")]
///     a: bool,
/// }
/// ```
///
/// A generic struct, even one whose parameter is a config field:
///
/// ```compile_fail
/// # use dataflow_rs::expr::ConfigField;
/// # use dataflow_rs::function::FunctionConfig;
/// #[derive(FunctionConfig)]
/// struct Generic<T: ConfigField> {
///     a: T,
/// }
/// ```
///
/// A tuple struct:
///
/// ```compile_fail
/// # use dataflow_rs::function::FunctionConfig;
/// #[derive(FunctionConfig)]
/// struct Tuple(bool);
/// ```
///
/// `records` on a `serde` field, which holds no expression to record:
///
/// ```compile_fail
/// # use dataflow_rs::function::FunctionConfig;
/// #[derive(FunctionConfig)]
/// struct Recorded {
///     #[config(serde, records)]
///     a: Option<bool>,
/// }
/// ```
///
/// A `serde` field whose type is not `Serialize`, which
/// [`resolve`](Self::resolve) reads it through:
///
/// ```compile_fail
/// # use dataflow_rs::function::FunctionConfig;
/// #[derive(serde::Deserialize)]
/// enum Mode {
///     Fast,
/// }
///
/// #[derive(FunctionConfig)]
/// struct Unserialized {
///     #[config(serde)]
///     mode: Mode,
/// }
/// ```
///
/// A key given twice on one field:
///
/// ```compile_fail
/// # use dataflow_rs::function::FunctionConfig;
/// #[derive(FunctionConfig)]
/// struct Renamed {
///     #[config(rename = "a", rename = "b")]
///     x: bool,
/// }
/// ```
///
/// A `default_json` that is not JSON, refused where it is written rather than
/// when a definition is first compiled:
///
/// ```compile_fail
/// # use dataflow_rs::function::FunctionConfig;
/// #[derive(FunctionConfig)]
/// struct BadDefault {
///     #[config(default_json = "{not json")]
///     extra: serde_json::Value,
/// }
/// ```
///
/// A hand-written config that does not say how it resolves:
///
/// ```compile_fail
/// use dataflow_rs::expr::{CompileCx, ExprVisitor, FieldSpec};
/// use dataflow_rs::function::FunctionConfig;
///
/// struct Bare;
///
/// impl FunctionConfig for Bare {
///     fn compile(_input: &serde_json::Value, _cx: &mut CompileCx<'_>) -> Option<Self> {
///         Some(Self)
///     }
///     fn visit(&self, _v: &mut dyn ExprVisitor) {}
///     fn fields() -> &'static [FieldSpec] {
///         &[]
///     }
/// }
/// ```
pub trait FunctionConfig: Sized + Send + Sync + 'static {
    /// Whether this is [`Templated`](super::Templated), whose fields are
    /// declared by the call's input rather than by a struct: the registry
    /// checks its spec against that, and the erasure walks its primary
    /// output by its own rule.
    const TEMPLATED: bool = false;

    /// Every issue in the config, each at its exact path below the
    /// [`CompileCx`]'s root. `None` when any is Rejected.
    fn compile(input: &Value, cx: &mut CompileCx<'_>) -> Option<Self>;

    /// Walks the config's expressions; paths are relative to the config's root.
    fn visit(&self, v: &mut dyn ExprVisitor);

    /// The config's fields, for tooling.
    fn fields() -> &'static [FieldSpec];

    /// The output [`Ctx::emit_default`](crate::Ctx::emit_default) writes
    /// through, and an answering layer answers with: the derive returns the
    /// field marked `#[output(primary)]`, or the only output when there is
    /// one (`None` for an `Option<Output>` the input did not set). A
    /// hand-written config has none unless it says so.
    ///
    /// It must lend the config's own field, the one [`resolve`](Self::resolve)
    /// resolves: [`Ctx::input_with_primary_output`](crate::Ctx::input_with_primary_output)
    /// and [`Ctx::input_without_primary_output`](crate::Ctx::input_without_primary_output)
    /// find that field by this reference, so a copy made here would leave the
    /// output to resolve as usual.
    fn primary_output(&self) -> Option<&Output> {
        None
    }

    /// The config resolved for one call, through `r`: every field with a
    /// value, set by the input or by its default, keyed by its authored key,
    /// in declaration order; an `Option` left unset is absent. An expression
    /// is evaluated, a document rendered, an output's path given as its
    /// dotted text, a nested config resolved as an object, and a
    /// `#[config(serde)]` field written as it serializes (unset when that is
    /// `null`, as an `Option` left `None` is). The derive generates it; a
    /// hand-written config writes its own, as it writes
    /// [`compile`](Self::compile) and [`visit`](Self::visit), so one that
    /// forgets it fails to compile rather than resolving to nothing.
    ///
    /// [`Ctx::input`](crate::Ctx::input) is this over the call's context,
    /// with every field that reads a secret `null`.
    ///
    /// # Errors
    ///
    /// The first field's that fails: an evaluation's, or a serde field that
    /// does not serialize.
    fn resolve(&self, r: &mut dyn Resolver) -> Result<OwnedDataValue>;
}

/// The config of a function that takes no input: `{}` or `null`.
impl FunctionConfig for () {
    fn compile(input: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
        let before = cx.rejected();
        match input {
            Value::Null => {}
            Value::Object(map) => super::__private::unknown_keys(map, &[], cx),
            _ => cx.issue(
                IssueCode::InputParse,
                "expected an object or null, this function takes no input",
            ),
        }
        (cx.rejected() == before).then_some(())
    }

    fn visit(&self, _v: &mut dyn ExprVisitor) {}

    fn fields() -> &'static [FieldSpec] {
        &[]
    }

    /// No input, so nothing to resolve: `null`.
    fn resolve(&self, _r: &mut dyn Resolver) -> Result<OwnedDataValue> {
        Ok(OwnedDataValue::Null)
    }
}
