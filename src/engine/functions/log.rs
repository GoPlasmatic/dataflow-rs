use crate::engine::error::Result;
use crate::engine::executor::{ArenaContext, with_arena};
use crate::engine::message::{Change, Message};
use crate::engine::task_outcome::TaskOutcome;
use datalogic_rs::{Engine, Logic};
use datavalue::DataValue;
use log::{debug, error, info, trace, warn};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Log levels supported by the log function
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

/// Configuration for the log function.
///
/// The message and field expressions are pre-compiled at startup.
#[derive(Debug, Clone, Deserialize)]
pub struct LogConfig {
    /// Log level to emit at
    #[serde(default)]
    pub level: LogLevel,

    /// JSONLogic expression to produce the log message string
    pub message: Value,

    /// Additional structured fields: each value is a JSONLogic expression
    #[serde(default)]
    pub fields: HashMap<String, Value>,

    /// Pre-compiled `message` JSONLogic, populated by `LogicCompiler`.
    #[serde(skip)]
    pub compiled_message: Option<Arc<Logic>>,

    /// Pre-compiled JSONLogic for each `fields` entry, populated by
    /// `LogicCompiler`. The inner `Option` is `None` for fields whose logic
    /// failed to compile (logged at engine construction).
    #[serde(skip)]
    pub compiled_fields: Vec<(String, Option<Arc<Logic>>)>,
}

impl LogConfig {
    /// Execute the log function, opening a fresh thread-local arena scope.
    ///
    /// Use this entry point when calling `LogConfig` outside an existing
    /// `with_arena` scope (direct API users, tests). Inside a workflow sync
    /// stretch the dispatch goes through [`Self::execute_in_arena`] to reuse
    /// the cached `ArenaContext` and avoid a redundant `to_arena` walk.
    pub fn execute(
        &self,
        message: &mut Message,
        engine: &Arc<Engine>,
    ) -> Result<(TaskOutcome, Vec<Change>)> {
        with_arena(|arena| {
            let mut arena_ctx = ArenaContext::from_owned(&message.context, arena);
            self.execute_in_arena(message, &mut arena_ctx, engine)
        })
    }

    /// Execute against an externally-provided `ArenaContext` so the cached
    /// arena form of `message.context` (built once at the top of the workflow
    /// sync stretch) is reused across every JSONLogic eval performed here.
    pub(crate) fn execute_in_arena(
        &self,
        _message: &mut Message,
        arena_ctx: &mut ArenaContext<'_>,
        engine: &Arc<Engine>,
    ) -> Result<(TaskOutcome, Vec<Change>)> {
        let arena = arena_ctx.arena();
        let ctx_av = arena_ctx.as_data_value();

        // Stringify a single eval result. For the common String case we copy
        // the str directly; otherwise the JSON emitter writes straight from
        // `&DataValue<'_>` without an intermediate `to_owned()` deep clone.
        let stringify = |compiled: &Logic| -> String {
            match engine.evaluate(compiled, ctx_av, arena) {
                Ok(DataValue::String(s)) => (*s).to_string(),
                Ok(other) => other.to_string(),
                Err(e) => {
                    error!("Log: Failed to evaluate expression: {:?}", e);
                    "<eval error>".to_string()
                }
            }
        };

        let log_message = match &self.compiled_message {
            Some(compiled) => stringify(compiled),
            None => "<uncompiled message>".to_string(),
        };

        let mut field_parts = Vec::with_capacity(self.compiled_fields.len());
        for (key, compiled_opt) in &self.compiled_fields {
            let val = match compiled_opt {
                Some(compiled) => stringify(compiled),
                None => "<uncompiled>".to_string(),
            };
            field_parts.push(format!("{}={}", key, val));
        }

        let full_message = if field_parts.is_empty() {
            log_message
        } else {
            format!("{} [{}]", log_message, field_parts.join(", "))
        };

        match self.level {
            LogLevel::Trace => trace!(target: "dataflow::log", "{}", full_message),
            LogLevel::Debug => debug!(target: "dataflow::log", "{}", full_message),
            LogLevel::Info => info!(target: "dataflow::log", "{}", full_message),
            LogLevel::Warn => warn!(target: "dataflow::log", "{}", full_message),
            LogLevel::Error => error!(target: "dataflow::log", "{}", full_message),
        }

        // Log function never modifies message, never fails
        Ok((TaskOutcome::Success, vec![]))
    }
}
