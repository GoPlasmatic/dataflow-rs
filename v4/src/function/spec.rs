//! `FunctionSpec`: what a function says about itself, as data.

use crate::expr::FieldSpec;
use std::borrow::Cow;

/// Is running a function twice the same as running it once?
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetrySafety {
    /// A repeat has the same effect as one run.
    Idempotent,
    /// A repeat has its effect again.
    NotIdempotent,
    /// Nothing is claimed.
    #[default]
    Unknown,
}

/// A config field that names another definition, such as a connector.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Reference {
    /// The config field holding the name.
    pub field: Cow<'static, str>,
    /// What it names, such as `connector`.
    pub kind: Cow<'static, str>,
}

impl Reference {
    /// A reference held in `field`, naming a `kind`.
    pub fn new(field: impl Into<Cow<'static, str>>, kind: impl Into<Cow<'static, str>>) -> Self {
        Self {
            field: field.into(),
            kind: kind.into(),
        }
    }
}

/// What a function says about itself, for tooling. Data only.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct FunctionSpec {
    /// What the function does.
    pub description: Cow<'static, str>,
    /// The group it is listed under.
    pub category: Cow<'static, str>,
    /// Whether a retry is safe.
    pub retry: RetrySafety,
    /// The config fields that name other definitions.
    pub references: Vec<Reference>,
    /// The config's fields, when a plugin declares them at runtime.
    pub runtime_fields: Option<Vec<FieldSpec>>,
}

impl FunctionSpec {
    /// Sets the description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<Cow<'static, str>>) -> Self {
        self.description = description.into();
        self
    }

    /// Sets the category.
    #[must_use]
    pub fn with_category(mut self, category: impl Into<Cow<'static, str>>) -> Self {
        self.category = category.into();
        self
    }

    /// Sets the retry safety.
    #[must_use]
    pub fn with_retry(mut self, retry: RetrySafety) -> Self {
        self.retry = retry;
        self
    }

    /// Sets the references.
    #[must_use]
    pub fn with_references(mut self, references: Vec<Reference>) -> Self {
        self.references = references;
        self
    }

    /// Sets the runtime-declared fields.
    #[must_use]
    pub fn with_runtime_fields(mut self, fields: Vec<FieldSpec>) -> Self {
        self.runtime_fields = Some(fields);
        self
    }
}
