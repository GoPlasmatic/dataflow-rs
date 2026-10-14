//! `EngineBuilder`: an engine from definitions, for a host that does not
//! quarantine (I68).

use super::engine::{Engine, EngineConfig};
use crate::base::Issue;
use crate::def::{ParseOptions, WorkflowDef, parse};
use crate::function::{Function, Registry};
use crate::program::CompileOptions;
use serde_json::Value;
use std::fmt;

/// A definition given to the builder: parsed, with the issues its parse
/// reported, which a rejection keeps, or refused by `parse` with its issues.
enum Pending {
    Parsed(Box<WorkflowDef>, Vec<Issue>),
    Refused(Vec<Issue>),
}

/// Builds an [`Engine`] from definitions: functions registered on a
/// [`Registry`], the workflows, and the config. [`build`](Self::build) fails
/// if any workflow is rejected, with every rejected definition's issues.
///
/// A host that quarantines, and runs the workflows that compile, uses a
/// [`Compiler`](crate::program::Compiler) instead.
///
/// ```
/// # extern crate dataflow_rs_v4 as dataflow_rs;
/// use dataflow_rs::Engine;
/// use dataflow_rs::program::IssueCode;
/// use serde_json::json;
///
/// let issues = Engine::builder()
///     .workflow_json(&json!({"id": "w", "tasks": [
///         {"id": "t", "function": {"name": "fetch"}}
///     ]}))
///     .build()
///     .unwrap_err();
/// assert_eq!(issues[0].code, IssueCode::UnknownFunction);
/// ```
pub struct EngineBuilder {
    registry: Registry,
    options: CompileOptions,
    config: EngineConfig,
    pending: Vec<Pending>,
}

impl EngineBuilder {
    /// A builder over `Registry::default()`, with the default options and
    /// config and no workflows.
    pub(crate) fn new() -> Self {
        Self {
            registry: Registry::default(),
            options: CompileOptions::default(),
            config: EngineConfig::default(),
            pending: Vec::new(),
        }
    }

    /// Replaces the registry, and every function registered so far with it.
    #[must_use]
    pub fn registry(mut self, registry: Registry) -> Self {
        self.registry = registry;
        self
    }

    /// Registers `f` under `name` on the registry, as
    /// [`Registry::register`], whose panics it shares.
    #[must_use]
    pub fn register<F: Function>(mut self, name: &str, f: F) -> Self {
        self.registry.register(name, f);
        self
    }

    /// Sets the options the workflows compile with.
    #[must_use]
    pub fn options(mut self, options: CompileOptions) -> Self {
        self.options = options;
        self
    }

    /// Sets the engine's config.
    #[must_use]
    pub fn config(mut self, config: EngineConfig) -> Self {
        self.config = config;
        self
    }

    /// Adds a parsed definition.
    #[must_use]
    pub fn workflow(mut self, def: WorkflowDef) -> Self {
        self.pending
            .push(Pending::Parsed(Box::new(def), Vec::new()));
        self
    }

    /// Adds a definition to parse with [`ParseOptions::default`], the compat
    /// reader on. One `parse` refuses makes [`build`](Self::build) fail with
    /// its parse issues, and one it reads but the compile rejects with its
    /// parse issues, then its compile issues.
    #[must_use]
    pub fn workflow_json(mut self, raw: &Value) -> Self {
        let parsed = parse(raw, ParseOptions::default());
        self.pending.push(match parsed.def {
            Some(def) => Pending::Parsed(Box::new(def), parsed.issues),
            None => Pending::Refused(parsed.issues),
        });
        self
    }

    /// Compiles every definition and links them into the engine's program.
    ///
    /// # Errors
    ///
    /// Every issue of every definition that `parse` or
    /// [`compile_one`](crate::program::Compiler::compile_one) rejected, in
    /// input order (I68), a rejected definition's parse issues before its
    /// compile issues. On success the other issues are dropped: a host that
    /// wants them calls [`Compiler::check`](crate::program::Compiler::check).
    pub fn build(self) -> std::result::Result<Engine, Vec<Issue>> {
        let compiler = self.registry.compiler(self.options);
        let mut workflows = Vec::with_capacity(self.pending.len());
        let mut rejected = Vec::new();
        for pending in self.pending {
            match pending {
                Pending::Parsed(def, mut parsed) => match compiler.compile_one(&def) {
                    Ok(compiled) => workflows.push(compiled.workflow),
                    Err(mut issues) => {
                        rejected.append(&mut parsed);
                        rejected.append(&mut issues);
                    }
                },
                Pending::Refused(mut issues) => rejected.append(&mut issues),
            }
        }
        if !rejected.is_empty() {
            return Err(rejected);
        }
        Ok(Engine::new(compiler.link(workflows), self.config))
    }
}

impl fmt::Debug for EngineBuilder {
    /// The registry, the options, the config and how many definitions it
    /// holds.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineBuilder")
            .field("registry", &self.registry)
            .field("options", &self.options)
            .field("config", &self.config)
            .field("workflows", &self.pending.len())
            .finish()
    }
}
