//! `FunctionConfig`: a function's input, compiled once at build.

use crate::expr::{CompileCx, ExprVisitor, FieldSpec};
use serde_json::Value;

/// A function's `input`, compiled at build so a malformed one fails before any
/// message runs. `#[derive(FunctionConfig)]` writes the implementation.
pub trait FunctionConfig: Sized + Send + Sync + 'static {
    /// Every issue in the config, each at its exact path below the
    /// [`CompileCx`]'s root. `None` when any is Rejected.
    fn compile(input: &Value, cx: &mut CompileCx<'_>) -> Option<Self>;

    /// Walks the config's expressions; paths are relative to the config's root.
    fn visit(&self, v: &mut dyn ExprVisitor);

    /// The config's fields, for tooling.
    fn fields() -> &'static [FieldSpec];
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
                crate::IssueCode::InputParse,
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
