use crate::engine::error::Result;
use crate::engine::executor::{ArenaContext, with_arena};
use crate::engine::message::{Change, Message};
use datalogic_rs::{Engine, Logic};
use datavalue::DataValue;
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

    /// Pre-compiled JSONLogic, populated by `LogicCompiler`. `None` is
    /// treated as "condition not met" (same fallback as before).
    #[serde(skip)]
    pub compiled_condition: Option<Arc<Logic>>,
}

impl FilterConfig {
    /// Execute the filter function, opening a fresh thread-local arena scope.
    ///
    /// Use this entry point when calling `FilterConfig` outside an existing
    /// `with_arena` scope. Inside a workflow sync stretch the dispatch goes
    /// through [`Self::execute_in_arena`] to reuse the cached arena form of
    /// `message.context` and avoid a redundant `to_arena` deep walk.
    ///
    /// Returns status 200 if condition passes, 299 for halt, 298 for skip.
    pub fn execute(
        &self,
        message: &mut Message,
        engine: &Arc<Engine>,
    ) -> Result<(usize, Vec<Change>)> {
        with_arena(|arena| {
            let mut arena_ctx = ArenaContext::from_owned(&message.context, arena);
            self.execute_in_arena(message, &mut arena_ctx, engine)
        })
    }

    /// Execute against an externally-provided `ArenaContext` so the cached
    /// arena form of `message.context` is reused. Eliminates the inner
    /// `with_arena`/`eval_to_owned` call that would re-borrow the
    /// thread-local arena `RefCell` (panic) when invoked from inside the
    /// sync stretch.
    pub(crate) fn execute_in_arena(
        &self,
        _message: &mut Message,
        arena_ctx: &mut ArenaContext<'_>,
        engine: &Arc<Engine>,
    ) -> Result<(usize, Vec<Change>)> {
        let condition_met = match &self.compiled_condition {
            Some(compiled) => {
                let ctx_av = arena_ctx.as_data_value();
                match engine.evaluate(compiled, ctx_av, arena_ctx.arena()) {
                    Ok(DataValue::Bool(true)) => true,
                    Ok(_) => false,
                    Err(e) => {
                        debug!("Filter: condition evaluation error: {:?}", e);
                        false
                    }
                }
            }
            None => {
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
