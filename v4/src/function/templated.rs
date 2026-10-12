//! `Templated`: the config of a plugin function whose fields are declared at
//! runtime.

use super::FunctionConfig;
use crate::base::AuthoredPath;
use crate::base::IssueCode;
use crate::expr::{
    CompileCx, ConfigField, Expr, ExprVisitor, FieldKind, FieldSpec, Interpolated, Output, Path,
    ValueKind, Visit,
};
use datavalue::OwnedDataValue;
use serde_json::Value;
use std::str::FromStr;

/// One compiled runtime field.
#[derive(Debug)]
#[allow(dead_code, reason = "a literal is read by phase 5's `get`")]
enum TemplatedField {
    Expr(Expr<OwnedDataValue>),
    Interpolated(Interpolated),
    Literal(OwnedDataValue),
    Output(Output),
}

/// A plugin function's config: the fields its manifest declared, compiled by
/// their kinds. The fields come from the [`CompileCx`]'s runtime fields.
#[derive(Debug)]
pub struct Templated {
    fields: Vec<(Box<str>, TemplatedField)>,
}

fn raw_fits(kind: ValueKind, raw: &Value) -> bool {
    match kind {
        ValueKind::Any => true,
        ValueKind::String => raw.is_string(),
        ValueKind::Number => raw.is_number(),
        ValueKind::Bool => raw.is_boolean(),
        ValueKind::Object => raw.is_object(),
        ValueKind::Array => raw.is_array(),
        ValueKind::Path => raw.is_string(),
    }
}

fn constant_fits(kind: ValueKind, value: &OwnedDataValue) -> bool {
    match kind {
        ValueKind::Any => true,
        ValueKind::String => matches!(value, OwnedDataValue::String(_)),
        ValueKind::Number => matches!(value, OwnedDataValue::Number(_)),
        ValueKind::Bool => matches!(value, OwnedDataValue::Bool(_)),
        ValueKind::Object => matches!(value, OwnedDataValue::Object(_)),
        ValueKind::Array => matches!(value, OwnedDataValue::Array(_)),
        ValueKind::Path => matches!(value, OwnedDataValue::String(s) if Path::from_str(s).is_ok()),
    }
}

fn expected(kind: ValueKind) -> &'static str {
    match kind {
        ValueKind::String => "a string",
        ValueKind::Number => "a number",
        ValueKind::Bool => "a boolean",
        ValueKind::Object => "an object",
        ValueKind::Array => "an array",
        ValueKind::Path => "a path",
        ValueKind::Any => "a value",
    }
}

/// Compiles `raw` as the spec's kind, at the current path.
fn field(kind: FieldKind, raw: &Value, cx: &mut CompileCx<'_>) -> Option<TemplatedField> {
    match kind {
        FieldKind::Expr(vk) => {
            let expr = <Expr<OwnedDataValue>>::compile(raw, cx)?;
            if expr.constant().is_some_and(|c| !constant_fits(vk, c)) {
                cx.issue(IssueCode::InputParse, format!("expected {}", expected(vk)));
                return None;
            }
            Some(TemplatedField::Expr(expr))
        }
        FieldKind::Interpolated => Interpolated::compile(raw, cx).map(TemplatedField::Interpolated),
        FieldKind::Literal(vk) => {
            if !raw_fits(vk, raw) {
                cx.issue(IssueCode::InputParse, format!("expected {}", expected(vk)));
                return None;
            }
            if vk == ValueKind::Path && !constant_fits(vk, &OwnedDataValue::from(raw)) {
                cx.issue(IssueCode::InvalidPath, "not a valid context path");
                return None;
            }
            Some(TemplatedField::Literal(OwnedDataValue::from(raw)))
        }
        FieldKind::Output => Output::compile(raw, cx).map(TemplatedField::Output),
        _ => {
            cx.issue(IssueCode::InputParse, "unsupported runtime field kind");
            None
        }
    }
}

impl Templated {
    fn compile_fields(input: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
        let Some(specs) = cx.runtime_fields() else {
            cx.issue(
                IssueCode::InputParse,
                "no runtime fields are declared for this function",
            );
            return None;
        };
        let Some(object) = input.as_object() else {
            cx.issue(IssueCode::InputParse, "expected an object");
            return None;
        };
        let before = cx.rejected();
        let names: Vec<&str> = specs.iter().map(|s| &*s.name).collect();
        for key in object.keys().filter(|k| !names.contains(&k.as_str())) {
            cx.at(key.as_str(), |cx| cx.unknown_key(key, &names));
        }
        let mut fields = Vec::new();
        for spec in specs {
            cx.at(&*spec.name, |cx| {
                if let Some(compiled) = Self::one(spec, object.get(&*spec.name), cx) {
                    fields.push((Box::from(&*spec.name), compiled));
                }
            });
        }
        (cx.rejected() == before).then_some(Self { fields })
    }

    /// One runtime field, or `None` when it is absent or rejected.
    fn one(
        spec: &FieldSpec,
        raw: Option<&Value>,
        cx: &mut CompileCx<'_>,
    ) -> Option<TemplatedField> {
        let output = matches!(spec.kind, FieldKind::Output);
        match raw.filter(|raw| !(output && raw.is_null())) {
            Some(raw) => field(spec.kind, raw, cx),
            None => {
                if let Some(text) = &spec.default {
                    return match serde_json::from_str::<Value>(text) {
                        Ok(default) => field(spec.kind, &default, cx),
                        Err(e) => {
                            cx.issue(
                                IssueCode::InputParse,
                                format!("the default of `{}` is not JSON ({e})", spec.name),
                            );
                            None
                        }
                    };
                }
                if spec.required {
                    cx.issue(
                        IssueCode::InputParse,
                        format!("missing field `{}`", spec.name),
                    );
                }
                None
            }
        }
    }

    fn walk(&self, v: &mut Visit<'_>) {
        for (name, field) in &self.fields {
            v.at(&**name, |v| match field {
                TemplatedField::Expr(expr) => v.expr(expr.raw()),
                TemplatedField::Output(output) => v.expr(output.path().raw()),
                TemplatedField::Interpolated(doc) => v.interpolated(doc),
                TemplatedField::Literal(_) => {}
            });
        }
    }
}

impl ConfigField for Templated {
    fn compile(raw: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
        Self::compile_fields(raw, cx)
    }
    fn visit(&self, v: &mut Visit<'_>) {
        self.walk(v);
    }
    fn kind() -> FieldKind {
        FieldKind::Templated
    }
}

impl FunctionConfig for Templated {
    fn compile(input: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
        Self::compile_fields(input, cx)
    }
    fn visit(&self, v: &mut dyn ExprVisitor) {
        self.walk(&mut Visit::new(v, AuthoredPath::root()));
    }
    fn fields() -> &'static [FieldSpec] {
        &[]
    }
}
