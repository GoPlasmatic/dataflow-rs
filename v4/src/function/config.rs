//! `FunctionConfig`: a function's input, compiled once at build.

use crate::base::IssueCode;
use crate::expr::{CompileCx, ExprVisitor, FieldSpec, Output};
use serde_json::Value;

/// A function's `input`, compiled at build so a malformed one fails before any
/// message runs. `#[derive(FunctionConfig)]` writes the implementation.
///
/// ```
/// # extern crate dataflow_rs_v4 as dataflow_rs;
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
/// # extern crate dataflow_rs_v4 as dataflow_rs;
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
/// # extern crate dataflow_rs_v4 as dataflow_rs;
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
/// The derive refuses what it cannot honour. An unknown `config` key:
///
/// ```compile_fail
/// # extern crate dataflow_rs_v4 as dataflow_rs;
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
/// # extern crate dataflow_rs_v4 as dataflow_rs;
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
/// # extern crate dataflow_rs_v4 as dataflow_rs;
/// # use dataflow_rs::function::FunctionConfig;
/// #[derive(FunctionConfig)]
/// struct Tuple(bool);
/// ```
///
/// `records` on a `serde` field, which holds no expression to record:
///
/// ```compile_fail
/// # extern crate dataflow_rs_v4 as dataflow_rs;
/// # use dataflow_rs::function::FunctionConfig;
/// #[derive(FunctionConfig)]
/// struct Recorded {
///     #[config(serde, records)]
///     a: Option<bool>,
/// }
/// ```
///
/// A key given twice on one field:
///
/// ```compile_fail
/// # extern crate dataflow_rs_v4 as dataflow_rs;
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
/// # extern crate dataflow_rs_v4 as dataflow_rs;
/// # use dataflow_rs::function::FunctionConfig;
/// #[derive(FunctionConfig)]
/// struct BadDefault {
///     #[config(default_json = "{not json")]
///     extra: serde_json::Value,
/// }
/// ```
pub trait FunctionConfig: Sized + Send + Sync + 'static {
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
    fn primary_output(&self) -> Option<&Output> {
        None
    }
}

/// The config of a function that takes no input: `{}` or `null`.
impl FunctionConfig for () {
    fn compile(input: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
        let before = cx.rejected();
        match input {
            Value::Null => {}
            Value::Object(map) => {
                for key in map.keys() {
                    cx.at(key.as_str(), |cx| cx.unknown_key(key, &[]));
                }
            }
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
}
