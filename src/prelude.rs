//! Common imports for building dataflow-rs engines and handlers.
//!
//! `use dataflow_rs::prelude::*;` brings in the types you need for the 90%
//! case: engine construction, custom handler authoring, message
//! processing, error handling.
//!
//! ```rust,no_run
//! use dataflow_rs::prelude::*;
//! use serde_json::json;
//!
//! # async fn run() -> Result<()> {
//! let engine = Engine::builder()
//!     // .register("my_handler", MyHandler)
//!     .build()?;
//!
//! let mut message = Message::builder()
//!     .payload_json(&json!({"order": {"total": 1500}}))
//!     .build();
//!
//! engine.process_message(&mut message).await?;
//! # Ok(()) }
//! ```
//!
//! Types not re-exported here (because they're only needed for less-common
//! flows): `BoxedFunctionHandler`, `DynAsyncFunctionHandler`,
//! `CompiledCustomInput`, the named `*Config` structs (`MapConfig`,
//! `ValidationConfig`, etc.), the trace surface (`ExecutionTrace`,
//! `ExecutionStep`, `StepResult`), and the rules-engine aliases
//! (`Rule`, `Action`, `RulesEngine`). Reach into the crate root for those.

pub use crate::engine::error::{DataflowError, ErrorInfo, Result};
pub use crate::engine::functions::AsyncFunctionHandler;
pub use crate::engine::message::{AuditTrail, Change, Message, MessageBuilder};
pub use crate::engine::task_context::TaskContext;
pub use crate::engine::task_outcome::TaskOutcome;
pub use crate::engine::{Engine, EngineBuilder, Task, Workflow, WorkflowStatus};
