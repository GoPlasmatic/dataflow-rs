# Custom Functions

Every task executes a function. Eight functions are built in (`map`,
`validation`, `filter`, `log`, `parse_json`, `parse_xml`, `publish_json`, and
`publish_xml`) and execute inline. Any other function is a **host function**: a
Rust type you write that implements `Function`, which the engine calls once per
task execution. This guide covers how to implement, test, and register custom
functions.

## The `Function` Trait

```text
pub trait Function: Send + Sync + 'static {
    type Config: FunctionConfig;

    fn spec(&self) -> FunctionSpec { FunctionSpec::default() }

    fn prepare(&self, cfg: &mut Self::Config, p: &Prepare<'_>)
        -> Result<(), Vec<Issue>> { Ok(()) }

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &Self::Config)
        -> Result<Outcome>;
}
```

The trait consists of three main components:

- **`Config`**: Represents the task's `input` as a typed Rust struct. The engine
  compiles the configuration once when the workflow is compiled, so malformed
  inputs are reported at their authored JSON path at compile time rather than
  failing when a message is processed. Use `#[derive(FunctionConfig)]` to
  generate the implementation automatically.
- **`Ctx`**: The execution context for the call. Use it to read message context
  data, evaluate configuration expressions, write output values, and record
  errors.
- **`Outcome`**: The execution result returned to the engine: `Ok`,
  `Status(u16)`, `Skip`, or `Halt`.

`call` is a standard `async fn` and does not require `#[async_trait]` or manual
future boxing.

## Your First Custom Function

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::Output;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::{Ctx, Engine, Expr, Function, Message, Outcome, Result};
use serde_json::json;

/// The task's `input`: `{"name": <expression>, "output": <path>}`.
#[derive(FunctionConfig)]
struct GreetConfig {
    /// Who to greet. Accepts any JSONLogic expression that produces a string.
    name: Expr<String>,
    /// Destination path for the greeting. Optional in the input; defaults to `data.greeting`.
    #[output(default = "data.greeting")]
    output: Output,
}

struct Greet;

impl Function for Greet {
    type Config = GreetConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &GreetConfig) -> Result<Outcome> {
        let name = ctx.eval(&cfg.name)?;
        let greeting = OwnedDataValue::from(&json!(format!("hello, {name}")));
        ctx.emit(&cfg.output, greeting)?;
        Ok(Outcome::Ok)
    }
}

let engine = Engine::builder()
    .register("greet", Greet)
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "t", "function": {"name": "greet",
                                 "input": {"name": {"var": "data.name"}}}}
    ]}))
    .build()
    .unwrap();

let mut message = Message::from_parts(
    OwnedDataValue::Null,
    OwnedDataValue::from(&json!({"name": "ada"})),
    OwnedDataValue::Null,
);
let run = engine.run(&mut message).await;
assert_eq!(run.matched(), 1);
assert_eq!(
    message.to_json()["context"]["data"]["greeting"],
    json!("hello, ada")
);
# });
```

`Engine::builder().register("greet", Greet)` registers the function under the
name referenced by `function.name` in workflow tasks. Registering the same name
twice replaces the previous implementation.

The eight built-in function names—plus `validate` (the 3.x alias for
`validation`)—are reserved. Attempting to register a reserved name with
`register` panics. If your application registers function names dynamically from
external configuration, check them first with `def::is_reserved_function` or call
`Registry::try_register`, which returns an `Err` of kind `Config` instead of
panicking.

If a function takes no input parameters, set `type Config = ();`. Its `input`
in the workflow JSON must then be `{}`, `null`, or omitted.

## Function Configuration

`#[derive(FunctionConfig)]` can be applied to any struct with named fields. Each
field is compiled from the matching JSON key, and any validation issue is
reported at its exact path (such as `tasks[2].function.input.timeout_ms`).
Unknown keys are rejected: keys close to a known field name are reported as
`LIKELY_TYPO` (along with the suggested field name), and all other unknown keys
are reported as `UNKNOWN_KEY`.

### Supported Field Types

| Rust Type | In Workflow `input` JSON | In `call` |
|---|---|---|
| `Expr<T>` | Any JSONLogic expression (literals evaluate to themselves) | `ctx.eval(&field)` returns `T` |
| `Output` | A literal or computed context path | `ctx.emit(&field, value)` writes to that path |
| `Interpolated` | A JSON document containing `{"var": …}` placeholders | `ctx.render(&field)` substitutes the placeholders |
| `bool`, `String`, `u8`–`u64`, `usize`, `i32`, `i64`, `f64` | A literal of that type (not evaluated as JSONLogic) | Used directly as a value |
| `serde_json::Value`, `OwnedDataValue` | Any raw JSON value, kept verbatim | Used directly as a value |
| `Option<T>` | Omitted, `null`, or a valid `T` | `None` or `Some(T)` |
| `Vec<T>` | A JSON array of `T` | `Vec<T>` |
| `BTreeMap<String, T>` | A JSON object whose values are `T` | `BTreeMap<String, T>` |
| Another `#[derive(FunctionConfig)]` struct | A nested JSON object | The nested config struct |

In `Expr<T>`, `T` can be:
- `OwnedDataValue` (the default when you write `Expr` without a type parameter,
  accepting any JSON value)
- `String` (returns string contents directly, or formats any other value as
  compact JSON)
- `u64`, `i64`, `f64`, `bool`, or `Path`

Any expression that does not read from the message context is constant-folded at
compile time and accessible via `Expr::constant()`. If a folded constant has the
wrong type, the compiler reports an `INPUT_PARSE` issue when building the
workflow rather than failing at runtime.

An explicit `null` in an `Option<T>` field becomes `None`, unless `T` itself can
represent `null` (`Expr`, `Interpolated`, `serde_json::Value`, and
`OwnedDataValue`), in which case it becomes `Some` containing a constant `null`.

### Configuration Attributes

| Attribute | Target | Description |
|---|---|---|
| `#[config(rename = "k")]` | Field | Sets the JSON key name (defaults to the Rust field name). |
| `#[config(default = <literal>)]` | Field | Provides a default string, integer, float, or boolean literal when the JSON key is omitted. |
| `#[config(default_json = "<json>")]` | Field | Provides an arbitrary JSON default as a string literal; invalid JSON causes a Rust compile error. |
| `#[config(serde)]` | Field | Deserializes the field with `serde_json` instead of `ConfigField` (requires `Deserialize + Serialize`). |
| `#[config(records)]` | Field | Marks the field's output as recorded by the engine, disallowing secret reads (see [Secrets](./secrets.md)). |
| `#[output(default = "p")]` | `Output` field | Provides a default target path when the key is omitted or `null`. |
| `#[output(primary)]` | `Output` field | Marks which `Output` field `ctx.emit_default` writes to (required on exactly one field when a struct has multiple `Output` fields). |
| `#[config(check = path)]` | Struct | Calls `fn(&Self, &mut CompileCx<'_>)` after all fields are compiled to validate cross-field constraints. |

Each field's Rust doc comment (`/// ...`) becomes its description in the
function catalogue.

```rust
use dataflow_rs::expr::{CompileCx, Output};
use dataflow_rs::function::{FunctionConfig, testing};
use dataflow_rs::program::IssueCode;
use dataflow_rs::Expr;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Method {
    Get,
    Post,
}

#[derive(FunctionConfig)]
#[config(check = HttpConfig::check)]
struct HttpConfig {
    /// The URL to call.
    url: Expr<String>,
    #[config(serde, default = "get")]
    method: Method,
    #[config(rename = "timeout_ms", default = 5000)]
    timeout: Expr<u64>,
    /// Extra headers, where each value is an expression.
    headers: Option<std::collections::BTreeMap<String, Expr<String>>>,
    #[output(default = "data.response")]
    output: Output,
}

impl HttpConfig {
    fn check(&self, cx: &mut CompileCx<'_>) {
        if self.timeout.constant().is_some_and(|ms| *ms > 60_000) {
            cx.at("timeout_ms", |cx| {
                cx.issue(IssueCode::InputParse, "at most 60000");
            });
        }
    }
}

// Compile a config standalone; issues are reported relative to the input object.
let cfg: HttpConfig = testing::config(json!({"url": "https://example.com"})).unwrap();
assert_eq!(cfg.timeout.constant(), Some(&5000));
assert_eq!(cfg.output.constant().unwrap().to_string(), "data.response");

let issues = testing::config::<HttpConfig>(json!({
    "url": "https://example.com", "timeout_ms": 90000, "ulr": "x"
})).err().unwrap();
let codes: Vec<(IssueCode, String)> =
    issues.iter().map(|i| (i.code, i.path.to_string())).collect();
assert_eq!(codes, [
    (IssueCode::LikelyTypo, "ulr".to_owned()),
    (IssueCode::InputParse, "timeout_ms".to_owned()),
]);
```

### Custom Field Types

You can create your own reusable field types by implementing `ConfigField`. This
trait defines how the field compiles, which expressions it contains (so secret
and template-key checks inspect them), how it resolves during a call, and how it
is described for authoring tools. A common pattern is a newtype wrapper around
`Expr` (for example, a connector name); see the `ConfigField` API documentation
for a complete example.

### Runtime-Defined Fields (`Templated`)

If your application loads function definitions dynamically from plugin manifests
rather than fixed Rust structs, use `Templated` as your `Function::Config`. A
`Templated` function declares its expected fields at runtime via
`FunctionSpec::runtime_fields`. Each `FieldSpec` defines the field's name, kind
(typed expression, output path, interpolated document, or literal), whether it
is required, and an optional default value. Inside `call`, read individual
fields by name with `Templated::get` or evaluate all fields at once with
`Templated::resolved`.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::{FieldKind, FieldSpec, ValueKind};
use dataflow_rs::function::{FunctionSpec, Templated, testing};
use dataflow_rs::{Ctx, Function, Outcome, Result};
use serde_json::json;

/// A manifest-defined function with a required `url` and an optional `out` defaulting to `data.out`.
struct Plugin;

impl Function for Plugin {
    type Config = Templated;

    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default().with_runtime_fields(vec![
            FieldSpec::runtime("url", FieldKind::Expr(ValueKind::String), true),
            FieldSpec::runtime("out", FieldKind::Output, false).with_default("\"data.out\""),
        ])
    }

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &Templated) -> Result<Outcome> {
        let url = cfg.get(ctx, "url")?.expect("url is required").into_owned();
        ctx.emit_default(url)?;
        Ok(Outcome::Ok)
    }
}

let called = testing::call(Plugin, json!({"url": "https://example.com"})).await;
assert_eq!(called.outcome.unwrap(), Outcome::Ok);
let out = called.message.context().get(&"data.out".parse().unwrap());
assert_eq!(out, Some(&OwnedDataValue::from(&json!("https://example.com"))));
# });
```

Registering a `Templated` function without runtime fields—or declaring runtime
fields on a function whose `Config` is not `Templated`—panics during
registration, since every workflow compilation would otherwise fail.

## The Call Context (`Ctx`)

`Ctx` is passed to `call` and to every `CallLayer`. It reads from the run's
shared context view without copying it per call, and buffers the current call's
writes in an isolated overlay. Reads within the same call see those buffered
writes immediately, while the underlying `Message` is only updated after the
call returns `Ok`.

### Reading from `Ctx`

| Method | Returns |
|---|---|
| `ctx.eval(&expr)` | Evaluates the expression against the call's view of the context (buffered writes first, then fan-out element bindings, then the message context). |
| `ctx.get(&path)` | Borrows the value at a `Path`. |
| `ctx.render(&doc)` | Renders an `Interpolated` document with its placeholders filled in as a `Rendered` value that serializes directly. |
| `ctx.payload()` | Returns the message's raw `payload` (which JSONLogic expressions cannot read). |
| `ctx.secret(name)` | Looks up a secret value from the engine's secret store by dotted name. |
| `ctx.info()` | Returns `CallInfo`: workflow and task IDs, registered function name, loop counter, fan-out element index, channel, and routing bucket. |
| `ctx.input()` | Evaluates the entire config into an object while replacing secret-reading fields with `null` without evaluating them (intended for logging layers and dry runs). |
| `ctx.resolve(&cfg)` | Evaluates the entire config into an object, including secrets (intended for the function's own use). |
| `ctx.deadline()`, `ctx.cancel()` | Returns the run's deadline and cancellation token so you can forward them to child operations or sub-runs. |
| `ctx.exprs()` | Returns the `ExprEngine` used to compile the program, allowing the function to compile ad-hoc expressions. |

### Writing to `Ctx`

| Method | Action |
|---|---|
| `ctx.set(&path, value)` | Writes `value` at `path`, creating intermediate objects as needed. |
| `ctx.emit(&output, value)` | Writes `value` to the literal or computed path specified by an `Output` field. |
| `ctx.emit_default(value)` | Writes `value` to the config's primary output field. |
| `ctx.remove(&path)` | Removes the key at `path` (does nothing if the path does not exist). |
| `ctx.record_error(record)` | Appends an `ErrorRecord` to `message.errors()`. |

Three rules govern how writes are committed to the message:

- **Context writes are committed only when `call` returns `Ok`.** Regardless of
  which `Outcome` variant is returned inside `Ok(...)`, buffered writes are
  applied in order at their target paths. If `call` returns `Err(...)`, all
  buffered context writes are discarded, leaving the message context untouched.
- **Recorded errors are always kept.** Any error added via `ctx.record_error`
  remains on `message.errors()` even if `call` returns `Err`. If the record has
  no timestamp, the engine stamps it with the run's timestamp.
- **Context roots must remain objects.** Calling `ctx.set` at `data`, `metadata`,
  or `temp_data` with a non-object value—or calling `ctx.remove` on a root—returns
  a `Config` error.

Writing beneath a scalar value or to a named key on an array is a no-op and
returns `Ok(())` (matching `Path::set`). When change capture is enabled, each
committed write records a `Change` entry (see [Audit Trails](./audit-trails.md)).

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::function::testing::{self, Write};
use dataflow_rs::{Ctx, Function, Message, Outcome, Path, Result};
use serde_json::json;

/// Increments `data.visits` and removes stale `data.cache`.
struct Visit;

impl Function for Visit {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        let visits: Path = "data.visits".parse()?;
        let seen = ctx.get(&visits).and_then(|v| v.as_i64()).unwrap_or(0);
        ctx.set(&visits, OwnedDataValue::from(&json!(seen + 1)))?;
        // Subsequent reads in the same call see the write immediately.
        assert_eq!(ctx.get(&visits).and_then(|v| v.as_i64()), Some(seen + 1));
        ctx.remove(&"data.cache".parse()?)?;
        Ok(Outcome::Ok)
    }
}

let data = OwnedDataValue::from(&json!({"visits": 1, "cache": "stale"}));
let called = testing::call(Visit, json!({}))
    .message(Message::from_parts(OwnedDataValue::Null, data, OwnedDataValue::Null))
    .await;
assert_eq!(called.writes[0].1, Write::Set(OwnedDataValue::from(&json!(2))));
assert_eq!(called.writes[1].1, Write::Removed);
assert_eq!(
    called.message.context().data(),
    &OwnedDataValue::from(&json!({"visits": 2}))
);
# });
```

### Interpolated Documents

When building a query or payload for an external system (such as a MongoDB
filter), you often want a mostly literal JSON document with a few dynamic
values—and without `$` keys being treated as JSONLogic escape prefixes.

Use `Interpolated` for this: it compiles a JSON document where only
`{"var": "…"}` nodes are evaluated as placeholders and all other keys (including
`$oid`, `$set`, etc.) are kept verbatim. Calling `ctx.render(&field)` resolves
each placeholder once and returns a `Rendered` value that serializes directly
without allocating a new document tree (or call `Rendered::to_owned()` if you
need an owned tree).

```rust
use dataflow_rs::expr::Interpolated;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::{Ctx, Function, Outcome, Result};
use serde_json::json;

#[derive(FunctionConfig)]
struct UpdateConfig {
    filter: Interpolated,
}

struct Update;

impl Function for Update {
    type Config = UpdateConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &UpdateConfig) -> Result<Outcome> {
        let filter = ctx.render(&cfg.filter);
        // `filter` serializes directly as `{"_id":{"$oid":"<data.id>"}}`.
        // Pass it to your driver or call `filter.to_owned()` for an owned value.
        let _bson = serde_json::to_string(&filter);
        Ok(Outcome::Ok)
    }
}
# let _ = (Update, json!({"filter": {"_id": {"$oid": {"var": "data.id"}}}}));
```

Only `var` placeholders are evaluated inside an `Interpolated` document:
- Other JSONLogic operator-shaped objects remain literal data and trigger an
  `OPERATOR_IN_DOCUMENT` advisory at compile time.
- `{"secret": …}` nodes are rejected with `SECRET_IN_DOCUMENT`.
- `var` placeholders rooted at `payload` always resolve to their default value
  and trigger a `PAYLOAD_VAR` defect.

## Return Values and Outcomes

| `call` Return Value | Recorded Status | Error Record | Next Step |
|---|---|---|---|
| `Ok(Outcome::Ok)` | `200` | None | Runs the next task |
| `Ok(Outcome::Status(s))` (`s < 400`) | `s` | None | Runs the next task |
| `Ok(Outcome::Status(s))` (`400 <= s < 500`) | `s` | None | Handled by the task's `on_rejection` policy |
| `Ok(Outcome::Status(s))` (`s >= 500`) | `s` | `TASK_STATUS_ERROR` | Handled by the task's `on_failure` policy |
| `Ok(Outcome::Skip)` | None (no audit entry) | None | Runs the next task |
| `Ok(Outcome::Halt)` | `299` | None | Stops the current workflow |
| `Err(e)` | `500` | `e.code()` | Handled by the task's `on_failure` policy |

Task-level `on_failure` and `on_rejection` policies control how rejections and
failures affect workflow execution; see [Control Flow](./control-flow.md).
- `Outcome::Skip` indicates that the task was a no-op: no audit entry is added
  and `metadata.progress` is not updated.
- `Outcome::Halt` stops the current workflow immediately, while allowing
  subsequent workflows in the engine to run.

When returning `Err`, use `Error::service(code, message)` to attach a
domain-specific error code (for example,
`Error::service("PARTNER_DOWN", "the partner API returned 503")` records the
code `"PARTNER_DOWN"`). Engine-generated errors use fixed codes: `LOGIC_ERROR`,
`BUDGET_EXCEEDED`, `TIMEOUT_ERROR`, `CANCELLED`, `CONFIG_ERROR`, and
`COMPILATION_ERROR`. By default, only timeout errors are marked retryable; call
`.with_retryable(true)` when a failure can be retried, and `.with_source(err)`
to attach an underlying error cause.

```rust
use dataflow_rs::{Error, Result};

fn partner_down(status: u16) -> Result<()> {
    Err(Error::service("PARTNER_DOWN", format!("the partner returned {status}"))
        .with_retryable(status == 503))
}

let e = partner_down(503).unwrap_err();
assert_eq!(e.code(), "PARTNER_DOWN");
assert!(e.is_retryable());
```

## Function Metadata and Compile-Time Preparation

`Function::spec` returns a `FunctionSpec` that is read once at registration and
exposed via `Registry::catalogue` for UI and authoring tools. You can use it to
specify:
- A human-readable description and category
- Retry safety (`RetrySafety::Idempotent`, `NotIdempotent`, or `Unknown`)
- External resource references (`Reference::new("connector", "connector")`),
  which a `ResourceOracle` validates at compile time (see
  [Authoring-Time Validation](./authoring-validation.md))
- Runtime field specifications for `Templated` functions

`Function::prepare` runs once per task at compile time, right after `Config` has
been compiled. It receives a `&mut Self::Config` and a `Prepare` handle
containing context that the config struct alone cannot see (the workflow ID,
task ID, and `FunctionSpec`). Return any validation issues with paths relative
to the config object; the compiler automatically prefixes them with
`function.input`, and any issue with `Rejected` severity fails workflow
compilation.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::function::{FunctionConfig, FunctionSpec, Prepare, RetrySafety, testing};
use dataflow_rs::program::IssueCode;
use dataflow_rs::{Ctx, Expr, Function, Issue, Outcome, Result};
use serde_json::json;

#[derive(FunctionConfig)]
struct ChargeConfig {
    amount: Expr<f64>,
}

struct Charge;

impl Function for Charge {
    type Config = ChargeConfig;

    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default()
            .with_description("Charges the customer")
            .with_category("payments")
            .with_retry(RetrySafety::NotIdempotent)
    }

    fn prepare(&self, cfg: &mut ChargeConfig, p: &Prepare<'_>) -> std::result::Result<(), Vec<Issue>> {
        // Validate that literal amounts are positive. Paths are relative to the config.
        if cfg.amount.constant().is_some_and(|a| *a <= 0.0) {
            let message = format!("task `{}` charges a non-positive amount", p.task_id());
            return Err(vec![Issue::new(IssueCode::InputParse, "amount".parse().unwrap(), message)]);
        }
        Ok(())
    }

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &ChargeConfig) -> Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

let called = testing::call(Charge, json!({"amount": -1})).await;
assert_eq!(called.issues[0].code, IssueCode::InputParse);
assert_eq!(called.issues[0].path.to_string(), "amount");
assert!(called.outcome.is_err());
# });
```

## Call Layers (Middleware)

A `CallLayer` wraps host function invocations like middleware. It receives the
call's `Ctx` and the remainder of the execution chain (`Next`), and can inspect
or modify the context, call `next.run(ctx).await`, or short-circuit the call and
return an `Outcome` directly. Built-in functions run inline and are never
wrapped by layers.

Layers can be attached at two scopes:
- **Engine-wide:** `Engine::builder().layer(l)` (or `Registry::layer`) wraps
  every host function call across all runs.
- **Per-run:** `RunOptions::layers` wraps host function calls for a single run,
  executing outside the registry's layers.

Within each list, the first layer added is the outermost wrapper.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::function::{BoxFuture, CallLayer, Next, testing};
use dataflow_rs::{Ctx, Function, Outcome, Result};
use serde_json::json;

/// Rewrites any `5xx` status into `503`.
struct Unavailable;

impl CallLayer for Unavailable {
    fn call<'a>(&'a self, ctx: &'a mut Ctx<'_>, next: Next<'a>) -> BoxFuture<'a, Result<Outcome>> {
        Box::pin(async move {
            Ok(match next.run(ctx).await? {
                Outcome::Status(500..) => Outcome::Status(503),
                outcome => outcome,
            })
        })
    }
}

/// Returns status `500` on every call.
struct Down;

impl Function for Down {
    type Config = ();

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        Ok(Outcome::Status(500))
    }
}

let called = testing::call(Down, json!({})).layer(Unavailable).await;
assert_eq!(called.outcome.unwrap(), Outcome::Status(503));
# });
```

Inside a layer, you can inspect `ctx.info()` and `ctx.input()`. Because
`ctx.input()` resolves any secret-reading field to `null` without evaluating it,
logging layers cannot accidentally leak secrets. Layers can also write outputs
using `ctx.emit` and `ctx.primary_output()` (the target path used by
`ctx.emit_default`).

For dry runs and mocking, use the built-in `function::layers::Answer` layer. It
passes each call's `Ctx` to a closure: when the closure returns `Some(value)`,
the layer writes `value` to the function's primary output and returns
`Ok(Outcome::Ok)` without invoking the underlying function.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::Output;
use dataflow_rs::function::{CallLayer, FunctionConfig, layers::Answer};
use dataflow_rs::{Ctx, Engine, Expr, Function, Message, Outcome, Result, RunOptions};
use serde_json::json;
use std::sync::Arc;

#[derive(FunctionConfig)]
struct HttpConfig {
    url: Expr<String>,
    #[output(default = "data.response")]
    output: Output,
}

/// A network function that is intercepted during dry runs.
struct HttpCall;

impl Function for HttpCall {
    type Config = HttpConfig;

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &HttpConfig) -> Result<Outcome> {
        unreachable!("answered by the dry-run layer")
    }
}

let engine = Engine::builder()
    .register("http_call", HttpCall)
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "fetch", "function": {"name": "http_call",
                                     "input": {"url": "https://example.com/orders"}}}
    ]}))
    .build()
    .unwrap();

// Stub out `http_call` with a synthetic response while letting other functions run normally.
let dry_run: Vec<Arc<dyn CallLayer>> = vec![Arc::new(Answer::new(|ctx| {
    if ctx.info().function != "http_call" || ctx.primary_output().is_none() {
        return None;
    }
    let input = ctx.input().ok()?;
    Some(OwnedDataValue::Object(vec![
        ("status".into(), "dry run".into()),
        ("request".into(), input),
    ]))
}))];

let mut message = Message::from_json(&json!({}));
let run = engine.run_with(&mut message, &RunOptions::default().layers(&dry_run)).await;
assert_eq!(run.matched(), 1);
assert_eq!(
    message.to_json()["context"]["data"]["response"],
    json!({"status": "dry run", "request": {"url": "https://example.com/orders",
                                             "output": "data.response"}})
);
# });
```

## Testing Custom Functions

`function::testing::call(f, input)` executes a single function call using the
same pipeline as the engine—compiling the config, providing a real `Ctx` over a
test message, running any attached layers, and committing writes on `Ok` or
discarding them on `Err`—without needing to set up a `Registry`, `Compiler`, or
`Engine`.

You can customize the test call with builder methods before awaiting it:
- `.message(m)`
- `.layer(l)`
- `.exprs(engine)` (for custom operators or secrets)
- `.name(n)`
- `.capture_changes(on)`
- `.channel(c)`
- `.deadline(at)`
- `.cancel(token)`

Awaiting the builder returns a `Called` struct containing:
- `issues`: Any compile-time issues produced by the config or `prepare`
- `outcome`: The call result (or a `Compile` error if the config was rejected)
- `writes`: All writes performed by the call, in order
- `changes`: Change records captured during the call
- `errors`: Error records added via `ctx.record_error`
- `message`: The resulting `Message` after writes are committed

To test configuration parsing and validation on its own, use
`testing::config::<C>(input)` (or `testing::config_with(&exprs, input)` when
using a custom `ExprEngine`).

## Inspecting Call Metadata

`ctx.info()` returns a `CallInfo` struct identifying the current execution step:
- `workflow_id` and `task_id` (both default to `"test"` under `testing::call`)
- `function`: The registered name used to invoke the function
- `channel`: The active channel name, if any
- `loop_counter`: `Some(n)` on iteration `n` of a workflow [`loop`](./loops.md)
- `element_index`: `Some(i)` when executing inside a task's [`for_each`](./for-each.md)

### Execution Inside `for_each` (Fan-Out)

When a task has a `for_each` block, the engine calls the function once per
array element. Each invocation sees its own element bound at `temp_data.<as>`
and its index at `temp_data.<as>_index`, layered on top of the shared message
context without copying it. Concurrent element calls cannot see each other's
writes; after all element calls complete, their writes are committed to the
message in element order. See [Fan-Out](./for-each.md) for details.

## Deadlines and Cancellation

A run can be configured with a deadline (`RunOptions::deadline`) and a
cancellation token (`RunOptions::cancel`). The engine checks both before each
host function call and races them against the call's future (on native targets).
If a deadline expires or the token is cancelled while a call is running, the
call future is dropped, its uncommitted writes are discarded, and the run ends
with `TimedOut` or `Cancelled`.

If your function starts a nested engine run or async sub-operation, pass along
`ctx.deadline()` and `ctx.cancel().child()` so cancelling the parent run also
cancels the child operation. Note that if a function itself returns an `Err` of
kind `TimedOut` or `Cancelled`, the engine treats it as a task failure governed
by the task's `on_failure` policy.

## Migrating from 3.x

In 3.x, custom functions implemented `AsyncFunctionHandler` using
`#[async_trait]`, took a `TaskContext<'_>`, returned `TaskOutcome`, performed
infallible writes via `TaskContext::set`, and manually compiled `Template`
fields in `compile_input`.

In 4.0:
- The trait is `Function` (using native `async fn`, without `#[async_trait]`).
- The context type is `Ctx`, and write operations return `Result`.
- The return type is `Outcome` (with the same four variants).
- Configurations use `#[derive(FunctionConfig)]`, where `Expr` fields compile
  automatically.

See [Migrating from 3.x](../migration/from-3.md) for a step-by-step migration
guide.
