//! The built-in functions' configs, and the dispatch over them.
//!
//! Crate-private (I42): code outside the crate reaches a built-in's config
//! through the `Compiler`, as `tests/map_vs_v3.rs` and the v4 corpus driver
//! do.
//!
//! What each config accepts is held to v3's build by the differential in
//! `tests.rs`. Each config's keys, and those of `map`'s mappings and
//! `validation`'s rules, are `def`'s tables for its input: the compat reader
//! moves any other key into the task's `ext` before a config compiles, as v3
//! ignored them (I54), and the tests hold the two to one set.

mod filter;
mod log;
mod map;
mod parse;
mod paths;
mod publish;
mod validation;

pub use filter::FilterConfig;
pub use log::LogConfig;
pub use map::MapConfig;
pub use parse::ParseConfig;
pub use publish::PublishConfig;
pub use validation::ValidationConfig;

use crate::expr::{CompileCx, ExprVisitor, FieldSpec};
use crate::function::{FunctionConfig, FunctionSpec};
use serde_json::Value;

/// A built-in function, by name (I42).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BuiltinKind {
    Map,
    Validation,
    Filter,
    Log,
    ParseJson,
    ParseXml,
    PublishJson,
    PublishXml,
}

impl BuiltinKind {
    /// Every built-in, in `def::BUILTIN_FUNCTIONS` order.
    pub(crate) const ALL: [Self; 8] = [
        Self::Map,
        Self::Validation,
        Self::Filter,
        Self::Log,
        Self::ParseJson,
        Self::ParseXml,
        Self::PublishJson,
        Self::PublishXml,
    ];

    /// The built-in called `name`. v3's alias `validate` is not one: the
    /// compat reader rewrites it before a name is looked up.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// Whether no host function may take `name`: a built-in's name, or
    /// `validate`, v3's alias for `validation`, which the compat reader
    /// rewrites before a name is looked up, so a host function under it
    /// could never be called.
    pub(crate) fn reserved(name: &str) -> bool {
        Self::from_name(name).is_some() || name == "validate"
    }

    /// The name a task calls it by.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Map => "map",
            Self::Validation => "validation",
            Self::Filter => "filter",
            Self::Log => "log",
            Self::ParseJson => "parse_json",
            Self::ParseXml => "parse_xml",
            Self::PublishJson => "publish_json",
            Self::PublishXml => "publish_xml",
        }
    }

    /// What the built-in says about itself: a one-sentence description and
    /// its category. Its retry safety is left `Unknown`.
    pub(crate) fn spec(self) -> FunctionSpec {
        let (category, description) = match self {
            Self::Map => (
                "transform",
                "Writes the results of JSONLogic to context paths, or removes them.",
            ),
            Self::Validation => (
                "validate",
                "Checks rules against the message and records an error for each that fails.",
            ),
            Self::Filter => (
                "control",
                "Halts the workflow, or skips the task, when its condition is not met.",
            ),
            Self::Log => (
                "observe",
                "Emits a log line, with structured fields, at a chosen level.",
            ),
            Self::ParseJson => (
                "parse",
                "Reads a JSON value from the payload or the context into `data`.",
            ),
            Self::ParseXml => (
                "parse",
                "Parses an XML string from the payload or the context into `data`.",
            ),
            Self::PublishJson => (
                "publish",
                "Serializes a value under `data` to a JSON string under `data`.",
            ),
            Self::PublishXml => (
                "publish",
                "Serializes a value under `data` to an XML string under `data`.",
            ),
        };
        FunctionSpec::default()
            .with_category(category)
            .with_description(description)
    }

    /// The config's fields: its derived `fields()`.
    pub(crate) fn fields(self) -> &'static [FieldSpec] {
        match self {
            Self::Map => MapConfig::fields(),
            Self::Validation => ValidationConfig::fields(),
            Self::Filter => FilterConfig::fields(),
            Self::Log => LogConfig::fields(),
            Self::ParseJson | Self::ParseXml => ParseConfig::fields(),
            Self::PublishJson | Self::PublishXml => PublishConfig::fields(),
        }
    }

    /// Compiles `input` as this built-in's config at `cx`. `None` when any
    /// issue is Rejected.
    pub(crate) fn compile(self, input: &Value, cx: &mut CompileCx<'_>) -> Option<Builtin> {
        match self {
            Self::Map => MapConfig::compile(input, cx).map(Builtin::Map),
            Self::Validation => ValidationConfig::compile(input, cx).map(Builtin::Validation),
            Self::Filter => FilterConfig::compile(input, cx).map(Builtin::Filter),
            Self::Log => LogConfig::compile(input, cx).map(Builtin::Log),
            Self::ParseJson => ParseConfig::compile(input, cx).map(Builtin::ParseJson),
            Self::ParseXml => ParseConfig::compile(input, cx).map(Builtin::ParseXml),
            Self::PublishJson => PublishConfig::compile(input, cx).map(Builtin::PublishJson),
            Self::PublishXml => PublishConfig::compile(input, cx).map(Builtin::PublishXml),
        }
    }
}

/// A built-in call site's callee: its typed config, unboxed, which is what
/// the runtime matches on (I42).
#[derive(Debug)]
pub(crate) enum Builtin {
    Map(MapConfig),
    Validation(ValidationConfig),
    Filter(FilterConfig),
    Log(LogConfig),
    ParseJson(ParseConfig),
    ParseXml(ParseConfig),
    PublishJson(PublishConfig),
    PublishXml(PublishConfig),
}

impl Builtin {
    /// Which built-in this is.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "only the tests ask a callee which built-in it is")
    )]
    pub(crate) fn kind(&self) -> BuiltinKind {
        match self {
            Self::Map(_) => BuiltinKind::Map,
            Self::Validation(_) => BuiltinKind::Validation,
            Self::Filter(_) => BuiltinKind::Filter,
            Self::Log(_) => BuiltinKind::Log,
            Self::ParseJson(_) => BuiltinKind::ParseJson,
            Self::ParseXml(_) => BuiltinKind::ParseXml,
            Self::PublishJson(_) => BuiltinKind::PublishJson,
            Self::PublishXml(_) => BuiltinKind::PublishXml,
        }
    }

    /// Walks the config's expressions; paths are relative to the config.
    pub(crate) fn visit(&self, v: &mut dyn ExprVisitor) {
        match self {
            Self::Map(c) => c.visit(v),
            Self::Validation(c) => c.visit(v),
            Self::Filter(c) => c.visit(v),
            Self::Log(c) => c.visit(v),
            Self::ParseJson(c) | Self::ParseXml(c) => c.visit(v),
            Self::PublishJson(c) | Self::PublishXml(c) => c.visit(v),
        }
    }
}

#[cfg(test)]
mod tests;
