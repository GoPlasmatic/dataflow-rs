//! What several topics share.

pub(crate) mod corpus_driver;
pub(crate) mod golden;
pub(crate) mod run_helpers;

use dataflow_rs::function::FunctionConfig;
use dataflow_rs::run::EngineBuilder;
use dataflow_rs::{Ctx, Engine, Error, Expr, Function, Message, Outcome, Result};
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// An engine over `workflows` with no custom host functions, which must build.
pub(crate) fn engine(workflows: &[Value]) -> Engine {
    engine_with(workflows, |b| b)
}

/// An engine over `workflows` configured by `configure`, which must build.
pub(crate) fn engine_with(
    workflows: &[Value],
    configure: impl FnOnce(EngineBuilder) -> EngineBuilder,
) -> Engine {
    run_helpers::build_with(workflows, configure)
        .unwrap_or_else(|issues| panic!("the engine builds: {issues}"))
}

/// A message whose `data` is `data` and id is `"m"`.
pub(crate) fn message(data: Value) -> Message {
    Message::from_parts(
        OwnedDataValue::Null,
        OwnedDataValue::from(&data),
        OwnedDataValue::Null,
    )
    .with_id("m")
}

/// A `map` task `id` writing `logic` at `path`.
pub(crate) fn map(id: &str, path: &str, logic: Value) -> Value {
    json!({"id": id, "function": {"name": "map", "input": {
        "mappings": [{"path": path, "logic": logic}]}}})
}

/// `node` with each `(key, value)` of `extra` set on it.
pub(crate) fn with(mut node: Value, extra: &[(&str, Value)]) -> Value {
    for (key, value) in extra {
        node[*key] = value.clone();
    }
    node
}

/// A workflow `id` over `tasks`, with each `(key, value)` of `extra` set on it.
pub(crate) fn workflow(id: &str, tasks: Vec<Value>, extra: &[(&str, Value)]) -> Value {
    with(json!({"id": id, "tasks": tasks}), extra)
}

/// `put`'s config (`trace` and `wire`).
#[derive(FunctionConfig)]
pub(crate) struct PutConfig {
    /// Written through `Ctx::set` in key order. A key's text after a space
    /// is a label, so one path can be written twice.
    pub(crate) writes: BTreeMap<String, Expr>,
    /// `"ok"`, `"skip"`, `"halt"`, `"err"` (a `PARTNER_DOWN` service error),
    /// or a status.
    pub(crate) outcome: Expr,
}

/// Writes, then returns what its config names.
pub(crate) struct Put;

impl Function for Put {
    type Config = PutConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &PutConfig) -> Result<Outcome> {
        tokio::task::yield_now().await;
        for (key, expr) in &cfg.writes {
            let path = key.split_once(' ').map_or(key.as_str(), |(path, _)| path);
            let value = ctx.eval(expr)?.into_owned();
            ctx.set(&path.parse()?, value)?;
        }
        let outcome = Value::from(&*ctx.eval(&cfg.outcome)?);
        match outcome.as_str() {
            Some("ok") => Ok(Outcome::Ok),
            Some("skip") => Ok(Outcome::Skip),
            Some("halt") => Ok(Outcome::Halt),
            Some("err") => Err(Error::service("PARTNER_DOWN", "down")),
            _ => {
                let status = outcome.as_u64().expect("an outcome is a word or a status");
                Ok(Outcome::Status(u16::try_from(status).unwrap()))
            }
        }
    }
}
