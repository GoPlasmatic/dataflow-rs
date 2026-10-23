# Host Functions

Dataflow-rs ships with eight [built-in functions](./overview.md) and performs no external I/O on its own. Everything else a workflow calls—such as an HTTP request, database query, queue publish, or ML model inference—is a **host function**: a custom `Function` implementation that you register on the engine by name. Its configuration is compiled once when the engine is built, and its `async fn call` method is invoked whenever a task executes it.

> **Migrating from 3.x:** Version 3.x included config structs for `http_call`, `enrich`, and `publish_kafka` without built-in runtime implementations. In 4.0, you register your own `Function` implementations under those names using `#[derive(FunctionConfig)]`. See [Migrating from 3.x](../migration/from-3.md).

This guide walks through building a complete `http_call` host function from scratch. To keep all examples self-contained and runnable without network access, `HttpCall` echoes back a structured description of the request; in production, you would hold an `Arc<reqwest::Client>` (or similar client) on the struct.

## A Host Function in Three Parts

Every custom host function consists of:
1. A config struct with `#[derive(FunctionConfig)]`
2. A struct implementing `Function`
3. Registration on `EngineBuilder` (or `Registry`)

```rust
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::{CompileCx, Interpolated, Output};
use dataflow_rs::function::{FunctionConfig, FunctionSpec, Prepare, Reference, RetrySafety};
use dataflow_rs::program::{AuthoredPath, IssueCode, Severity};
use dataflow_rs::{Ctx, Error, Expr, Function, Issue, Outcome, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The `input` configuration for an `http_call` task, compiled once at build time.
#[derive(FunctionConfig)]
#[config(check = HttpCallConfig::rules)]
struct HttpCallConfig {
    /// Name of the configured connector to send the request through.
    connector: String,
    /// HTTP method.
    #[config(default = "GET")]
    method: String,
    /// Request URL path (static string or JSONLogic expression).
    path: Expr<String>,
    /// Request headers, where each value is a static string or JSONLogic expression.
    #[config(default_json = "{}")]
    headers: BTreeMap<String, Expr<String>>,
    /// Optional request body template with `{"var": "..."}` placeholders.
    body: Option<Interpolated>,
    /// Request timeout in milliseconds.
    #[config(default = 30000)]
    timeout_ms: Expr<u64>,
    /// Destination path where the response is written.
    #[output(default = "data.response")]
    output: Output,
}

const METHODS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];

impl HttpCallConfig {
    /// Custom build-time validation rule checked after all fields compile.
    fn rules(&self, cx: &mut CompileCx<'_>) {
        if !METHODS.contains(&self.method.as_str()) {
            cx.at("method", |cx| {
                let message = format!("`{}` is not one of {METHODS:?}", self.method);
                cx.issue(IssueCode::InputParse, message);
            });
        }
    }
}

/// Sends HTTP requests through named connectors.
struct HttpCall {
    /// Map of connector name to base URL.
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

    /// Metadata describing this function for linters, UIs, and catalogues.
    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default()
            .with_description("Sends an HTTP request through a named connector and writes the response.")
            .with_category("io")
            .with_retry(RetrySafety::Unknown)
            .with_references(vec![Reference::new("connector", "connector")])
    }

    /// Deployment-specific build-time check: verify that `cfg.connector` exists
    /// in `self.connectors`. Returning a `Rejected` issue rejects the workflow at build time.
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

    /// Execute a single task call against the current message context.
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

        // In production, send `cfg.method` to `{base}{path}` with `headers` and `body`.
        // Note: we record only header names (not values) in the response because a
        // header may contain a secret, and anything written via `ctx.emit` is recorded
        // on the message.
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

### 1. The Config Struct (`#[derive(FunctionConfig)]`)

`#[derive(FunctionConfig)]` generates the build-time compiler, schema specification, and runtime resolver for a struct with named fields. Each field's Rust type determines how the corresponding JSON key in `function.input` is compiled and evaluated:

| Field Type | What the JSON Key Accepts | How It Is Evaluated at Runtime |
|------------|--------------------|-----------------|
| `String`, `bool`, integer/float primitives, `serde_json::Value`, `OwnedDataValue` | Static JSON literal of that type | Read directly from the config struct (never evaluated as JSONLogic) |
| `Expr<T>` (`OwnedDataValue`, `String`, `u64`, `i64`, `f64`, `bool`, `Path`) | Static literal or JSONLogic expression resolving to `T` | Evaluated per call with `ctx.eval(&field)`. Static literals fold to constants at build time and are borrowed with zero overhead |
| `Interpolated` | JSON document template with `{"var": "..."}` placeholders (`$` keys are kept as literal data) | Rendered per call with `ctx.render(&field)`, looking up each placeholder once |
| `Output` | Destination context path (static string or dynamic JSONLogic expression) | Written to via `ctx.emit(&field, value)` or `ctx.emit_default(value)` |
| `Option<T>` | Optional field | `None` when the key is omitted (or `null` when `T` does not accept `null`) |
| `Vec<T>`, `BTreeMap<String, T>` | Array or object map of any supported config field type | Compiled element-by-element with full JSON path tracking |
| Nested `#[derive(FunctionConfig)]` struct | Nested JSON object | Compiled recursively with issues reported at their nested paths |
| Any `Deserialize + Serialize` type with `#[config(serde)]` | Any JSON value matching the Serde type | Deserialized at build time via `serde_json` |

Supported attributes on `#[derive(FunctionConfig)]`:

| Attribute | Target | Description |
|-----------|----|---------|
| `#[config(rename = "key")]` | Field | Override the JSON property name (defaults to the Rust field name) |
| `#[config(default = <literal>)]` | Field | Default string, integer, float, or boolean value when the JSON key is omitted |
| `#[config(default_json = "<json>")]` | Field | Default JSON value (parsed from the string literal) when the key is omitted |
| `#[config(serde)]` | Field | Deserialize the field at build time using `serde_json` (errors are reported as `INPUT_PARSE`) |
| `#[config(records)]` | Field | Marks a field whose result is recorded on the message, forbidding `{"secret": "..."}` in that field |
| `#[output(default = "data.x")]` | `Output` field | Default destination path when the key is omitted or `null` |
| `#[output(primary)]` | `Output` field | Marks the primary output used by `ctx.emit_default(value)` when a config has multiple `Output` fields |
| `#[config(check = path::to::fn)]` | Struct | Runs a custom `fn(&Self, &mut CompileCx<'_>)` validator after all fields compile |

At build time, config compilation collects all issues across all fields at once, reporting each at its exact path under `tasks[i].function.input`:
- Unrecognized keys are reported as `UNKNOWN_KEY` (or `LIKELY_TYPO` for near misses like `paht` instead of `path`).
- Missing required fields or wrong types are reported as `INPUT_PARSE`.
- Literal paths outside `data`, `metadata`, and `temp_data` are reported as `INVALID_PATH`.
- Doc comments on struct fields are captured automatically as field descriptions in `Registry::catalogue()`.

After `FunctionConfig` compiles the struct, `Function::prepare` runs so your function instance can validate external references (such as checking `cfg.connector` against `self.connectors`). You can also validate declared `Reference`s across all functions using a `program::ResourceOracle` configured on `CompileOptions::resources`.

### 2. The `call` Method and `Ctx`

`Function::call` receives `&mut Ctx<'_>` and `&Self::Config`, and returns `Result<Outcome>`. Its returned future must be `Send`.

| `Ctx` Method | Description |
|--------------|--------------|
| `eval(&expr)` | Evaluates an `Expr<T>` against the current context (borrows folded constants directly; evaluates dynamic expressions in a pooled arena). |
| `render(&doc)` | Renders an `Interpolated` document template (serializable directly or convertible to `OwnedDataValue` via `.to_owned()`). |
| `get(&path)` | Borrows the current value at `path` (including uncommitted writes made earlier in the same call). |
| `payload()` | Borrows the message's read-only `payload`. |
| `secret("name")` | Looks up a secret value from the engine's `Secrets` store. |
| `info()` | Returns `CallIdentity` metadata: `function`, `workflow_id`, `task_id`, `channel`, `routing_bucket`, `loop_counter`, and `element_index`. |
| `emit(&output, value)`, `emit_default(value)` | Evaluates `output` (or the config's primary output) and writes `value` to that path. |
| `set(&path, value)`, `remove(&path)` | Writes or removes a value at `path`. |
| `record_error(record)` | Appends an `ErrorRecord` to `message.errors()` (preserved even if `call` later returns `Err`). |
| `input()` | Evaluates the config into an `OwnedDataValue` with all secret-reading expressions replaced by `null` (safe for layers and dry runs). |
| `resolve(&cfg)` | Evaluates the entire config into an `OwnedDataValue` with secrets included (for use inside `call`). |
| `deadline()`, `cancel()` | Returns the run's `Instant` deadline or `CancelToken` so your function can forward them to downstream I/O. |

How the engine handles the return value of `call`:

| Return Value | Audit Status | Recorded Error | Next Action |
|--------|--------------|--------|------|
| `Ok(Outcome::Ok)` | `200` | None | Continue to the next task |
| `Ok(Outcome::Status(s))` (`s < 400`) | `s` | None | Continue to the next task |
| `Ok(Outcome::Status(s))` (`400 <= s < 500`) | `s` | None | Treated as a **rejection**: governed by `on_rejection` (`"continue"` by default) |
| `Ok(Outcome::Status(s))` (`s >= 500`) | `s` | `TASK_STATUS_ERROR` | Treated as a **failure**: governed by `on_failure` (`"fail"` by default) |
| `Ok(Outcome::Skip)` | *(none)* | None | Skips audit entry and `metadata.progress`; continues to the next task |
| `Ok(Outcome::Halt)` | `299` | None | Stops the current workflow (`WorkflowExit::Halted`); subsequent workflows still run |
| `Err(e)` | `500` | `e.code()` (`Task <id> error: <message>`) | Treated as a **failure** (discards all context writes from this call) |

### Transactional Overlay Writes

All writes made during a host function call (`emit`, `emit_default`, `set`, `remove`) are buffered in a per-call overlay:
- Your function can immediately read its own uncommitted writes via `ctx.get` and `ctx.eval`.
- When `call` returns `Ok(...)`, the engine commits those writes to the message context in order and records their `Change` entries on the audit trail.
- When `call` returns `Err(...)`, the engine **discards** all context writes and `Change` entries from that call, ensuring a failed function never leaves the message in a half-updated state (any explicit error records added via `ctx.record_error` are still preserved).

Note on context roots (`data`, `metadata`, `temp_data`):
- Root paths must always hold JSON objects; calling `ctx.set` at `"data"` with a non-object returns a `CONFIG_ERROR`, and root paths cannot be removed.
- Calling `ctx.set` at a root replaces the root object (unlike `map`, which merges keys into the root).

## 3. Registering and Running the Function

Use `Engine::builder().register(name, func)` (or `Registry::register`) to register your function. Built-in function names (and `"validate"`) are reserved—registering a reserved name with `register` panics; use `Registry::try_register` or `def::is_reserved_function` when registering dynamic plugin names.

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

    // Referencing an unknown connector fails at build time via `prepare`:
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

    // Specifying an unsupported HTTP method fails at build time via `#[config(check = ...)]`:
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

## Fan-Out (`for_each`) on a Host Function

Adding a `for_each` block to a task executes its host function once per element of a source array:
- Each element call sees its item at `temp_data.<as>` and its zero-based index at `temp_data.<as>_index` (and `ctx.info().element_index`), isolated from sibling element calls.
- Up to `max_concurrency` calls run concurrently, and their writes are folded into the message in element order once all calls finish—so changing `max_concurrency` affects latency without changing the resulting message state.
- When `collect` and `into` are configured, the value each call writes to `collect` is gathered into an array at `into` in element order.

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

See [Fan-Out (`for_each`)](../advanced/for-each.md) for complete details on element folding and error policies.

## Middleware Layers (`CallLayer`)

A `CallLayer` wraps host function calls, receiving the call's `Ctx` and the remainder of the call chain (`Next`). A layer can inspect or modify `ctx`, time the call, delegate via `next.run(ctx).await`, or short-circuit with a synthetic response. (Built-in functions run inline and do not pass through `CallLayer`s.)

You can attach layers at two scopes:
- **Engine-wide:** `EngineBuilder::layer` (or `Registry::layer`) wraps every host call across all runs.
- **Per-run:** `RunOptions::layers` wraps host calls for a single run, executing outside the registry's layers.

`function::layers::Answer` is a built-in helper layer for dry runs and tests: you pass a closure that inspects `ctx` (`ctx.info()`, `ctx.input()`, `ctx.primary_output()`) and returns `Some(value)` to write `value` to the primary output and short-circuit with `Outcome::Ok`, or `None` to pass the call through. Because `ctx.input()` redacts any field that references a secret to `null`, dry-run layers can safely log or record resolved task inputs without leaking credentials:

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

/// Middleware layer that counts and times host function calls.
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
    // Configure a secret store so the Authorization header can read a token.
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

    // 1. Live run: executes `HttpCall` inside the `Metered` layer.
    let mut message = Message::new(OwnedDataValue::Null);
    engine.run(&mut message).await.into_result().expect("the run completes");
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    let data = Value::from(message.context().data());
    assert_eq!(data["user"]["headers"], json!(["Authorization"]));
    assert!(!message.to_json().to_string().contains("s3cr3t"));

    // 2. Dry run: `Answer` intercepts `http_call` and writes the resolved input
    //    without calling `HttpCall` or inner registry layers.
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
    // The secret-bearing header is automatically redacted to `null` in `ctx.input()`.
    assert_eq!(data["user"]["request"]["headers"]["Authorization"], Value::Null);
    // Because the per-run `Answer` layer short-circuited outside `Metered`, `calls` is still 1.
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}
```

## Unit-Testing a Host Function (`function::testing`)

`function::testing::call(func, input_json)` lets you unit-test a single host function call directly—running real config compilation, `prepare`, `Ctx` overlay buffering, and any layers—without needing to construct a full workflow or `Engine`.

- Chain `.message(msg)`, `.layer(layer)`, or `.name("name")` on the builder, then `.await` it to get a `Called` report containing `issues`, `outcome`, `writes`, `changes`, `errors`, and the updated `message`.
- Use `function::testing::config(input_json)` to test config compilation in isolation.

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

// 1. Test a single call against a seeded message:
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

// 2. Test an invalid config (reports `LikelyTypo` for `paht` and `InputParse` for missing `path`):
let called = testing::call(http_call(), json!({"connector": "user_service", "paht": "/x"})).await;
let codes: Vec<IssueCode> = called.issues.iter().map(|i| i.code).collect();
assert_eq!(codes, [IssueCode::LikelyTypo, IssueCode::InputParse]);
assert!(called.outcome.is_err());
assert!(called.writes.is_empty());

// 3. Test config compilation and default values on their own:
let cfg: HttpCallConfig =
    testing::config(json!({"connector": "user_service", "path": "/x"})).ok().unwrap();
assert_eq!(cfg.method, "GET");
assert_eq!(cfg.timeout_ms.constant(), Some(&30000));
assert_eq!(cfg.output.constant().unwrap().to_string(), "data.response");
# });
```

## Dynamic Plugins (`Templated` Configs)

If a function's configuration schema is loaded dynamically at runtime (for example, from a plugin manifest rather than a compile-time Rust struct), set `type Config = function::Templated` and declare its fields via `FunctionSpec::with_runtime_fields(vec![FieldSpec::...])`. The engine will validate and compile the fields according to their declared `FieldKind`s and allow `call` to evaluate them by field name. See the [Custom Functions guide](../advanced/custom-functions.md#plugins-whose-fields-come-from-a-manifest) for an example.

## Next Steps

- [Custom Functions](../advanced/custom-functions.md) — Deep dive into config types, `Interpolated` documents, and dynamic plugins
- [Fan-Out (`for_each`)](../advanced/for-each.md) — Concurrent per-element execution and folding rules
- [Secrets](../advanced/secrets.md) — Working with secrets safely inside host functions
- [API Reference](../api/reference.md) — Complete type and method reference
