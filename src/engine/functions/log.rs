use crate::engine::error::Result;
use crate::engine::executor::with_arena;
use crate::engine::message::{Change, Message};
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

    /// Cache index for the compiled message expression
    #[serde(skip)]
    pub message_index: Option<usize>,

    /// Cache indices for the compiled field expressions
    #[serde(skip)]
    pub field_indices: Vec<(String, Option<usize>)>,
}

impl LogConfig {
    /// Execute the log function. Always returns Ok((200, [])).
    pub fn execute(
        &self,
        message: &mut Message,
        engine: &Arc<Engine>,
        logic_cache: &[Arc<Logic>],
    ) -> Result<(usize, Vec<Change>)> {
        // Log is read-only across the entire task call: convert the context
        // to its arena form once, then evaluate the message expression plus
        // every structured field against the same shared `ctx_av`.
        let (log_message, field_parts) = with_arena(|arena| {
            let ctx_av: DataValue<'_> = message.context.to_arena(arena);

            // Stringify a single eval result. Avoids `to_owned()` on the
            // borrowed arena result: for the common String case we just
            // copy the str, for everything else we use the JSON emitter
            // which writes straight from `&DataValue<'_>`.
            let stringify = |idx: usize| -> String {
                match engine.evaluate(&logic_cache[idx], ctx_av, arena) {
                    Ok(DataValue::String(s)) => (*s).to_string(),
                    Ok(other) => other.to_string(),
                    Err(e) => {
                        error!("Log: Failed to evaluate expression: {:?}", e);
                        "<eval error>".to_string()
                    }
                }
            };

            let log_message = match self.message_index {
                Some(idx) if idx < logic_cache.len() => stringify(idx),
                _ => "<uncompiled message>".to_string(),
            };

            let mut field_parts = Vec::with_capacity(self.field_indices.len());
            for (key, idx_opt) in &self.field_indices {
                let val = match idx_opt {
                    Some(idx) if *idx < logic_cache.len() => stringify(*idx),
                    _ => "<uncompiled>".to_string(),
                };
                field_parts.push(format!("{}={}", key, val));
            }

            (log_message, field_parts)
        });

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
        Ok((200, vec![]))
    }
}
