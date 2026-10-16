//! The proc-macro behind `#[derive(FunctionConfig)]`: a derive must live in
//! a proc-macro crate of its own, and one derive serves the built-ins and
//! hosts alike.
//!
//! The generated code names nothing but `::dataflow_rs::function::__private`,
//! `::core` and `::std`, so the library must be reachable as `::dataflow_rs`.
//! A dependent keeps the dependency key `dataflow-rs`, and renames v3, not
//! v4, when both are in scope. Inside the crate, `extern crate self as
//! dataflow_rs` does it.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::parse::ParseStream;
use syn::spanned::Spanned;
use syn::{
    Attribute, Data, DeriveInput, Error, Expr, ExprLit, Fields, GenericArgument, Ident, Lit,
    LitStr, Meta, Path, PathArguments, Result, Token, Type,
};

/// Derives `FunctionConfig` for a function's typed configuration, and
/// `ConfigField`, so a derived config nests inside another.
///
/// It applies to a struct with named fields. Each field is compiled from the
/// key of its name through the field type's `ConfigField` implementation,
/// every issue at its own path, unknown keys first.
///
/// | Attribute | On | Meaning |
/// | --- | --- | --- |
/// | `#[config(rename = "k")]` | field | The JSON key; the default is the field's name |
/// | `#[config(default = <literal>)]` | field | A string, integer, float or bool, compiled through the field's type when the key is absent |
/// | `#[config(default_json = "<json>")]` | field | Any JSON default, as text; text that is not JSON is a compile error |
/// | `#[config(serde)]` | field | Read with `serde_json`; an error is `INPUT_PARSE`, and an absent key is read from `null`. Resolved as it serializes, so the type is `Serialize` too |
/// | `#[config(records)]` | field | The engine records the field's result; inherited by everything below it |
/// | `#[output(default = "p")]` | output | The default when the key is absent or `null` |
/// | `#[output(primary)]` | output | Required on exactly one output when there are several |
/// | `#[config(check = path)]` | struct | `fn(&Self, &mut CompileCx<'_>)`, called only when every field compiled |
///
/// A field is an output when it carries `#[output(...)]`, or its type's last
/// path segment is `Output`, or it is an `Option` of such a type. A field's
/// doc comment is its description.
///
/// The primary output, the one marked or the only one, is what
/// `FunctionConfig::primary_output` returns, so its type is `Output` or
/// `Option<Output>`; any other is refused.
///
/// `FunctionConfig::resolve` gives every field with a value, set by the
/// input or by its default, keyed by its key, in declaration order, each
/// through its type's `ConfigField::resolve`, and a `serde` field as it
/// serializes; an `Option` left unset is absent.
#[proc_macro_derive(FunctionConfig, attributes(config, output))]
pub fn derive_function_config(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    expand(&input)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

/// One field, as its attributes declare it.
struct Field<'a> {
    ident: &'a Ident,
    ty: &'a Type,
    key: String,
    /// The default as JSON text.
    default: Option<String>,
    serde: bool,
    records: bool,
    output: bool,
    primary: bool,
    description: Option<String>,
}

fn expand(input: &DeriveInput) -> Result<TokenStream2> {
    let name = &input.ident;
    // A raw identifier names its key and its messages without the `r#`.
    let shown = name.unraw();
    let named = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => return Err(not_a_struct(input)),
        },
        _ => return Err(not_a_struct(input)),
    };
    if !input.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &input.generics,
            "`#[derive(FunctionConfig)]` does not support generic parameters",
        ));
    }
    let check = struct_check(&input.attrs)?;

    let mut errors: Option<Error> = None;
    let mut fields = Vec::new();
    for field in named {
        match parse_field(field) {
            Ok(f) => fields.push(f),
            Err(e) => push_error(&mut errors, e),
        }
    }
    for (i, f) in fields.iter().enumerate() {
        if fields[..i].iter().any(|g| g.key == f.key) {
            push_error(
                &mut errors,
                Error::new_spanned(f.ident, format!("the key `{}` is used twice", f.key)),
            );
        }
    }
    let outputs = fields.iter().filter(|f| f.output).count();
    let primaries = fields.iter().filter(|f| f.primary).count();
    if outputs > 1 && primaries != 1 {
        push_error(
            &mut errors,
            Error::new_spanned(
                name,
                format!(
                    "`{shown}` has {outputs} outputs: mark exactly one of them `#[output(primary)]`"
                ),
            ),
        );
    }
    if let Some(errors) = errors {
        return Err(errors);
    }
    if outputs == 1 {
        for f in fields.iter_mut().filter(|f| f.output) {
            f.primary = true;
        }
    }
    let primary = fields
        .iter()
        .find(|f| f.primary)
        .map(|f| primary_shape(f).map(|optional| (f.ident, optional)))
        .transpose()?;

    Ok(generate(name, check.as_ref(), &fields, primary))
}

/// Whether the primary output `f` is an `Option<Output>` rather than an
/// `Output`; an error for any other type.
fn primary_shape(f: &Field<'_>) -> Result<bool> {
    if is_output_type(f.ty) {
        Ok(false)
    } else if is_option_of_output(f.ty) {
        Ok(true)
    } else {
        Err(Error::new_spanned(
            f.ty,
            format!(
                "the primary output `{}` must be an `Output` or an `Option<Output>`",
                f.ident.unraw()
            ),
        ))
    }
}

fn not_a_struct(input: &DeriveInput) -> Error {
    Error::new_spanned(
        &input.ident,
        "`#[derive(FunctionConfig)]` applies to a struct with named fields; for any other \
         type, derive `serde::Deserialize` and `serde::Serialize`, and read it through a \
         `#[config(serde)]` field",
    )
}

fn push_error(errors: &mut Option<Error>, error: Error) {
    match errors {
        Some(errors) => errors.combine(error),
        None => *errors = Some(error),
    }
}

/// The struct's `#[config(check = path)]`, if any.
fn struct_check(attrs: &[Attribute]) -> Result<Option<Path>> {
    let mut check = None;
    for attr in attrs {
        if attr.path().is_ident("output") {
            return Err(Error::new_spanned(attr, "`#[output]` goes on a field"));
        }
        if !attr.path().is_ident("config") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("check") {
                if check.is_some() {
                    return Err(meta.error("`check` is declared twice"));
                }
                check = Some(meta.value()?.parse::<Path>()?);
                Ok(())
            } else {
                Err(meta.error("on a struct, `config` takes only `check = path`"))
            }
        })?;
    }
    Ok(check)
}

fn parse_field(field: &syn::Field) -> Result<Field<'_>> {
    let Some(ident) = field.ident.as_ref() else {
        return Err(Error::new_spanned(field, "a field needs a name"));
    };
    let mut f = Field {
        ident,
        ty: &field.ty,
        // `r#type` is the key `type`.
        key: ident.unraw().to_string(),
        default: None,
        serde: false,
        records: false,
        output: false,
        primary: false,
        description: None,
    };
    let mut output_attr = false;
    let mut docs = Vec::new();
    // Each `config(…)` and `output(…)` key this field has given, across all of
    // its attributes: a repeat is refused, as a second `default` is.
    let mut given: Vec<String> = Vec::new();
    let mut once = |attr: &str, meta: &syn::meta::ParseNestedMeta<'_>| -> Result<()> {
        let key = meta
            .path
            .get_ident()
            .map_or_else(String::new, ToString::to_string);
        let seen = format!("{attr}({key})");
        if given.contains(&seen) {
            return Err(meta.error(format!("`{key}` is given twice on this field")));
        }
        given.push(seen);
        Ok(())
    };
    for attr in &field.attrs {
        if attr.path().is_ident("config") {
            attr.parse_nested_meta(|meta| {
                once("config", &meta)?;
                if meta.path.is_ident("rename") {
                    f.key = meta.value()?.parse::<LitStr>()?.value();
                } else if meta.path.is_ident("default") {
                    let json = literal_json(meta.value()?)?;
                    set_default(&mut f.default, json, &meta)?;
                } else if meta.path.is_ident("default_json") {
                    let json = json_text(&meta.value()?.parse::<LitStr>()?)?;
                    set_default(&mut f.default, json, &meta)?;
                } else if meta.path.is_ident("serde") {
                    f.serde = true;
                } else if meta.path.is_ident("records") {
                    f.records = true;
                } else if meta.path.is_ident("check") {
                    return Err(meta.error("`check` goes on the struct"));
                } else {
                    return Err(meta.error(
                        "unknown `config` attribute: expected `rename`, `default`, \
                         `default_json`, `serde` or `records`",
                    ));
                }
                Ok(())
            })?;
        } else if attr.path().is_ident("output") {
            output_attr = true;
            attr.parse_nested_meta(|meta| {
                once("output", &meta)?;
                if meta.path.is_ident("default") {
                    let path = meta.value()?.parse::<LitStr>()?.value();
                    set_default(&mut f.default, json_string(&path), &meta)?;
                } else if meta.path.is_ident("primary") {
                    f.primary = true;
                } else {
                    return Err(meta.error(
                        "unknown `output` attribute: expected `default = \"path\"` or `primary`",
                    ));
                }
                Ok(())
            })?;
        } else if attr.path().is_ident("doc")
            && let Meta::NameValue(doc) = &attr.meta
            && let Expr::Lit(ExprLit {
                lit: Lit::Str(text),
                ..
            }) = &doc.value
        {
            docs.push(text.value());
        }
    }
    if f.serde && output_attr {
        return Err(Error::new_spanned(
            ident,
            "a `#[config(serde)]` field cannot be an `#[output]`",
        ));
    }
    if f.serde && f.records {
        return Err(Error::new_spanned(
            ident,
            "a `#[config(serde)]` field holds no expression, so it cannot be \
             `#[config(records)]`",
        ));
    }
    f.output = output_attr || (!f.serde && (is_output_type(f.ty) || is_option_of_output(f.ty)));
    let lines: Vec<&str> = docs
        .iter()
        .flat_map(|doc| doc.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if !lines.is_empty() {
        f.description = Some(lines.join(" "));
    }
    Ok(f)
}

fn set_default(
    slot: &mut Option<String>,
    json: String,
    meta: &syn::meta::ParseNestedMeta<'_>,
) -> Result<()> {
    if slot.is_some() {
        return Err(meta.error("a field takes one default"));
    }
    *slot = Some(json);
    Ok(())
}

/// Does the type's last path segment name `Output`?
fn is_output_type(ty: &Type) -> bool {
    match ty {
        Type::Path(path) => {
            path.qself.is_none()
                && path
                    .path
                    .segments
                    .last()
                    .is_some_and(|s| s.ident == "Output")
        }
        _ => false,
    }
}

/// Is the type an `Option` of a type whose last path segment is `Output`?
fn is_option_of_output(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    let Some(last) = path.path.segments.last() else {
        return false;
    };
    if path.qself.is_some() || last.ident != "Option" {
        return false;
    }
    match &last.arguments {
        PathArguments::AngleBracketed(args) => {
            args.args.len() == 1
                && matches!(
                    args.args.first(),
                    Some(GenericArgument::Type(inner)) if is_output_type(inner)
                )
        }
        _ => false,
    }
}

/// A `default = <literal>` as JSON text: a string, an integer or float
/// (optionally negative), or a bool.
fn literal_json(input: ParseStream<'_>) -> Result<String> {
    let negative = input.peek(Token![-]);
    if negative {
        input.parse::<Token![-]>()?;
    }
    let sign = if negative { "-" } else { "" };
    match input.parse::<Lit>()? {
        Lit::Str(s) if !negative => Ok(json_string(&s.value())),
        Lit::Bool(b) if !negative => Ok(b.value().to_string()),
        Lit::Int(i) => Ok(format!("{sign}{}", i.base10_digits())),
        // Through `f64` so the text is JSON: Rust's `1.` and `1_0.5` are not.
        Lit::Float(f) => {
            let value = f.base10_parse::<f64>()?;
            if !value.is_finite() {
                return Err(Error::new_spanned(
                    f,
                    "a float `default` must be finite; JSON has no infinity",
                ));
            }
            Ok(format!("{sign}{value:?}"))
        }
        other => Err(Error::new_spanned(
            other,
            "`default` takes a string, integer, float or bool literal; use `default_json` \
             for anything else",
        )),
    }
}

/// A `default_json`'s text, refused here when it is not JSON, so the
/// generated code parses it infallibly.
fn json_text(lit: &LitStr) -> Result<String> {
    let text = lit.value();
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(_) => Ok(text),
        Err(e) => Err(Error::new_spanned(
            lit,
            format!("`default_json` is not JSON ({e}): {text}"),
        )),
    }
}

/// `s` as a JSON string literal.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn option<T: quote::ToTokens>(value: Option<T>) -> TokenStream2 {
    match value {
        Some(value) => quote!(::core::option::Option::Some(#value)),
        None => quote!(::core::option::Option::None),
    }
}

/// The expansion. `primary` is the primary output's field, and whether it is
/// an `Option<Output>`.
fn generate(
    name: &Ident,
    check: Option<&Path>,
    fields: &[Field<'_>],
    primary: Option<(&Ident, bool)>,
) -> TokenStream2 {
    let p = quote!(::dataflow_rs::function::__private);
    let keys: Vec<&str> = fields.iter().map(|f| f.key.as_str()).collect();
    let vars: Vec<Ident> = (0..fields.len()).map(|i| format_ident!("__f{i}")).collect();
    let idents = fields.iter().map(|f| f.ident);

    let compiles = fields.iter().zip(&vars).map(|(f, var)| {
        let (ty, key, output) = (f.ty, &f.key, f.output);
        let default = option(f.default.as_deref());
        let site = format!("{}::{}", name.unraw(), f.ident.unraw());
        let helper = if f.serde {
            quote!(serde_field)
        } else {
            quote!(field)
        };
        quote! {
            let #var = #p::#helper::<#ty>(
                __object,
                #p::Key::new(#key, #default, #site, #output),
                cx,
            );
        }
    });
    let assemble = if fields.is_empty() {
        quote!()
    } else {
        quote! {
            let (#(::core::option::Option::Some(#vars),)*) = (#(#vars,)*) else {
                return ::core::option::Option::None;
            };
        }
    };
    let check = check.map(|check| quote!(#check(&__out, cx);));

    let visits: Vec<TokenStream2> = fields
        .iter()
        .filter(|f| !f.serde)
        .map(|f| {
            let (ident, key, records) = (f.ident, &f.key, f.records);
            quote! {
                v.at(#key, |v| v.recording(#records, |v| {
                    #p::ConfigField::visit(&self.#ident, v);
                }));
            }
        })
        .collect();
    let v = if visits.is_empty() {
        quote!(_v)
    } else {
        quote!(v)
    };

    let specs = fields.iter().map(|f| {
        let (ty, key, records, primary) = (f.ty, &f.key, f.records, f.primary);
        let kind = if f.serde {
            quote!(#p::FieldKind::Literal(#p::ValueKind::Any))
        } else {
            quote!(<#ty as #p::ConfigField>::kind())
        };
        let required = if f.default.is_some() {
            quote!(false)
        } else if f.serde {
            quote!(#p::serde_required::<#ty>())
        } else {
            quote!(<#ty as #p::ConfigField>::absent().is_none())
        };
        let default = option(f.default.as_deref());
        let description = option(f.description.as_deref());
        quote! {
            #p::spec(#key, #kind, #required, #records, #default, #primary, #description)
        }
    });

    // Every field in declaration order: a `serde` one through `Serialize`,
    // spanned at its type so a missing impl is reported there.
    let resolves: Vec<TokenStream2> = fields
        .iter()
        .map(|f| {
            let (ident, key) = (f.ident, &f.key);
            if f.serde {
                quote_spanned! {f.ty.span()=>
                    #p::resolve_serde(&mut __entries, #key, &self.#ident)?;
                }
            } else {
                quote! {
                    #p::resolve_field(&mut __entries, #key, &self.#ident, r)?;
                }
            }
        })
        .collect();
    let r = if fields.iter().any(|f| !f.serde) {
        quote!(r)
    } else {
        quote!(_r)
    };
    let capacity = fields.len();
    let resolve = if fields.is_empty() {
        quote!(::core::result::Result::Ok(#p::OwnedDataValue::Object(
            ::std::vec::Vec::new()
        )))
    } else {
        quote! {
            let mut __entries: #p::Entries = ::std::vec::Vec::with_capacity(#capacity);
            #(#resolves)*
            ::core::result::Result::Ok(#p::OwnedDataValue::Object(__entries))
        }
    };

    let primary_output = primary.map(|(ident, optional)| {
        let output = if optional {
            quote!(::core::option::Option::as_ref(&self.#ident))
        } else {
            quote!(::core::option::Option::Some(&self.#ident))
        };
        quote! {
            fn primary_output(&self) -> ::core::option::Option<&#p::Output> {
                #output
            }
        }
    });

    quote! {
        #[automatically_derived]
        impl #p::FunctionConfig for #name {
            fn compile(
                input: &#p::Value,
                cx: &mut #p::CompileCx<'_>,
            ) -> ::core::option::Option<Self> {
                const KNOWN: &[&str] = &[#(#keys),*];
                let __rejected_before = cx.rejected();
                let __object = #p::object(input, cx)?;
                #p::unknown_keys(__object, KNOWN, cx);
                #(#compiles)*
                #assemble
                let __out = Self { #(#idents: #vars,)* };
                #check
                #p::finish(__out, __rejected_before, cx)
            }

            fn visit(&self, v: &mut dyn #p::ExprVisitor) {
                let mut v = #p::Visit::new(v, #p::AuthoredPath::root());
                <Self as #p::ConfigField>::visit(self, &mut v);
            }

            fn fields() -> &'static [#p::FieldSpec] {
                static FIELDS: #p::OnceLock<::std::vec::Vec<#p::FieldSpec>> =
                    #p::OnceLock::new();
                FIELDS.get_or_init(|| ::std::vec![#(#specs),*])
            }

            #primary_output

            fn resolve(
                &self,
                #r: &mut dyn #p::Resolver,
            ) -> #p::Result<#p::OwnedDataValue> {
                #resolve
            }
        }

        #[automatically_derived]
        impl #p::ConfigField for #name {
            fn compile(
                raw: &#p::Value,
                cx: &mut #p::CompileCx<'_>,
            ) -> ::core::option::Option<Self> {
                <Self as #p::FunctionConfig>::compile(raw, cx)
            }

            fn visit(&self, #v: &mut #p::Visit<'_>) {
                #(#visits)*
            }

            fn resolve(
                &self,
                r: &mut dyn #p::Resolver,
            ) -> #p::Result<::core::option::Option<#p::OwnedDataValue>> {
                ::core::result::Result::map(
                    <Self as #p::FunctionConfig>::resolve(self, r),
                    ::core::option::Option::Some,
                )
            }

            fn kind() -> #p::FieldKind {
                #p::FieldKind::Nested(<Self as #p::FunctionConfig>::fields)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the derive says about `input`, which it must refuse.
    fn refused(input: TokenStream2) -> String {
        let input: DeriveInput = syn::parse2(input).expect("a struct");
        expand(&input).expect_err("refused").to_string()
    }

    fn expands(input: TokenStream2) -> bool {
        let input: DeriveInput = syn::parse2(input).expect("a struct");
        expand(&input).is_ok()
    }

    #[test]
    fn a_primary_output_of_another_type_is_an_expansion_error() {
        // Marked primary.
        assert_eq!(
            refused(quote! {
                struct Many {
                    #[output(primary)]
                    many: Vec<Output>,
                    other: Output,
                }
            }),
            "the primary output `many` must be an `Output` or an `Option<Output>`"
        );
        // The lone output, primary without saying so.
        assert_eq!(
            refused(quote! {
                struct Keyed {
                    #[output(default = "data.k")]
                    r#keyed: std::collections::BTreeMap<String, Output>,
                }
            }),
            "the primary output `keyed` must be an `Output` or an `Option<Output>`"
        );
        // An `Output`, however it is spelled, and an `Option` of one expand.
        assert!(expands(quote! {
            struct Spelled {
                #[output(primary)]
                a: ::dataflow_rs::expr::Output,
                b: Output,
            }
        }));
        assert!(expands(quote! {
            struct Optional {
                #[output(primary)]
                a: core::option::Option<Output>,
                b: Output,
            }
        }));
    }

    #[test]
    fn an_option_of_an_output_counts_as_an_output() {
        // Beside an `Output`, unmarked: two outputs, neither primary.
        assert_eq!(
            refused(quote! {
                struct Two {
                    out: Output,
                    maybe: Option<Output>,
                }
            }),
            "`Two` has 2 outputs: mark exactly one of them `#[output(primary)]`"
        );
        // Alone, unmarked: the primary.
        assert!(expands(quote! {
            struct Lone {
                url: Expr<String>,
                maybe: Option<Output>,
            }
        }));
        // A `#[config(serde)]` field is read with serde, never an output.
        assert!(expands(quote! {
            struct Serde {
                out: Output,
                #[config(serde)]
                maybe: Option<Output>,
            }
        }));
    }
}
