use crate::engine::error::Result;
use crate::engine::executor::eval_to_owned;
use crate::engine::message::{Change, Message};
use datalogic_rs::{Engine, Logic};
use datavalue::OwnedDataValue;
use log::{debug, info};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// Status code: filter condition passed, continue normally
pub const FILTER_STATUS_PASS: usize = 200;
/// Status code: skip this task, continue with next task
pub const FILTER_STATUS_SKIP: usize = 298;
/// Status code: halt the current workflow, no further tasks execute
pub const FILTER_STATUS_HALT: usize = 299;

/// What to do when the filter condition evaluates to false
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RejectAction {
    /// Halt the entire workflow — no further tasks in this workflow execute
    #[default]
    Halt,
    /// Skip only this task — continue with next task in the workflow
    Skip,
}

/// Configuration for the filter/gate function
#[derive(Debug, Clone, Deserialize)]
pub struct FilterConfig {
    /// JSONLogic condition to evaluate against the message context.
    /// If true, the message passes through. If false, the on_reject action is taken.
    pub condition: Value,

    /// What to do when the condition is false
    #[serde(default)]
    pub on_reject: RejectAction,

    /// Cache index for the compiled condition
    #[serde(skip)]
    pub condition_index: Option<usize>,
}

impl FilterConfig {
    /// Execute the filter function.
    ///
    /// Returns status 200 if condition passes, 299 for halt, 298 for skip.
    pub fn execute(
        &self,
        message: &mut Message,
        engine: &Arc<Engine>,
        logic_cache: &[Arc<Logic>]) -> Result<(usize, Vec<Change>)> {
        let condition_met = match self.condition_index {
            Some(idx) if idx < logic_cache.len() => {
                match eval_to_owned(engine, &logic_cache[idx], &message.context) {
                    Ok(OwnedDataValue::Bool(true)) => true,
                    Ok(_) => false,
                    Err(e) => {
                        debug!("Filter: condition evaluation error: {:?}", e);
                        false
                    }
                }
            }
            _ => {
                debug!("Filter: condition not compiled, treating as not met");
                false
            }
        };

        if condition_met {
            debug!("Filter: condition passed");
            Ok((FILTER_STATUS_PASS, vec![]))
        } else {
            match self.on_reject {
                RejectAction::Halt => {
                    info!("Filter: condition not met, halting workflow");
                    Ok((FILTER_STATUS_HALT, vec![]))
                }
                RejectAction::Skip => {
                    debug!("Filter: condition not met, skipping");
                    Ok((FILTER_STATUS_SKIP, vec![]))
                }
            }
        }
    }
}
