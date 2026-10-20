# Host Functions

The engine ships the eight [built-ins](./overview.md) and no I/O. Everything
else a workflow calls, an HTTP request, a database read, a message on a
queue, a model, is a **host function**: a `Function` the host registers under
a name, with a typed config the engine compiles once when it builds, and a
call the engine makes per task run. 3.x shipped typed configs for
`http_call`, `enrich` and `publish_kafka` without the functions behind them;
4.0 ships neither, and a host that used them registers its own under the same
names. See [Migrating from 3.x](../migration/from-3.md).

This page builds an `http_call` from scratch. To keep the page runnable
without a network, its function answers every request with a fixed response
that echoes the request; a real one holds an HTTP client in place of that.

## A function in three parts

A host function is a config struct, a `Function` implementation, and a
registration.

```rust
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::{CompileCx, Interpolated, Output};
use dataflow_rs::function::{FunctionConfig, FunctionSpec, Prepare, Reference, RetrySafety};
use dataflow_rs::program::{AuthoredPath, IssueCode, Severity};
use dataflow_rs::{Ctx, Error, Expr, Function, Issue, Outcome, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The `input` of an `http_call` task, compiled once when the engine builds.
#[derive(FunctionConfig)]
#[config(check = HttpCallConfig::rules)]
struct HttpCallConfig {
    /// The connector the request goes through, by name.
    connector: String,
    /// The HTTP method.
    #[config(default = "GET")]
    method: String,
    /// The request path.
    path: Expr<String>,
    /// Header values, each JSONLogic.
    #[config(default_json = "{}")]
    headers: BTreeMap<String, Expr<String>>,
    /// The request body: data with `{"var": ...}` holes.
    body: Option<Interpolated>,
    /// The request timeout.
    #[config(default = 30000)]
    timeout_ms: Expr<u64>,
    /// Where the response is written.
    #[output(default = "data.response")]
    output: Output,
}

const METHODS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];

impl HttpCallConfig {
    /// A rule the field types alone cannot state, checked at build.
    fn rules(&self, cx: &mut CompileCx<'_>) {
        if !METHODS.contains(&self.method.as_str()) {
            cx.at("method", |cx| {
                let message = format!("`{}` is not one of {METHODS:?}", self.method);
                cx.issue(IssueCode::InputParse, message);
            });
        }
    }
}

/// Sends requests through named connectors. This one holds no client: it
/// answers every request with a fixed response describing the request.
struct HttpCall {
    /// Connector name to base URL.
    connectors: BTreeMap<String, String>,
}

impl HttpCall {
    fn new(connectors: &[(&str, &str)]) -> Self {
        Self {
            connectors: connectors
                .iter()
                .map(|(name, base)| (name.to_string(), base.to_string()))
                .collect(),
        }
    }
}

impl Function for HttpCall {
    type Config = HttpCallConfig;

    /// What the function says about itself, for tooling: read once, when it
    /// is registered.
    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default()
            .with_description("Sends an HTTP request through a named connector and writes the response.")
            .with_category("io")
            .with_retry(RetrySafety::Unknown)
            .with_references(vec![Reference::new("connector", "connector")])
    }

    /// A check the config alone cannot make: the connector must be one this
    /// deployment has. A Rejected issue refuses the task's workflow at build.
    fn prepare(
        &self,
        cfg: &mut HttpCallConfig,
        p: &Prepare<'_>,
    ) -> std::result::Result<(), Vec<Issue>> {
        if self.connectors.contains_key(&cfg.connector) {
            return Ok(());
        }
        let message = format!(
            "task `{}` names connector `{}`, which this deployment does not have",
            p.task_id(),
            cfg.connector
        );
        let issue = Issue::new(
            IssueCode::UnresolvedReference,
            AuthoredPath::root().join("connector"),
            message,
        )
        .with_severity(Severity::Rejected);
        Err(vec![issue])
    }

    /// One call: read the config against the message, do the work, write.
    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &HttpCallConfig) -> Result<Outcome> {
        let base = &self.connectors[&cfg.connector];
        let path = ctx.eval(&cfg.path)?;
        let timeout_ms = ctx.eval(&cfg.timeout_ms)?;
        let mut headers = BTreeMap::new();
        for (name, value) in &cfg.headers {
            headers.insert(name.clone(), ctx.eval(value)?.into_owned());
        }
        let body = match &cfg.body {
            Some(doc) => serde_json::to_value(ctx.render(doc))
                .map_err(|e| Error::service("BODY_ENCODING", e.to_string()))?,
            None => Value::Null,
        };

        // A real client sends `cfg.method` to `{base}{path}` with the headers
        // and the body here, and awaits the response within `timeout_ms`.
        // This one describes the request instead. It writes the header names
        // and not their values: a header may carry a secret, and what a
        // function writes, the engine records.
        let response = json!({
            "status": 200,
            "url": format!("{base}{path}"),
            "method": cfg.method,
            "headers": headers.keys().collect::<Vec<_>>(),
            "body": body,
            "timeout_ms": timeout_ms,
        });
        ctx.emit(&cfg.output, OwnedDataValue::from(&response))?;
        Ok(Outcome::Ok)
    }
}
```

### The config

`#[derive(FunctionConfig)]` applies to a struct with named fields. Each
field is compiled from the key of its name, and the field's type says what
position the key holds in the input:

| Field type | What the key holds | When it is read |
|------------|--------------------|-----------------|
| `String`, `bool`, `u64`, `i64`, `f64` (and the other integer widths), `serde_json::Value`, `OwnedDataValue` | A literal of that type | Never evaluated: `connector` names the connector, and nothing else |
| `Expr<T>` | JSONLogic resolving to `T`: `OwnedDataValue` (the default), `String`, `u64`, `i64`, `f64`, `bool` or `Path` | Evaluated per call with `ctx.eval`. An expression that reads nothing folds to a constant at build, and `eval` lends it |
| `Interpolated` | A document: literal data with `{"var": "..."}` holes, `$` keys kept as data | Rendered per call with `ctx.render`, each hole looked up once |
| `Output` | A write destination: a path, or JSONLogic computing one | Written through with `ctx.emit`; `#[output(default = "data.x")]` fills an absent key |
| `Option<T>` | An optional key | Absent, or `null` where `T` has no `null` of its own, is `None` |
| `Vec<T>`, `BTreeMap<String, T>` | An array or an object of any of these | Each element at its index or key |
| Another derived config | A nested object | Its own fields, each issue at its path |
| A `Deserialize + Serialize` type, with `#[config(serde)]` | Whatever it deserializes | Read with `serde_json`; an enum of spellings, say |

The attributes:

| Attribute | On | Meaning |
|-----------|----|---------|
| `#[config(rename = "k")]` | field | The JSON key; the default is the field's name |
| `#[config(default = <literal>)]` | field | A string, integer, float or bool, compiled through the field's type when the key is absent |
| `#[config(default_json = "<json>")]` | field | Any JSON default, as text |
| `#[config(serde)]` | field | Read with `serde_json`; an error is `INPUT_PARSE` |
| `#[config(records)]` | field | The engine records the field's result, so it may not read a secret |
| `#[output(default = "p")]` | output | The destination when the key is absent or `null` |
| `#[output(primary)]` | output | Which output `ctx.emit_default` writes through, when there are several |
| `#[config(check = path)]` | struct | A `fn(&Self, &mut CompileCx<'_>)` for rules between fields, run once every field compiled |

The compile reports every issue at once, each at its path below the task's
`function.input`: an unknown key is `UNKNOWN_KEY`, or `LIKELY_TYPO` when it
is a near miss of a known one (`paht` for `path`); a missing required key is
`INPUT_PARSE` (``missing field `path` ``); a literal path outside `data`,
`metadata` and `temp_data` is `INVALID_PATH`. Any of them refuses the task's
workflow, so a malformed input fails at build, not on the first message. A
field's doc comment is its description, which `Registry::catalogue` lists
with the field.

`prepare` runs once the config compiled, with what the config alone cannot
know: here, the connectors the function was built with. Its issues are
relative to the config, and the compiler puts them under the task's path. A
deployment-wide check of every `Reference` a spec declares is the
`program::ResourceOracle`, given to the compiler through
`CompileOptions::resources`: a literal name it does not resolve is reported
as `UNRESOLVED_REFERENCE`, a Defect, which `Compiler::check` returns and the
definition loads with.

### The call

`call` gets a `Ctx`, the function's view of one call, and returns an
`Outcome` or an `Err`. It is an `async fn` whose future is `Send`, so hold
nothing that is not `Send` across an `.await`.

| `Ctx` method | What it does |
|--------------|--------------|
| `eval(&expr)` | Evaluates an `Expr<T>` against the context as this call sees it. A folded constant is lent; anything else is evaluated in a pooled arena and converted to `T` |
| `render(&doc)` | Renders an `Interpolated`: each hole looked up once, lent from where it was found. The result serializes straight into a wire buffer, and `to_owned()` builds a tree |
| `get(&path)` | The value at a context path, borrowed where it can be |
| `payload()` | The message's payload, which no expression reads |
| `secret("name")` | A secret from the engine's store, which the engine never records |
| `info()` | The call's facts: the function's name, `workflow_id`, `task_id`, the channel, the loop counter, the fan-out element's index |
| `emit(&output, value)`, `emit_default(value)` | Writes through an `Output`, or through the config's primary output |
| `set(&path, value)`, `remove(&path)` | Writes or removes at a path |
| `record_error(record)` | Adds an `ErrorRecord` to the message's errors, kept whatever the call returns |
| `input()` | The config resolved for this call with every secret-reading field `null`: what a layer or a dry run reads |
| `resolve(&cfg)` | The config resolved for this call, secrets included: the function's own view of it |
| `deadline()`, `cancel()` | The run's limits, for a function that starts work of its own |

What the call returns:

| Return | Audit status | Record | Then |
|--------|--------------|--------|------|
| `Ok(Outcome::Ok)` | `200` | none | The next task runs |
| `Ok(Outcome::Status(s))`, `s < 400` | `s` | none | The next task runs |
| `Ok(Outcome::Status(s))`, `400 ≤ s < 500` | `s` | none | A rejection: the task's `on_rejection` decides (`continue` by default) |
| `Ok(Outcome::Status(s))`, `s ≥ 500` | `s` | `TASK_STATUS_ERROR` | A failure: the task's `on_failure` decides (`fail` by default, which reaches the workflow) |
| `Ok(Outcome::Skip)` | none | none | The next task runs; no `metadata.progress` is written |
| `Ok(Outcome::Halt)` | `299` | none | The rest of the workflow does not run; later workflows do |
| `Err(e)` | `500` | `e.code()`, as `Task <id> error: <message>` | A failure, as a `5xx` |

`Error::service(code, message)` makes an error with the host's own code,
such as `PARTNER_DOWN`, and `with_retryable(true)` marks it retryable; an
error from `ctx.eval` carries the engine's code (`LOGIC_ERROR`,
`BUDGET_EXCEEDED`). Reserve `Status` for a status the host answers with, and
`Halt` and `Skip` for control flow; do not encode control flow in a status.
See [Actions (Tasks)](../core-concepts/task.md) for the policies.

### What a write does

A call's writes go to an overlay of its own. The call reads its own writes at
once, through `get` and `eval`, and they reach the message only when the
call returns `Ok`, whatever the `Outcome`: an `Err` drops them, and their
`Change`s, and keeps the records the call added with `record_error`. The
settle replays the writes at their own paths, in order, so a write below a
value another write replaced lands as a sequence of plain writes would.

Two rules hold at a context root. A root takes only an object: `set` at
`data` with a string, a number or an array is a `CONFIG_ERROR`, and the root
is kept. And a root cannot be removed. A write at a root replaces it, where
`map`'s root mapping merges; the audit entry records it as a change at
`data`. Every write records a `Change` with the path, the old value and the
new, when the run captures changes (on by default), as `map`'s writes do.

## Registration

`Engine::builder().register(name, f)` registers a function on the builder's
registry; `Registry::register` does the same on a registry a host keeps. A
name registered before is replaced. The eight built-in names, and `validate`,
are reserved: registering one panics, as does a spec the config cannot
honour, such as a `Reference` naming a field the config does not have. A
host that registers names from data checks them with
`def::is_reserved_function` first, or calls `Registry::try_register`, which
returns an `Err` of kind `Config` instead of panicking.

```rust
# use dataflow_rs::datavalue::OwnedDataValue;
# use dataflow_rs::expr::{CompileCx, Interpolated, Output};
# use dataflow_rs::function::{FunctionConfig, FunctionSpec, Prepare, Reference, RetrySafety};
# use dataflow_rs::program::{AuthoredPath, Severity};
# use dataflow_rs::{Ctx, Error, Expr, Function, Issue, Outcome, Result};
# use std::collections::BTreeMap;
# #[derive(FunctionConfig)]
# #[config(check = HttpCallConfig::rules)]
# struct HttpCallConfig {
#     connector: String,
#     #[config(default = "GET")]
#     method: String,
#     path: Expr<String>,
#     #[config(default_json = "{}")]
#     headers: BTreeMap<String, Expr<String>>,
#     body: Option<Interpolated>,
#     #[config(default = 30000)]
#     timeout_ms: Expr<u64>,
#     #[output(default = "data.response")]
#     output: Output,
# }
# const METHODS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];
# impl HttpCallConfig {
#     fn rules(&self, cx: &mut CompileCx<'_>) {
#         if !METHODS.contains(&self.method.as_str()) {
#             cx.at("method", |cx| {
#                 let message = format!("`{}` is not one of {METHODS:?}", self.method);
#                 cx.issue(IssueCode::InputParse, message);
#             });
#         }
#     }
# }
# struct HttpCall {
#     connectors: BTreeMap<String, String>,
# }
# impl HttpCall {
#     fn new(connectors: &[(&str, &str)]) -> Self {
#         Self {
#             connectors: connectors
#                 .iter()
#                 .map(|(name, base)| (name.to_string(), base.to_string()))
#                 .collect(),
#         }
#     }
# }
# impl Function for HttpCall {
#     type Config = HttpCallConfig;
#     fn spec(&self) -> FunctionSpec {
#         FunctionSpec::default()
#             .with_category("io")
#             .with_retry(RetrySafety::Unknown)
#             .with_references(vec![Reference::new("connector", "connector")])
#     }
#     fn prepare(
#         &self,
#         cfg: &mut HttpCallConfig,
#         p: &Prepare<'_>,
#     ) -> std::result::Result<(), Vec<Issue>> {
#         if self.connectors.contains_key(&cfg.connector) {
#             return Ok(());
#         }
#         let message = format!(
#             "task `{}` names connector `{}`, which this deployment does not have",
#             p.task_id(),
#             cfg.connector
#         );
#         let issue = Issue::new(
#             IssueCode::UnresolvedReference,
#             AuthoredPath::root().join("connector"),
#             message,
#         )
#         .with_severity(Severity::Rejected);
#         Err(vec![issue])
#     }
#     async fn call(&self, ctx: &mut Ctx<'_>, cfg: &HttpCallConfig) -> Result<Outcome> {
#         let base = &self.connectors[&cfg.connector];
#         let path = ctx.eval(&cfg.path)?;
#         let timeout_ms = ctx.eval(&cfg.timeout_ms)?;
#         let mut headers = BTreeMap::new();
#         for (name, value) in &cfg.headers {
#             headers.insert(name.clone(), ctx.eval(value)?.into_owned());
#         }
#         let body = match &cfg.body {
#             Some(doc) => serde_json::to_value(ctx.render(doc))
#                 .map_err(|e| Error::service("BODY_ENCODING", e.to_string()))?,
#             None => Value::Null,
#         };
#         let response = json!({
#             "status": 200,
#             "url": format!("{base}{path}"),
#             "method": cfg.method,
#             "headers": headers.keys().collect::<Vec<_>>(),
#             "body": body,
#             "timeout_ms": timeout_ms,
#         });
#         ctx.emit(&cfg.output, OwnedDataValue::from(&response))?;
#         Ok(Outcome::Ok)
#     }
# }
use dataflow_rs::program::IssueCode;
use dataflow_rs::{Engine, Message};
use serde_json::{Value, json};

#[tokio::main]
async fn main() {
    let workflow = json!({
        "id": "lookup",
        "tasks": [
            {
                "id": "load",
                "function": {"name": "parse_json", "input": {"source": "payload", "target": "request"}}
            },
            {
                "id": "fetch_user",
                "function": {"name": "http_call", "input": {
                    "connector": "user_service",
                    "path": {"cat": ["/users/", {"var": "data.request.user_id"}]},
                    "headers": {
                        "Accept": "application/json",
                        "X-Trace": {"cat": ["req-", {"var": "data.request.user_id"}]}
                    },
                    "output": "data.user"
                }}
            },
            {
                "id": "summarise",
                "function": {"name": "map", "input": {"mappings": [
                    {"path": "data.summary.user_url", "logic": {"var": "data.user.url"}}
                ]}}
            }
        ]
    });

    let engine = Engine::builder()
        .register("http_call", HttpCall::new(&[("user_service", "https://users.internal")]))
        .workflow_json(&workflow)
        .build()
        .expect("the engine builds");

    let mut message = Message::from_json(&json!({"user_id": 7}));
    let run = engine.run(&mut message).await;
    run.into_result().expect("the run completes");

    let data = Value::from(message.context().data());
    assert_eq!(data["user"]["url"], json!("https://users.internal/users/7"));
    assert_eq!(data["user"]["method"], json!("GET"));
    assert_eq!(data["user"]["headers"], json!(["Accept", "X-Trace"]));
    assert_eq!(data["user"]["timeout_ms"], json!(30000));
    assert_eq!(data["summary"]["user_url"], json!("https://users.internal/users/7"));

    // A connector the function does not have refuses the workflow at build,
    // through `prepare`, at the key that names it.
    let issues = Engine::builder()
        .register("http_call", HttpCall::new(&[("user_service", "https://users.internal")]))
        .workflow_json(&json!({"id": "w", "tasks": [
            {"id": "fetch", "function": {"name": "http_call", "input": {"connector": "billing", "path": "/x"}}}
        ]}))
        .build()
        .unwrap_err();
    let issue = issues
        .iter()
        .find(|i| i.code == IssueCode::UnresolvedReference)
        .expect("the connector is reported");
    assert_eq!(issue.path.to_string(), "tasks[0].function.input.connector");
    assert_eq!(issue.step_id.as_deref(), Some("fetch"));

    // So does a method the config's own check refuses.
    let issues = Engine::builder()
        .register("http_call", HttpCall::new(&[("user_service", "https://users.internal")]))
        .workflow_json(&json!({"id": "w", "tasks": [
            {"id": "fetch", "function": {"name": "http_call", "input": {
                "connector": "user_service", "method": "FETCH", "path": "/x"}}}
        ]}))
        .build()
        .unwrap_err();
    assert_eq!(issues[0].code, IssueCode::InputParse);
    assert_eq!(issues[0].path.to_string(), "tasks[0].function.input.method");
}
```

A task calls the function by its registered name, and its `input` is the
config's JSON. Everything the engine does for a built-in it does here too:
the compile at build, the audit entry, `metadata.progress`, the policies.

## `for_each` on a host function

A task's `for_each` runs the function once per element of an array. It is
refused on a built-in, which runs inline; a host function is where it
belongs. Each call sees its element at `temp_data.<as>` and the element's
index at `temp_data.<as>_index`, isolated from the other calls: the calls
run under `max_concurrency` at once, and their writes are folded into the
message in element order once every call has finished, so `max_concurrency`
changes timing and never results. With `collect` and `into`, what each call
left at `collect` lands in the array at `into`, in element order.
`ctx.info().element_index` says which element a call is.

```rust
# use dataflow_rs::datavalue::OwnedDataValue;
# use dataflow_rs::expr::{CompileCx, Interpolated, Output};
# use dataflow_rs::function::{FunctionConfig, FunctionSpec, Prepare, Reference, RetrySafety};
# use dataflow_rs::program::{AuthoredPath, IssueCode, Severity};
# use dataflow_rs::{Ctx, Error, Expr, Function, Issue, Outcome, Result};
# use std::collections::BTreeMap;
# #[derive(FunctionConfig)]
# struct HttpCallConfig {
#     connector: String,
#     #[config(default = "GET")]
#     method: String,
#     path: Expr<String>,
#     #[config(default_json = "{}")]
#     headers: BTreeMap<String, Expr<String>>,
#     body: Option<Interpolated>,
#     #[config(default = 30000)]
#     timeout_ms: Expr<u64>,
#     #[output(default = "data.response")]
#     output: Output,
# }
# struct HttpCall {
#     connectors: BTreeMap<String, String>,
# }
# impl HttpCall {
#     fn new(connectors: &[(&str, &str)]) -> Self {
#         Self {
#             connectors: connectors
#                 .iter()
#                 .map(|(name, base)| (name.to_string(), base.to_string()))
#                 .collect(),
#         }
#     }
# }
# impl Function for HttpCall {
#     type Config = HttpCallConfig;
#     async fn call(&self, ctx: &mut Ctx<'_>, cfg: &HttpCallConfig) -> Result<Outcome> {
#         let base = &self.connectors[&cfg.connector];
#         let path = ctx.eval(&cfg.path)?;
#         let timeout_ms = ctx.eval(&cfg.timeout_ms)?;
#         let mut headers = BTreeMap::new();
#         for (name, value) in &cfg.headers {
#             headers.insert(name.clone(), ctx.eval(value)?.into_owned());
#         }
#         let body = match &cfg.body {
#             Some(doc) => serde_json::to_value(ctx.render(doc))
#                 .map_err(|e| Error::service("BODY_ENCODING", e.to_string()))?,
#             None => Value::Null,
#         };
#         let response = json!({
#             "status": 200,
#             "url": format!("{base}{path}"),
#             "method": cfg.method,
#             "headers": headers.keys().collect::<Vec<_>>(),
#             "body": body,
#             "timeout_ms": timeout_ms,
#             "element": ctx.info().element_index,
#         });
#         ctx.emit(&cfg.output, OwnedDataValue::from(&response))?;
#         Ok(Outcome::Ok)
#     }
# }
use dataflow_rs::{Engine, Message};
use serde_json::{Value, json};

#[tokio::main]
async fn main() {
    let workflow = json!({
        "id": "orders",
        "tasks": [
            {
                "id": "load",
                "function": {"name": "parse_json", "input": {"source": "payload", "target": "request"}}
            },
            {
                "id": "fetch_orders",
                "for_each": {
                    "over": {"var": "data.request.order_ids"},
                    "as": "order_id",
                    "max_concurrency": 4,
                    "collect": "temp_data.order",
                    "into": "data.orders"
                },
                "function": {"name": "http_call", "input": {
                    "connector": "orders",
                    "path": {"cat": ["/orders/", {"var": "temp_data.order_id"}]},
                    "output": "temp_data.order"
                }}
            }
        ]
    });
    let engine = Engine::builder()
        .register("http_call", HttpCall::new(&[("orders", "https://orders.internal")]))
        .workflow_json(&workflow)
        .build()
        .expect("the engine builds");

    let mut message = Message::from_json(&json!({"order_ids": [11, 12, 13]}));
    engine.run(&mut message).await.into_result().expect("the run completes");

    let data = Value::from(message.context().data());
    let orders = data["orders"].as_array().expect("one result per element");
    assert_eq!(orders.len(), 3);
    assert_eq!(orders[0]["url"], json!("https://orders.internal/orders/11"));
    assert_eq!(orders[2]["url"], json!("https://orders.internal/orders/13"));
    assert_eq!(orders[2]["element"], json!(2));
}
```

The folding rules, what a failing element leaves, and the loop shape are on
the [Fan-Out](../advanced/for-each.md) page.

## Layers

A `CallLayer` wraps host calls: it is handed the call's `Ctx` and the rest
of the chain, `Next`, and runs the chain with `next.run(ctx).await`, or
answers without running it. The built-ins run inline and are never wrapped.

A layer is added at two scopes. `EngineBuilder::layer`, or
`Registry::layer`, wraps every host call of every run; `RunOptions::layers`
wraps the calls of one run. The run's layers are outermost, then the
registry's, each list in the order it was added, then the function.

`layers::Answer` is a ready-made layer for a dry run: a closure is asked
about each call and sees its `Ctx`, with `info()`, `input()` and
`primary_output()`; `Some(value)` is written through the callee's primary
output and the call returns `Ok` without reaching the layers inside it or the
function, and `None` passes the call on. `input()` is the config resolved
for the call with every secret-reading field `null`, so a dry run can log a
request without logging its credential:

```rust
# use dataflow_rs::expr::{CompileCx, Interpolated, Output};
# use dataflow_rs::function::{FunctionConfig, FunctionSpec, Prepare, Reference, RetrySafety};
# use dataflow_rs::program::{AuthoredPath, IssueCode, Severity};
# use dataflow_rs::{Error, Expr, Function, Issue};
# use std::collections::BTreeMap;
# #[derive(FunctionConfig)]
# struct HttpCallConfig {
#     connector: String,
#     #[config(default = "GET")]
#     method: String,
#     path: Expr<String>,
#     #[config(default_json = "{}")]
#     headers: BTreeMap<String, Expr<String>>,
#     body: Option<Interpolated>,
#     #[config(default = 30000)]
#     timeout_ms: Expr<u64>,
#     #[output(default = "data.response")]
#     output: Output,
# }
# struct HttpCall {
#     connectors: BTreeMap<String, String>,
# }
# impl HttpCall {
#     fn new(connectors: &[(&str, &str)]) -> Self {
#         Self {
#             connectors: connectors
#                 .iter()
#                 .map(|(name, base)| (name.to_string(), base.to_string()))
#                 .collect(),
#         }
#     }
# }
# impl Function for HttpCall {
#     type Config = HttpCallConfig;
#     async fn call(&self, ctx: &mut Ctx<'_>, cfg: &HttpCallConfig) -> Result<Outcome> {
#         let base = &self.connectors[&cfg.connector];
#         let path = ctx.eval(&cfg.path)?;
#         let timeout_ms = ctx.eval(&cfg.timeout_ms)?;
#         let mut headers = BTreeMap::new();
#         for (name, value) in &cfg.headers {
#             headers.insert(name.clone(), ctx.eval(value)?.into_owned());
#         }
#         let body = match &cfg.body {
#             Some(doc) => serde_json::to_value(ctx.render(doc))
#                 .map_err(|e| Error::service("BODY_ENCODING", e.to_string()))?,
#             None => Value::Null,
#         };
#         let response = json!({
#             "status": 200,
#             "url": format!("{base}{path}"),
#             "method": cfg.method,
#             "headers": headers.keys().collect::<Vec<_>>(),
#             "body": body,
#             "timeout_ms": timeout_ms,
#         });
#         ctx.emit(&cfg.output, OwnedDataValue::from(&response))?;
#         Ok(Outcome::Ok)
#     }
# }
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::{ExprEngine, Secrets};
use dataflow_rs::function::{BoxFuture, CallLayer, Next, layers::Answer};
use dataflow_rs::{Ctx, Engine, Message, Outcome, Registry, Result, RunOptions};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Times every host call into shared counters.
struct Metered {
    calls: Arc<AtomicU64>,
    nanos: Arc<AtomicU64>,
}

impl CallLayer for Metered {
    fn call<'a>(&'a self, ctx: &'a mut Ctx<'_>, next: Next<'a>) -> BoxFuture<'a, Result<Outcome>> {
        Box::pin(async move {
            let start = Instant::now();
            let outcome = next.run(ctx).await;
            self.nanos.fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
            self.calls.fetch_add(1, Ordering::Relaxed);
            outcome
        })
    }
}

#[tokio::main]
async fn main() {
    // The engine's secrets: a header reads one by name.
    let secrets = Secrets::new(OwnedDataValue::from(&json!({"user_service_token": "s3cr3t"})))
        .expect("an object");
    let exprs = ExprEngine::builder().secrets(secrets).build().expect("the expression engine builds");

    let calls = Arc::new(AtomicU64::new(0));
    let engine = Engine::builder()
        .registry(Registry::new(exprs))
        .register("http_call", HttpCall::new(&[("user_service", "https://users.internal")]))
        .layer(Metered { calls: Arc::clone(&calls), nanos: Arc::new(AtomicU64::new(0)) })
        .workflow_json(&json!({"id": "w", "tasks": [
            {"id": "fetch", "function": {"name": "http_call", "input": {
                "connector": "user_service",
                "path": "/users/7",
                "headers": {"Authorization": {"cat": ["Bearer ", {"secret": "user_service_token"}]}},
                "output": "data.user"
            }}}
        ]}))
        .build()
        .expect("the engine builds");

    // A real run: the function runs inside the meter.
    let mut message = Message::new(OwnedDataValue::Null);
    engine.run(&mut message).await.into_result().expect("the run completes");
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    let data = Value::from(message.context().data());
    assert_eq!(data["user"]["headers"], json!(["Authorization"]));
    assert!(!message.to_json().to_string().contains("s3cr3t"));

    // A dry run: every `http_call` with somewhere to write is answered with
    // the request it would have made; any other call goes through.
    let dry_run: Vec<Arc<dyn CallLayer>> = vec![Arc::new(Answer::new(|ctx| {
        if ctx.info().function != "http_call" || ctx.primary_output().is_none() {
            return None;
        }
        let input = ctx.input().ok()?;
        Some(OwnedDataValue::from(&json!({"status": "dry run", "request": Value::from(&input)})))
    }))];
    let opts = RunOptions::default().layers(&dry_run);
    let mut message = Message::new(OwnedDataValue::Null);
    engine.run_with(&mut message, &opts).await.into_result().expect("the run completes");

    let data = Value::from(message.context().data());
    assert_eq!(data["user"]["status"], json!("dry run"));
    assert_eq!(data["user"]["request"]["path"], json!("/users/7"));
    assert_eq!(data["user"]["request"]["timeout_ms"], json!(30000));
    // The header that reads a secret is `null` in `input()`: never evaluated.
    assert_eq!(data["user"]["request"]["headers"]["Authorization"], Value::Null);
    // The answer came before the registry's layers: the meter saw no call.
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}
```

Inside the function, `ctx.eval` on the same header gives the real value: a
secret is the function's to use and the engine's to keep out of the message.
A field whose result the engine records, an `Output` or a field marked
`#[config(records)]`, may not read a secret at all; the definition is refused
with `SECRET_IN_MESSAGE_WRITE`. See [Secrets](../advanced/secrets.md).

## Testing a function

`function::testing::call(f, input)` runs one call of one function outside
any workflow, through the real compile, the real `Ctx` and overlay, and the
layers it is given, and settles it as a run would. No registry, compiler,
definition or engine is assembled by hand. It is a builder: `.message(m)`
sets the message the call runs over, `.layer(l)` adds a layer, `.name(n)`
the name the function is called by; `.await` runs it and gives a `Called`
with the compile `issues`, the `outcome`, the call's `writes` and `changes`,
the `errors` it recorded, and the `message` after the settle. `testing::config`
compiles a config alone.

```rust
# use dataflow_rs::expr::{CompileCx, Interpolated, Output};
# use dataflow_rs::function::{FunctionConfig, FunctionSpec, Prepare, Reference, RetrySafety};
# use dataflow_rs::program::{AuthoredPath, Severity};
# use dataflow_rs::{Ctx, Error, Expr, Function, Issue, Result};
# use std::collections::BTreeMap;
# #[derive(FunctionConfig)]
# struct HttpCallConfig {
#     connector: String,
#     #[config(default = "GET")]
#     method: String,
#     path: Expr<String>,
#     #[config(default_json = "{}")]
#     headers: BTreeMap<String, Expr<String>>,
#     body: Option<Interpolated>,
#     #[config(default = 30000)]
#     timeout_ms: Expr<u64>,
#     #[output(default = "data.response")]
#     output: Output,
# }
# struct HttpCall {
#     connectors: BTreeMap<String, String>,
# }
# impl HttpCall {
#     fn new(connectors: &[(&str, &str)]) -> Self {
#         Self {
#             connectors: connectors
#                 .iter()
#                 .map(|(name, base)| (name.to_string(), base.to_string()))
#                 .collect(),
#         }
#     }
# }
# impl Function for HttpCall {
#     type Config = HttpCallConfig;
#     fn prepare(
#         &self,
#         cfg: &mut HttpCallConfig,
#         p: &Prepare<'_>,
#     ) -> std::result::Result<(), Vec<Issue>> {
#         if self.connectors.contains_key(&cfg.connector) {
#             return Ok(());
#         }
#         let message = format!(
#             "task `{}` names connector `{}`, which this deployment does not have",
#             p.task_id(),
#             cfg.connector
#         );
#         let issue = Issue::new(
#             IssueCode::UnresolvedReference,
#             AuthoredPath::root().join("connector"),
#             message,
#         )
#         .with_severity(Severity::Rejected);
#         Err(vec![issue])
#     }
#     async fn call(&self, ctx: &mut Ctx<'_>, cfg: &HttpCallConfig) -> Result<Outcome> {
#         let base = &self.connectors[&cfg.connector];
#         let path = ctx.eval(&cfg.path)?;
#         let timeout_ms = ctx.eval(&cfg.timeout_ms)?;
#         let mut headers = BTreeMap::new();
#         for (name, value) in &cfg.headers {
#             headers.insert(name.clone(), ctx.eval(value)?.into_owned());
#         }
#         let body = match &cfg.body {
#             Some(doc) => serde_json::to_value(ctx.render(doc))
#                 .map_err(|e| Error::service("BODY_ENCODING", e.to_string()))?,
#             None => Value::Null,
#         };
#         let response = json!({
#             "status": 200,
#             "url": format!("{base}{path}"),
#             "method": cfg.method,
#             "headers": headers.keys().collect::<Vec<_>>(),
#             "body": body,
#             "timeout_ms": timeout_ms,
#         });
#         ctx.emit(&cfg.output, OwnedDataValue::from(&response))?;
#         Ok(Outcome::Ok)
#     }
# }
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::function::testing;
use dataflow_rs::program::IssueCode;
use dataflow_rs::{Message, Outcome};
use serde_json::{Value, json};

# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
let http_call = || HttpCall::new(&[("user_service", "https://users.internal")]);

// One call over a message, through the real compile and overlay.
let input = json!({
    "connector": "user_service",
    "method": "POST",
    "path": {"cat": ["/users/", {"var": "data.id"}]},
    "body": {"include": ["orders"], "since": {"var": "data.since"}}
});
let data = OwnedDataValue::from(&json!({"id": 7, "since": "2026-01-01"}));
let called = testing::call(http_call(), input)
    .message(Message::from_parts(OwnedDataValue::Null, data, OwnedDataValue::Null))
    .await;

assert!(called.issues.is_empty(), "{:?}", called.issues);
assert_eq!(called.outcome.unwrap(), Outcome::Ok);
assert_eq!(called.writes.len(), 1);
let at = "data.response".parse().unwrap();
let response = Value::from(called.message.context().get(&at).unwrap());
assert_eq!(response["url"], json!("https://users.internal/users/7"));
assert_eq!(response["method"], json!("POST"));
assert_eq!(response["body"], json!({"include": ["orders"], "since": "2026-01-01"}));

// A config that does not compile runs nothing; `issues` says why.
let called = testing::call(http_call(), json!({"connector": "user_service", "paht": "/x"})).await;
let codes: Vec<IssueCode> = called.issues.iter().map(|i| i.code).collect();
assert_eq!(codes, [IssueCode::LikelyTypo, IssueCode::InputParse]);
assert!(called.outcome.is_err());
assert!(called.writes.is_empty());

// A config alone, its defaults filled in.
let cfg: HttpCallConfig =
    testing::config(json!({"connector": "user_service", "path": "/x"})).ok().unwrap();
assert_eq!(cfg.method, "GET");
assert_eq!(cfg.timeout_ms.constant(), Some(&30000));
assert_eq!(cfg.output.constant().unwrap().to_string(), "data.response");
# });
```

## Plugins with runtime fields

A function whose fields are not known when the host is compiled, such as a
plugin loaded from a manifest, declares them at registration instead:
`FunctionSpec::with_runtime_fields` lists them as `FieldSpec`s, and the
function's `Config` is `function::Templated`, which compiles them by their
kinds and lets the call read them by name. The API reference documents both.

## Next Steps

- [Fan-Out](../advanced/for-each.md): `for_each` on a host function, in full
- [Secrets](../advanced/secrets.md): what a function may read and what the engine never records
- [Custom Functions](../advanced/custom-functions.md): the longer guide
- [API Reference](../api/reference.md)
