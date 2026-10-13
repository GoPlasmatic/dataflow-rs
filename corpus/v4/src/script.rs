//! The corpus's one host function, v3's `script` (`corpus/v3/src/script.rs`)
//! on v4's `Function`. Its config is v3's input, compiled: a case's `script`
//! tasks compile here exactly as they will run.
//!
//! Phase 3 compiles definitions and runs nothing, so `call` is unreachable.
//! Phase 5 ports v3's behaviour: yield `yields` times, write the identity to
//! `identity`, apply `set`, push `errors`, then evaluate `outcome`.

use dataflow_rs::function::FunctionConfig;
use dataflow_rs::{Ctx, Expr, Function, Outcome, Path, Result};

/// `script`: everything it does is in its config.
pub struct Script;

/// `script`'s input. An unknown key is refused, as every v4 config refuses
/// one: break 2a.
#[derive(FunctionConfig)]
pub struct ScriptConfig {
    /// The writes, in order.
    #[config(default_json = "[]")]
    pub set: Vec<Write>,
    /// The errors to add, in order.
    #[config(default_json = "[]")]
    pub errors: Vec<AddError>,
    /// Where to write the call's identity: workflow, task, loop counter and
    /// element index.
    pub identity: Option<Expr<Path>>,
    /// How many times to yield before anything else.
    pub yields: Option<Expr<u64>>,
    /// The outcome, evaluated last: `"success"`, `"skip"`, `"halt"`,
    /// `{"status": n}` or `{"error": {...}}`. Absent or `null` is success.
    pub outcome: Option<Expr>,
}

/// One write to `path`: of `value` (evaluated), of what `get` reads, or of
/// `raw` (never evaluated). Exactly one of the three is set.
#[derive(FunctionConfig)]
pub struct Write {
    /// Where the value is written.
    pub path: Expr<Path>,
    /// An expression, evaluated.
    pub value: Option<Expr>,
    /// A path, read; `null` when absent.
    pub get: Option<Expr<Path>>,
    /// The config value itself.
    pub raw: Option<serde_json::Value>,
}

/// One error the call adds to the message.
#[derive(FunctionConfig)]
pub struct AddError {
    /// The error's code.
    pub code: String,
    /// The error's message.
    pub message: String,
}

impl Function for Script {
    type Config = ScriptConfig;

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &ScriptConfig) -> Result<Outcome> {
        unreachable!("phase 3 compiles definitions only: no Ctx exists")
    }
}
