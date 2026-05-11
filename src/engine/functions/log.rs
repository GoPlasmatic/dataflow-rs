use crate::engine::error::Result;
use crate::engine::executor::eval_to_json;
use crate::engine::message::{Change, Message};
use datalogic_rs::{Engine, Logic};
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
        let context_arc = message.get_context_arc();

        // Evaluate message expression
        let log_message = match self.message_index {
            Some(idx) if idx < logic_cache.len() => {
                match eval_to_json(engine, &logic_cache[idx], &context_arc) {
                    Ok(Value::String(s)) => s,
                    Ok(other) => other.to_string(),
                    Err(e) => {
                        error!("Log: Failed to evaluate message expression: {:?}", e);
                        "<message eval error>".to_string()
                    }
                }
            }
            _ => "<uncompiled message>".to_string(),
        };

        // Evaluate field expressions
        let mut field_parts = Vec::new();
        for (key, idx_opt) in &self.field_indices {
            let val = match idx_opt {
                Some(idx) if *idx < logic_cache.len() => {
                    match eval_to_json(engine, &logic_cache[*idx], &context_arc) {
                        Ok(Value::String(s)) => s,
                        Ok(v) => v.to_string(),
                        Err(_) => "<eval error>".to_string(),
                    }
                }
                _ => "<uncompiled>".to_string(),
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
        Ok((200, vec![]))
    }
}
