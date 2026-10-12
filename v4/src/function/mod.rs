//! Functions: the typed units of work a task runs.

#[doc(hidden)]
pub mod builtins;
mod config;

pub use crate::expr::{ExprVisitor, FieldKind, FieldSpec, ValueKind};
pub use config::FunctionConfig;
/// Derives [`FunctionConfig`](trait@FunctionConfig) (and `ConfigField`, so a
/// derived config nests).
pub use dataflow_rs_derive::FunctionConfig;

/// What `#[derive(FunctionConfig)]` expands to names, and nothing else. Not an
/// API: the derive emits `::dataflow_rs::function::__private::…` paths, which
/// resolve here.
#[doc(hidden)]
pub mod __private {
    pub use super::config::FunctionConfig;
    pub use crate::base::AuthoredPath;
    pub use crate::expr::{
        CompileCx, ConfigField, ExprVisitor, FieldKind, FieldSpec, ValueKind, Visit,
    };
    pub use serde_json::Value;
    pub use std::sync::OnceLock;

    use crate::base::IssueCode;
    use serde::de::DeserializeOwned;
    use serde_json::Map;
    use std::borrow::Cow;

    /// One field's key and its declared default, for the compile helpers.
    #[derive(Clone, Copy)]
    pub struct Key {
        /// The authored key.
        key: &'static str,
        /// The default, as JSON text.
        default: Option<&'static str>,
        /// `Struct::field`, for the panic on a default that is not JSON.
        site: &'static str,
        /// An output: an explicit `null` is absent.
        output: bool,
    }

    impl Key {
        /// A field's key: its authored name, its default as JSON text,
        /// `Struct::field`, and whether it is an output.
        pub const fn new(
            key: &'static str,
            default: Option<&'static str>,
            site: &'static str,
            output: bool,
        ) -> Self {
            Self {
                key,
                default,
                site,
                output,
            }
        }
    }

    /// The input as an object, or `INPUT_PARSE` at the current path.
    pub fn object<'v>(input: &'v Value, cx: &mut CompileCx<'_>) -> Option<&'v Map<String, Value>> {
        let object = input.as_object();
        if object.is_none() {
            cx.issue(IssueCode::InputParse, "expected an object");
        }
        object
    }

    /// Reports each key of `object` that is not in `known`, in the input's
    /// key order.
    pub fn unknown_keys(object: &Map<String, Value>, known: &[&str], cx: &mut CompileCx<'_>) {
        for key in object.keys().filter(|key| !known.contains(&key.as_str())) {
            cx.at(key.as_str(), |cx| cx.unknown_key(key, known));
        }
    }

    /// The field's raw value: present (and, for an output, not `null`), else
    /// its parsed default.
    fn raw(object: &Map<String, Value>, key: Key) -> Option<Cow<'_, Value>> {
        match object.get(key.key) {
            Some(raw) if !(key.output && raw.is_null()) => Some(Cow::Borrowed(raw)),
            _ => key.default.map(|text| Cow::Owned(default(text, key.site))),
        }
    }

    /// A declared default, parsed.
    ///
    /// # Panics
    ///
    /// When the text is not JSON: a `default_json` the author got wrong.
    fn default(text: &str, site: &str) -> Value {
        serde_json::from_str(text)
            .unwrap_or_else(|e| panic!("the default of `{site}` is not JSON ({e}): {text}"))
    }

    fn missing(key: &str, cx: &mut CompileCx<'_>) {
        cx.issue(IssueCode::InputParse, format!("missing field `{key}`"));
    }

    /// Compiles one field through its [`ConfigField`] impl, at its key.
    pub fn field<T: ConfigField>(
        object: &Map<String, Value>,
        key: Key,
        cx: &mut CompileCx<'_>,
    ) -> Option<T> {
        cx.at(key.key, |cx| match raw(object, key) {
            Some(raw) => T::compile(&raw, cx),
            None => T::absent().or_else(|| {
                missing(key.key, cx);
                None
            }),
        })
    }

    /// Compiles one `#[config(serde)]` field with `serde_json`, at its key.
    pub fn serde_field<T: DeserializeOwned>(
        object: &Map<String, Value>,
        key: Key,
        cx: &mut CompileCx<'_>,
    ) -> Option<T> {
        cx.at(key.key, |cx| {
            let Some(raw) = raw(object, key) else {
                return serde_json::from_value(Value::Null).map_or_else(
                    |_| {
                        missing(key.key, cx);
                        None
                    },
                    Some,
                );
            };
            serde_json::from_value(raw.into_owned())
                .map_err(|e| cx.issue(IssueCode::InputParse, e.to_string()))
                .ok()
        })
    }

    /// Whether a `#[config(serde)]` field is required: `T` cannot be read
    /// from `null`.
    pub fn serde_required<T: DeserializeOwned>() -> bool {
        serde_json::from_value::<T>(Value::Null).is_err()
    }

    /// The assembled config, unless this compile recorded a Rejected issue.
    pub fn finish<T>(out: T, rejected_before: usize, cx: &CompileCx<'_>) -> Option<T> {
        (cx.rejected() == rejected_before).then_some(out)
    }

    /// One [`FieldSpec`], as the derive declares it.
    pub fn spec(
        name: &'static str,
        kind: FieldKind,
        required: bool,
        records: bool,
        default: Option<&'static str>,
        primary: bool,
        description: Option<&'static str>,
    ) -> FieldSpec {
        FieldSpec {
            name: Cow::Borrowed(name),
            kind,
            required,
            records,
            default: default.map(Cow::Borrowed),
            primary,
            description: description.map(Cow::Borrowed),
        }
    }
}
