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
    /// An `Expr(ValueKind::Path)` field, compiled as the derive's `Expr<Path>`
    /// is, so it is refused the same way.
    Path(Expr<Path>),
    Interpolated(Interpolated),
    Literal(OwnedDataValue),
    Output(Output),
}

/// A compiled runtime field with its name, and whether the engine records
/// its result ([`FieldSpec::records`]).
#[derive(Debug)]
struct Field {
    name: Box<str>,
    records: bool,
    value: TemplatedField,
}

/// A plugin function's config: the fields its manifest declared, compiled by
/// their kinds. The fields come from the [`CompileCx`]'s runtime fields.
#[derive(Debug)]
pub struct Templated {
    fields: Vec<Field>,
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
        FieldKind::Expr(ValueKind::Path) => {
            <Expr<Path>>::compile(raw, cx).map(TemplatedField::Path)
        }
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
                if let Some(value) = Self::one(spec, object.get(&*spec.name), cx) {
                    fields.push(Field {
                        name: Box::from(&*spec.name),
                        records: spec.records,
                        value,
                    });
                }
            });
        }
        (cx.rejected() == before).then_some(Self { fields })
    }

    /// One runtime field, or `None` when it is absent or rejected.
    ///
    /// An explicit `null` reads as absent, so the spec's default applies and a
    /// required field is missing, unless the field's kind takes `null` as a
    /// value: an expression, a document, or a literal of any JSON. That is
    /// how a derived config's `Option` field reads it (decision I53).
    fn one(
        spec: &FieldSpec,
        raw: Option<&Value>,
        cx: &mut CompileCx<'_>,
    ) -> Option<TemplatedField> {
        let takes_null = matches!(
            spec.kind,
            FieldKind::Expr(_) | FieldKind::Interpolated | FieldKind::Literal(ValueKind::Any)
        );
        match raw.filter(|raw| takes_null || !raw.is_null()) {
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
        for field in &self.fields {
            v.at(&*field.name, |v| {
                v.recording(field.records, |v| match &field.value {
                    TemplatedField::Expr(expr) => v.expr(expr.raw()),
                    TemplatedField::Path(expr) => v.expr(expr.raw()),
                    // As recorded, as an `Output` always is.
                    TemplatedField::Output(output) => output.visit(v),
                    TemplatedField::Interpolated(doc) => v.interpolated(doc),
                    TemplatedField::Literal(_) => {}
                });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Issue, Severity};
    use crate::expr::ExprEngine;
    use serde_json::json;

    fn specs() -> Vec<FieldSpec> {
        vec![
            FieldSpec::runtime("label", FieldKind::Literal(ValueKind::String), false),
            FieldSpec::runtime("mode", FieldKind::Literal(ValueKind::String), false)
                .with_default("\"fast\""),
            FieldSpec::runtime("out", FieldKind::Output, false),
            FieldSpec::runtime("name", FieldKind::Literal(ValueKind::String), true),
            FieldSpec::runtime("value", FieldKind::Expr(ValueKind::Any), false),
            FieldSpec::runtime("doc", FieldKind::Interpolated, false),
            FieldSpec::runtime("any", FieldKind::Literal(ValueKind::Any), false),
        ]
    }

    fn compile(input: &Value) -> (Option<Templated>, Vec<Issue>) {
        let engine = ExprEngine::default();
        let specs = specs();
        let mut issues = Vec::new();
        let out = {
            let mut cx = CompileCx::new(&engine, &mut issues)
                .at_path("tasks[0].function.input".parse().unwrap())
                .with_runtime_fields(&specs);
            <Templated as FunctionConfig>::compile(input, &mut cx)
        };
        (out, issues)
    }

    fn get<'t>(templated: &'t Templated, name: &str) -> Option<&'t TemplatedField> {
        templated
            .fields
            .iter()
            .find(|f| &*f.name == name)
            .map(|f| &f.value)
    }

    #[test]
    fn a_null_reads_as_absent_unless_the_kind_takes_null() {
        let (out, issues) = compile(&json!({
            "label": null, "mode": null, "out": null, "name": "n",
            "value": null, "doc": null, "any": null,
        }));
        assert!(issues.is_empty(), "{issues:?}");
        let out = out.unwrap();
        // A literal string and an output have no null: absent, and the
        // declared default applies.
        assert!(get(&out, "label").is_none());
        assert!(get(&out, "out").is_none());
        assert!(matches!(
            get(&out, "mode"),
            Some(TemplatedField::Literal(OwnedDataValue::String(s))) if s == "fast"
        ));
        // An expression, a document and any JSON take `null` as a value.
        assert!(matches!(
            get(&out, "value"),
            Some(TemplatedField::Expr(e)) if e.constant() == Some(&OwnedDataValue::Null)
        ));
        assert!(matches!(
            get(&out, "doc"),
            Some(TemplatedField::Interpolated(_))
        ));
        assert!(matches!(
            get(&out, "any"),
            Some(TemplatedField::Literal(OwnedDataValue::Null))
        ));
    }

    #[test]
    fn a_null_on_a_required_literal_is_a_missing_field() {
        let (out, issues) = compile(&json!({"name": null}));
        assert!(out.is_none());
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].code, IssueCode::InputParse);
        assert_eq!(issues[0].severity, Severity::Rejected);
        assert_eq!(issues[0].path.to_string(), "tasks[0].function.input.name");
        assert_eq!(issues[0].message, "missing field `name`");
    }
}
