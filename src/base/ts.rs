//! The `ts` feature's hand-written declarations.
//!
//! A wire shape whose `Serialize` is derived derives its TypeScript through
//! ts-rs. One whose `Serialize` is written by hand declares it through
//! [`ts_decl!`], in the format ts-rs's derive writes, so the exporter in
//! `tests/ts_types.rs` renders both alike. A `JsonValue` is the exporter's own
//! declaration: `OwnedDataValue` is datavalue's type, which cannot implement
//! ts-rs's trait here.

/// The JSDoc comment ts-rs writes for doc lines `lines`, each starting with
/// the space a `///` comment's line does; empty for none.
pub(crate) fn jsdoc(lines: &[&str]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::from("/**\n");
    for line in lines {
        out.push_str(" *");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(" */\n");
    out
}

/// One property of an object declared by hand.
pub(crate) struct Prop {
    /// The key.
    pub(crate) key: &'static str,
    /// The serializer may leave the key out.
    pub(crate) optional: bool,
    /// The value's TypeScript type.
    pub(crate) ty: String,
    /// The property's doc lines, as [`jsdoc`] takes them.
    pub(crate) doc: &'static [&'static str],
}

impl Prop {
    /// A key the serializer always writes.
    pub(crate) fn new(
        key: &'static str,
        ty: impl Into<String>,
        doc: &'static [&'static str],
    ) -> Self {
        Self {
            key,
            optional: false,
            ty: ty.into(),
            doc,
        }
    }

    /// A key the serializer may leave out.
    pub(crate) fn optional(
        key: &'static str,
        ty: impl Into<String>,
        doc: &'static [&'static str],
    ) -> Self {
        Self {
            optional: true,
            ..Self::new(key, ty, doc)
        }
    }
}

/// An object type of `props`, in order, as ts-rs's derive writes one.
pub(crate) fn object(props: &[Prop]) -> String {
    let fields: Vec<String> = props
        .iter()
        .map(|p| {
            let doc = match p.doc {
                [] => String::new(),
                doc => format!("\n{}", jsdoc(doc)),
            };
            let mark = if p.optional { "?" } else { "" };
            format!("{doc}{}{mark}: {},", p.key, p.ty)
        })
        .collect();
    format!("{{ {} }}", fields.join(" "))
}

/// A union of the string literals `names`, as ts-rs's derive writes one for
/// an enum of unit variants.
pub(crate) fn literals(names: impl IntoIterator<Item = impl std::fmt::Display>) -> String {
    names
        .into_iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Implements `ts_rs::TS` for `$ty`, declared as TypeScript type `$name`:
/// documented by the doc lines `$doc`, and defined by `$inline`, an
/// expression of the `&ts_rs::Config` it binds to `$cfg`.
macro_rules! ts_decl {
    ($ty:ty, $name:literal, [$($doc:literal),* $(,)?], |$cfg:ident| $inline:expr) => {
        impl ts_rs::TS for $ty {
            type WithoutGenerics = Self;
            type OptionInnerType = Self;

            fn docs() -> Option<String> {
                Some($crate::base::ts::jsdoc(&[$($doc),*]))
            }

            fn ident(_: &ts_rs::Config) -> String {
                $name.to_owned()
            }

            fn name(_: &ts_rs::Config) -> String {
                $name.to_owned()
            }

            fn inline($cfg: &ts_rs::Config) -> String {
                $inline
            }

            fn decl(cfg: &ts_rs::Config) -> String {
                format!("type {} = {};", $name, <Self as ts_rs::TS>::inline(cfg))
            }

            fn decl_concrete(cfg: &ts_rs::Config) -> String {
                <Self as ts_rs::TS>::decl(cfg)
            }

            fn output_path() -> Option<std::path::PathBuf> {
                Some(std::path::PathBuf::from(concat!($name, ".ts")))
            }
        }
    };
}

pub(crate) use ts_decl;
