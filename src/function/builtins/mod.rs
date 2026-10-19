//! The built-in functions' configs, and the dispatch over them.
//!
//! Crate-private: code outside the crate reaches a built-in's config
//! through the `Compiler`, as the integration tests do.
//!
//! What each config accepts and refuses, and where, is held by the rows in
//! `tests.rs`. Each config's keys, and those of `map`'s mappings and
//! `validation`'s rules, are `def`'s tables for its input: the compat reader
//! moves any other key into the task's `ext` before a config compiles, as v3
//! ignored them, and the tests hold the two to one set.

mod filter;
mod log;
mod map;
#[cfg(test)]
pub(crate) mod one_task;
mod parse;
mod paths;
mod publish;
mod validation;
mod xml;

pub(crate) use filter::FilterConfig;
pub(crate) use log::LogConfig;
pub(crate) use map::MapConfig;
pub(crate) use parse::ParseConfig;
pub(crate) use publish::PublishConfig;
pub(crate) use validation::ValidationConfig;

use crate::base::Result;
use crate::expr::{CompileCx, ExprVisitor, FieldSpec};
use crate::function::{FunctionConfig, FunctionSpec, Outcome, SyncCtx};
use serde_json::Value;

/// A built-in function, by name.
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

    /// The name a task calls it by: its row of `def::BUILTIN_FUNCTIONS`,
    /// which [`Self::ALL`] follows.
    pub(crate) fn name(self) -> &'static str {
        crate::def::BUILTIN_FUNCTIONS[self as usize]
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
            Self::Filter => FilterConfig::compile(input, cx).map(|c| Builtin::Filter(c.into())),
            Self::Log => LogConfig::compile(input, cx).map(|c| Builtin::Log(c.into())),
            Self::ParseJson => {
                ParseConfig::compile(input, cx).map(|c| Builtin::ParseJson(c.into()))
            }
            Self::ParseXml => ParseConfig::compile(input, cx).map(|c| Builtin::ParseXml(c.into())),
            Self::PublishJson => {
                PublishConfig::compile_for(input, false, cx).map(|c| Builtin::PublishJson(c.into()))
            }
            Self::PublishXml => {
                PublishConfig::compile_for(input, true, cx).map(|c| Builtin::PublishXml(c.into()))
            }
        }
    }
}

/// A built-in call site's callee: its typed config, which is what the
/// runtime matches on, with no trait object and no virtual call. A config
/// holding an expression inline (`filter`, `log`, parse and publish) is
/// boxed, so every call site stays small; `map` and `validation` hold only
/// their lists.
#[derive(Debug)]
pub(crate) enum Builtin {
    Map(MapConfig),
    Validation(ValidationConfig),
    Filter(Box<FilterConfig>),
    Log(Box<LogConfig>),
    ParseJson(Box<ParseConfig>),
    ParseXml(Box<ParseConfig>),
    PublishJson(Box<PublishConfig>),
    PublishXml(Box<PublishConfig>),
}

impl Builtin {
    /// Which built-in this is.
    #[cfg(test)]
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

    /// One call: a match over the eight configs.
    pub(crate) fn call<'p>(&'p self, cx: &mut SyncCtx<'_, '_, 'p>) -> Result<Outcome> {
        match self {
            Self::Map(c) => c.call(cx),
            Self::Validation(c) => c.call(cx),
            Self::Filter(c) => c.call(cx),
            Self::Log(c) => c.call(cx),
            Self::ParseJson(c) => c.json(cx),
            Self::ParseXml(c) => c.xml(cx),
            Self::PublishJson(c) => c.json(cx),
            Self::PublishXml(c) => c.xml(cx),
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
