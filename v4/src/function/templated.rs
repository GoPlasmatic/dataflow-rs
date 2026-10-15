//! `Templated`: the config of a plugin function whose fields are declared at
//! runtime.

use super::{Ctx, FunctionConfig};
use crate::base::AuthoredPath;
use crate::base::{IssueCode, Result};
use crate::expr::{
    CompileCx, ConfigField, Expr, ExprVisitor, FieldKind, FieldSpec, Interpolated, Output, Path,
    Resolver, ValueKind, Visit,
};
use datavalue::OwnedDataValue;
use serde_json::Value;
use std::borrow::Cow;
use std::str::FromStr;

/// One compiled runtime field.
#[derive(Debug)]
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
///
/// A call reads them by name with [`Templated::get`], or all at once with
/// [`Templated::resolved`]. Its primary output is the only `Output` field the
/// manifest declares, or, of several, the one marked
/// [`primary`](FieldSpec::primary).
#[derive(Debug)]
pub struct Templated {
    /// The fields the input set, in declaration order.
    fields: Vec<Field>,
    /// The primary output's position in `fields`, when the input set it.
    primary: Option<usize>,
}

impl TemplatedField {
    /// The field's value through `r`, as the derive's field of the same kind
    /// resolves: through that kind's [`ConfigField::resolve`].
    fn resolve(&self, r: &mut dyn Resolver) -> Result<OwnedDataValue> {
        let value = match self {
            Self::Expr(expr) => expr.resolve(r)?,
            Self::Path(expr) => expr.resolve(r)?,
            Self::Interpolated(doc) => doc.resolve(r)?,
            Self::Literal(value) => value.resolve(r)?,
            Self::Output(out) => out.resolve(r)?,
        };
        // Every kind a runtime field compiles to is set once compiled.
        Ok(value.unwrap_or(OwnedDataValue::Null))
    }

    /// The field's value for one call: a literal and a folded constant lent
    /// from the config, anything else resolved, as [`Self::resolve`] gives
    /// it, owned.
    fn lend<'s>(&'s self, ctx: &Ctx<'_>) -> Result<Cow<'s, OwnedDataValue>> {
        match self {
            Self::Literal(value) => return Ok(Cow::Borrowed(value)),
            Self::Expr(expr) => {
                if let Some(constant) = expr.constant() {
                    return Ok(Cow::Borrowed(constant));
                }
            }
            _ => {}
        }
        self.resolve(&mut &*ctx).map(Cow::Owned)
    }
}

/// What makes `specs` break the derive's rule for outputs, if anything:
/// of several `Output` fields, exactly one is marked
/// [`primary`](FieldSpec::primary). [`Registry::register`](super::Registry::register)
/// refuses a `Templated` function whose runtime fields break it, as the
/// derive refuses such a struct at expansion.
pub(super) fn primary_problem(specs: &[FieldSpec]) -> Option<String> {
    let (outputs, primaries) = specs
        .iter()
        .filter(|spec| matches!(spec.kind, FieldKind::Output))
        .fold((0, 0), |(n, p), spec| {
            (n + 1, p + usize::from(spec.primary))
        });
    (outputs > 1 && primaries != 1).then(|| {
        format!(
            "its runtime fields hold {outputs} outputs, {primaries} of them primary: mark \
             exactly one of them `primary`"
        )
    })
}

/// The name of the primary output among `specs`: the only `Output` field,
/// or, of several, the one marked primary, which `register` makes sure of.
pub(super) fn primary_name(specs: &[FieldSpec]) -> Option<&str> {
    let is_output = |spec: &&FieldSpec| matches!(spec.kind, FieldKind::Output);
    let mut outputs = specs.iter().filter(is_output);
    let first = outputs.next()?;
    if outputs.next().is_none() {
        return Some(&first.name);
    }
    specs
        .iter()
        .filter(is_output)
        .find(|spec| spec.primary)
        .map(|spec| &*spec.name)
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
        let primary = primary_name(specs)
            .and_then(|name| fields.iter().position(|field| &*field.name == name));
        (cx.rejected() == before).then_some(Self { fields, primary })
    }

    /// The field `name`'s value for this call: a literal, and an expression
    /// that folded to a constant, lent from the config; an expression
    /// evaluated, a document rendered, and an output's or a path's
    /// destination as a string, owned. `None` for a field the input did not
    /// set, and for a name no field has.
    ///
    /// # Errors
    ///
    /// [`Ctx::eval`]'s, for an expression, a path or an output.
    pub fn get<'s>(&'s self, ctx: &Ctx<'_>, name: &str) -> Result<Option<Cow<'s, OwnedDataValue>>> {
        self.fields
            .iter()
            .find(|field| &*field.name == name)
            .map(|field| field.value.lend(ctx))
            .transpose()
    }

    /// Every field the input set, each as [`Self::get`] gives it, as an
    /// object in declaration order: what a derived config with the same
    /// fields resolves to ([`FunctionConfig::resolve`], on the same per-field
    /// code). Secrets are read, as the call reads them.
    ///
    /// # Errors
    ///
    /// The first field's that fails, as [`Self::get`]'s.
    pub fn resolved(&self, ctx: &Ctx<'_>) -> Result<OwnedDataValue> {
        self.resolve_fields(&mut &*ctx)
    }

    /// Every field the input set, through `r`, as an object in declaration
    /// order: [`FunctionConfig::resolve`], which a derived config with the
    /// same fields gives alike.
    fn resolve_fields(&self, r: &mut dyn Resolver) -> Result<OwnedDataValue> {
        let entries = self
            .fields
            .iter()
            .map(|field| Ok((field.name.to_string(), field.value.resolve(r)?)))
            .collect::<Result<Vec<_>>>()?;
        Ok(OwnedDataValue::Object(entries))
    }

    /// One runtime field, or `None` when it is absent or rejected.
    ///
    /// An absent key takes the spec's default, and a required field with none
    /// is missing. An explicit `null` is compiled when the field's kind takes
    /// `null` as a value: an expression, a document, or a literal of any
    /// JSON. On any other kind it reads as absent without the default, so a
    /// required field is missing even when it declares one; an output alone
    /// keeps its default, as a derived config's does (decision I53).
    fn one(
        spec: &FieldSpec,
        raw: Option<&Value>,
        cx: &mut CompileCx<'_>,
    ) -> Option<TemplatedField> {
        let takes_null = matches!(
            spec.kind,
            FieldKind::Expr(_) | FieldKind::Interpolated | FieldKind::Literal(ValueKind::Any)
        );
        let default = match raw {
            Some(raw) if takes_null || !raw.is_null() => return field(spec.kind, raw, cx),
            // A `null` the kind has no value for: absent, with no default.
            Some(_) if !matches!(spec.kind, FieldKind::Output) => None,
            _ => spec.default.as_deref(),
        };
        if let Some(text) = default {
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
    fn resolve(&self, r: &mut dyn Resolver) -> Result<Option<OwnedDataValue>> {
        self.resolve_fields(r).map(Some)
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

    fn primary_output(&self) -> Option<&Output> {
        match &self.fields[self.primary?].value {
            TemplatedField::Output(out) => Some(out),
            _ => None,
        }
    }

    /// [`Templated::resolved`], through `r`.
    fn resolve(&self, r: &mut dyn Resolver) -> Result<OwnedDataValue> {
        self.resolve_fields(r)
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
            FieldSpec::runtime("dest", FieldKind::Output, false).with_default("\"data.dest\""),
            // Required and defaulted: a host can build this through the
            // public fields.
            FieldSpec::runtime("both", FieldKind::Literal(ValueKind::String), true)
                .with_default("\"fallback\""),
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
            "value": null, "doc": null, "any": null, "dest": null,
        }));
        assert!(issues.is_empty(), "{issues:?}");
        let out = out.unwrap();
        // A literal string has no null: absent, and its declared default is
        // not applied.
        assert!(get(&out, "label").is_none());
        assert!(get(&out, "mode").is_none());
        // An output has no null either, but keeps its default.
        assert!(get(&out, "out").is_none());
        assert!(matches!(get(&out, "dest"), Some(TemplatedField::Output(_))));
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
    fn the_primary_output_is_the_only_output_or_the_marked_one() {
        let primary = |specs: &[FieldSpec], input: &Value| {
            let engine = ExprEngine::default();
            let mut issues = Vec::new();
            let mut cx = CompileCx::new(&engine, &mut issues).with_runtime_fields(specs);
            let templated = <Templated as FunctionConfig>::compile(input, &mut cx).unwrap();
            templated
                .primary_output()
                .map(|out| out.constant().unwrap().to_string())
        };
        let input = json!({"name": "n", "out": "data.out"});
        // Two outputs, `out` and `dest`, neither marked: none.
        assert_eq!(primary(&specs(), &input), None);
        // One of them marked: it, whether the input set it or its default did.
        let mut marked = specs();
        marked
            .iter_mut()
            .find(|s| s.name == "dest")
            .unwrap()
            .primary = true;
        assert_eq!(primary(&marked, &input).as_deref(), Some("data.dest"));
        // The only output, unmarked; none when the input leaves it unset.
        let one: Vec<FieldSpec> = specs().into_iter().filter(|s| s.name != "dest").collect();
        assert_eq!(primary(&one, &input).as_deref(), Some("data.out"));
        assert_eq!(primary(&one, &json!({"name": "n"})), None);
    }

    #[test]
    fn a_null_on_a_required_literal_is_a_missing_field() {
        let (out, issues) = compile(&json!({"name": null}));
        assert!(out.is_none());
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].code, IssueCode::InputParse);
        assert_eq!(issues[0].severity, Severity::Rejected);
        assert_eq!(issues[0].path.to_string(), "tasks[0].function.input.name");
    }

    #[test]
    fn a_null_on_a_required_field_with_a_default_is_missing_and_absence_is_not() {
        // An absent key takes the default.
        let (out, issues) = compile(&json!({"name": "n"}));
        assert!(issues.is_empty(), "{issues:?}");
        assert!(matches!(
            get(&out.unwrap(), "both"),
            Some(TemplatedField::Literal(OwnedDataValue::String(s))) if s == "fallback"
        ));
        // An explicit `null` reads as absent with no default: missing.
        let (out, issues) = compile(&json!({"name": "n", "both": null}));
        assert!(out.is_none());
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].code, IssueCode::InputParse);
        assert_eq!(issues[0].severity, Severity::Rejected);
        assert_eq!(issues[0].path.to_string(), "tasks[0].function.input.both");
    }
}
