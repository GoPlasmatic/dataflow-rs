# Custom Functions

A task runs a function. Eight functions are built in (`map`, `validation`,
`filter`, `log`, `parse_json`, `parse_xml`, `publish_json`, `publish_xml`)
and run inline. Everything else is a **host function**: a type you write,
implementing `Function`, that the engine calls once per task run. This page
is the contract for writing one.

## The trait

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

Three parts:

- **`Config`** is the task's `input`, as a typed struct. The engine compiles
  it once, when the workflow is compiled, so a malformed input is an issue at
  its authored path and never a message-time error.
  `#[derive(FunctionConfig)]` writes the implementation.
- **`Ctx`** is the call's view of the run: it reads the context, evaluates
  the config's expressions, writes results, and records errors.
- **`Outcome`** is what the call tells the engine: `Ok`, `Status(u16)`,
  `Skip` or `Halt`.

`call` is an ordinary `async fn`, with no attribute macro and no boxing: the
trait asks for a `Send` future, which an `async fn` in the impl provides.

## A first function

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
    /// Who to greet. Any JSONLogic that gives a string.
    name: Expr<String>,
    /// Where the greeting goes. Optional in the input; this is the default.
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

`Engine::builder().register("greet", Greet)` registers the function under
the name a task's `function.name` calls it by. A name registered twice is
replaced. The eight built-in names, and `validate` (the 3.x alias for
`validation`), are reserved: registering under one panics, so a host that
registers names from data checks them against `def::is_reserved_function`
first, or calls `Registry::try_register`, which returns an `Err` of kind
`Config` instead of panicking.

A function that takes no input has `type Config = ();`. Its input must then
be `{}`, `null` or absent.

## The config

`#[derive(FunctionConfig)]` applies to a struct with named fields. Each field
is compiled from the key of its name, every issue is reported at its own
path (`tasks[2].function.input.timeout_ms`), and an unknown key is refused:
a near miss of a known key as `LIKELY_TYPO`, naming the key it resembles, and
any other as `UNKNOWN_KEY`.

### Field types

| Type | In the input | At the call |
|---|---|---|
| `Expr<T>` | Any JSONLogic. A literal is JSONLogic for itself. | `ctx.eval(&field)` gives a `T` |
| `Output` | A context path, literal or computed | `ctx.emit(&field, value)` writes through it |
| `Interpolated` | A document with `{"var": …}` holes | `ctx.render(&field)` fills the holes |
| `bool`, `String`, `u8` to `u64`, `usize`, `i32`, `i64`, `f64` | A literal of that type, never evaluated | The value |
| `serde_json::Value`, `OwnedDataValue` | Any JSON, kept as written | The value |
| `Option<T>` | Absent, `null`, or a `T` | `None` or `Some` |
| `Vec<T>` | An array of `T` | The elements |
| `BTreeMap<String, T>` | An object of `T` | The entries |
| Another derived config | A nested object | The nested struct |

`T` in `Expr<T>` is one of `OwnedDataValue` (the default, so `Expr` alone
means any value), `String` (a string's contents; any other result as its
compact JSON), `u64`, `i64`, `f64`, `bool` and `Path`. An expression that
reads nothing folds to a constant when the workflow is compiled, and
`Expr::constant()` lends it; a folded constant of the wrong type is an
`INPUT_PARSE` issue at build, not an error on the first message.

An explicit `null` in an `Option<T>` field is `None`, unless `T` takes `null`
as a value, as `Expr`, `Interpolated`, `serde_json::Value` and
`OwnedDataValue` do: there it is a constant `null`.

### Attributes

| Attribute | On | Meaning |
|---|---|---|
| `#[config(rename = "k")]` | field | The JSON key; the default is the field's name |
| `#[config(default = <literal>)]` | field | A string, integer, float or bool, compiled through the field's type when the key is absent |
| `#[config(default_json = "<json>")]` | field | Any JSON default, as text; text that is not JSON is a compile error where it is written |
| `#[config(serde)]` | field | Read with `serde_json` instead of `ConfigField`, for an enum or a struct of your own; the type is `Deserialize` and `Serialize` |
| `#[config(records)]` | field | The engine records the field's result, so it may not read a secret (see [Secrets](./secrets.md)) |
| `#[output(default = "p")]` | output | The path when the key is absent or `null` |
| `#[output(primary)]` | output | Which output `ctx.emit_default` writes through; required on exactly one when there are several |
| `#[config(check = path)]` | struct | `fn(&Self, &mut CompileCx<'_>)`, called after every field compiled, for rules between fields |

A field's doc comment is its description in the function's catalogue entry.

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
    /// Extra headers, each value an expression.
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

// A config compiled alone, with every issue at its path below the input.
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

### Your own field type

A type of your own joins the table by implementing `ConfigField`: how it
compiles, which expressions it holds (so the secret and template-key checks
see them), how it resolves for a call, and what kind of field it is for
tooling. A newtype over an `Expr` is the common case: a connector name, say.
The `ConfigField` rustdoc shows one.

### Fields declared at runtime

A plugin host registers one function type per manifest entry, and the
manifest, not the type, says what the config's fields are. Its config is
`Templated`: the fields come from the `FunctionSpec::runtime_fields` the
function's `spec` declares, each a `FieldSpec` with a name, a kind (an
expression of a given kind, an output, a document or a literal), whether it
is required, and a default. A call reads them by name with `Templated::get`,
or all at once with `Templated::resolved`.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::{FieldKind, FieldSpec, ValueKind};
use dataflow_rs::function::{FunctionSpec, Templated, testing};
use dataflow_rs::{Ctx, Function, Outcome, Result};
use serde_json::json;

/// One manifest entry: a required `url`, and an `out` defaulting to `data.out`.
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

Registering a `Templated` function with no runtime fields, or declaring
runtime fields on a function whose config is not `Templated`, panics at
`register`: either would fail every compile.

## The call context

`Ctx` is handed to `call` and to every layer. It reads the run's one view
of the context (no view is built per call) and keeps the call's writes in
an overlay of its own, so a read sees them at once while the message does
not until the call returns.

### Reading

| Method | Gives |
|---|---|
| `ctx.eval(&expr)` | The expression evaluated against the context as this call sees it: the call's writes, then a fan-out element's bindings, then the run's context |
| `ctx.get(&path)` | The value at a `Path`, borrowed where it can be |
| `ctx.render(&doc)` | An `Interpolated` document with its holes filled, as a `Rendered` that serializes straight into whatever takes it |
| `ctx.payload()` | The message's payload, which no expression reads |
| `ctx.secret(name)` | A value from the engine's secret store, by dotted name |
| `ctx.info()` | The call's `CallInfo`: workflow and task ids, the function's registered name, the loop counter, the fan-out element index, the channel, the routing bucket |
| `ctx.input()` | The whole config resolved for this call, with every field that reads a secret `null` and never evaluated: what a layer or a dry run reads |
| `ctx.resolve(&cfg)` | The whole config resolved for this call, secrets included: the function's own view, to hand on whole |
| `ctx.deadline()`, `ctx.cancel()` | The run's deadline and cancellation token, to pass to a run this function starts |
| `ctx.exprs()` | The expression engine the program was compiled with, for compiling an expression of the function's own |

### Writing

| Method | Does |
|---|---|
| `ctx.set(&path, value)` | Assigns at a path, creating the containers on the way |
| `ctx.emit(&output, value)` | `set` at the path an `Output` names, literal or computed for this call |
| `ctx.emit_default(value)` | `emit` through the config's primary output |
| `ctx.remove(&path)` | Removes the key at a path; nothing there is nothing to do |
| `ctx.record_error(record)` | Appends an `ErrorRecord` to the message's errors |

Three rules decide what reaches the message:

- **Writes land when the call returns `Ok`, whatever its `Outcome`, and are
  dropped when it returns `Err`.** A function that writes a partial result
  and then fails leaves the context as it found it. The engine replays the
  writes at their own paths, in order, so a second call's write to a sibling
  key does not undo the first's.
- **Records are kept either way.** What `record_error` added stays on the
  message after an `Err` too, with the run's timestamp when the record
  carries none.
- **A root takes only an object, and cannot be removed.** `set` at `data`,
  `metadata` or `temp_data` with anything but an object is a `Config`
  error, and so is `remove` at a root.

A write below a scalar, or a key on an array, lands nowhere and is `Ok`; the
walk is `Path::set`'s. Writes record a `Change` each while the run captures
changes (see [Audit Trails](./audit-trails.md)).

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::function::testing::{self, Write};
use dataflow_rs::{Ctx, Function, Message, Outcome, Path, Result};
use serde_json::json;

/// Counts visits at `data.visits`, and drops the stale `data.cache`.
struct Visit;

impl Function for Visit {
    type Config = ();

    async fn call(&self, ctx: &mut Ctx<'_>, _cfg: &()) -> Result<Outcome> {
        let visits: Path = "data.visits".parse()?;
        let seen = ctx.get(&visits).and_then(|v| v.as_i64()).unwrap_or(0);
        ctx.set(&visits, OwnedDataValue::from(&json!(seen + 1)))?;
        // The call reads its own write at once.
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

### Documents with holes

A payload for another system, a MongoDB filter say, is data with a few live
parts. An `Interpolated` field is that: `{"var": "…"}` holes in otherwise
literal JSON, `$` keys kept as written, compiled once. `ctx.render` looks
each hole up once and hands back a `Rendered`, which serializes the
document's skeleton with each value in place and builds no tree;
`Rendered::to_owned` builds one when a caller needs it.

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
        // `filter` serializes as `{"_id":{"$oid":"<data.id>"}}`; hand it to
        // the driver, or `filter.to_owned()` for a tree.
        let _bson = serde_json::to_string(&filter);
        Ok(Outcome::Ok)
    }
}
# let _ = (Update, json!({"filter": {"_id": {"$oid": {"var": "data.id"}}}}));
```

Only `var` is live in a document. Any other operator-shaped node stays data
and is reported as `OPERATOR_IN_DOCUMENT` (an advisory), a `{"secret": …}`
node is refused as `SECRET_IN_DOCUMENT`, and a hole rooted at `payload`
always yields its default and is reported as `PAYLOAD_VAR`.

## What a call returns

| `call` returns | Status recorded | Error record | Then |
|---|---|---|---|
| `Ok(Outcome::Ok)` | `200` | none | the next task |
| `Ok(Outcome::Status(s))`, `s < 400` | `s` | none | the next task |
| `Ok(Outcome::Status(s))`, `400 <= s < 500` | `s` | none | the task's `on_rejection` |
| `Ok(Outcome::Status(s))`, `s >= 500` | `s` | `TASK_STATUS_ERROR` | the task's `on_failure` |
| `Ok(Outcome::Skip)` | none, and no audit entry | none | the next task |
| `Ok(Outcome::Halt)` | `299` | none | the rest of the workflow does not run |
| `Err(e)` | `500` | `e`'s code | the task's `on_failure` |

The policies are the task's `on_failure` and `on_rejection`; see
[Control Flow](./control-flow.md). `Skip` is "did nothing": it records no
audit entry and writes no `metadata.progress`. `Halt` stops this workflow
only; later workflows still see the message.

An `Err` carries a code. `Error::service(code, message)` is the function's
own: `Error::service("PARTNER_DOWN", "the partner API returned 503")` is
recorded as `PARTNER_DOWN`. The engine's own kinds have fixed codes:
`LOGIC_ERROR` for an evaluation that failed, `BUDGET_EXCEEDED`,
`TIMEOUT_ERROR`, `CANCELLED`, `CONFIG_ERROR`, `COMPILATION_ERROR`. Only a timeout is retryable by default;
`with_retryable(true)` says a retry may succeed, and `with_source` keeps the
cause.

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

## What the function says about itself

`spec` returns a `FunctionSpec`, read once at registration and listed by
`Registry::catalogue` for tooling: a description, a category, whether a
retry is safe (`RetrySafety::Idempotent`, `NotIdempotent` or `Unknown`), the
config fields that name another definition (`Reference::new("connector",
"connector")`, which a `ResourceOracle` resolves at compile; see
[Authoring-Time Validation](./authoring-validation.md)), and the runtime
fields of a `Templated` config.

`prepare` runs once per task at compile, after the config compiled, with
what the config alone cannot know: the task's and workflow's ids and the
function's own spec, through `Prepare`. It returns issues with paths
relative to the config; the compiler prefixes `function.input`, and a
Rejected one fails the task's compile.

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
        // A literal amount must be positive. The path is relative to the config.
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

## Layers

A `CallLayer` wraps host calls: it is handed the `Ctx` and the rest of the
chain as `Next`, and either runs `next.run(ctx).await` or answers without
it. The built-ins are never wrapped.

Layers live at two scopes. `Engine::builder().layer(l)` (or
`Registry::layer`) wraps every host call of every run; `RunOptions::layers`
wraps one run's calls, outside the registry's. In each list the first added
is the outermost.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::function::{BoxFuture, CallLayer, Next, testing};
use dataflow_rs::{Ctx, Function, Outcome, Result};
use serde_json::json;

/// Turns any `5xx` into a `503`.
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

/// Fails every call with a `500`.
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

A layer sees the call's `ctx.info()` and `ctx.input()`, the config resolved
with every secret-reading field `null`, so a logging layer never reads a
secret. It may write through `ctx.emit` and `ctx.primary_output()`, the
output `emit_default` would use.

`function::layers::Answer` is the ready-made dry run: a closure is asked
about each call and, when it returns `Some(value)`, the value is written
through the callee's primary output and the call returns `Ok` without
reaching the function.

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

/// Would call the network; the dry run never lets it.
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

// Every `http_call` answers with the input it was given; the rest run.
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

## Testing a function

`function::testing::call(f, input)` runs one call through the real path:
the config compiles as a workflow would compile it, the call gets a real
`Ctx` over the message given, the layers given wrap it, and the settle
applies the writes on `Ok` and drops them on `Err`. It needs no registry,
compiler, definition or engine. The builder's setters are optional:
`.message(m)`, `.layer(l)`, `.exprs(engine)` for a host's own operators and
secrets, `.name(n)`, `.capture_changes(on)`, `.channel(c)`, `.deadline(at)`
and `.cancel(token)`.

`.await` gives a `Called`: the compile `issues`, the `outcome` (an `Err` of
kind `Compile` when the config was rejected and nothing ran), the `writes`
in order, the `changes` recorded, the `errors` the call added, and the
`message` after the settle. No recorder runs, so the message gains no audit
entry and no `metadata.progress`: the `Called` says what the call returned.

`testing::config::<C>(input)` compiles a config alone, `Err` carrying every
issue, and `testing::config_with(&exprs, input)` compiles it against a
host's engine.

## Knowing which call you are

`ctx.info()` is the call's `CallInfo`, for labelling what a function
produces: `workflow_id` and `task_id`, `function` (the registered name it
was called by), `channel`, `loop_counter` (`Some(n)` on sweep `n` of a
workflow carrying a [`loop`](./loops.md), named counter or not), and
`element_index` (`Some(i)` inside a [`for_each`](./for-each.md)). Under
`testing::call` the ids are both `"test"`.

### Inside a fan-out

A task carrying `for_each` calls the function once per element. Each call
sees its element at `temp_data.<as>` and its index at `temp_data.<as>_index`,
bound beneath the call's own writes over the run's one view: no call copies
the context, and no call sees another's writes. The writes are replayed into
the message in element order after every call has finished. See
[Fan-Out](./for-each.md).

## Deadlines and cancellation

A run may carry a deadline and a cancellation token (`RunOptions::deadline`,
`RunOptions::cancel`). Both are checked before each host call and, natively,
raced against it: a call they cut short is dropped, its writes with it, and
the run ends `TimedOut` or `Cancelled`. A function that starts a run of its
own passes `ctx.deadline()` on and hands the child run `ctx.cancel()`'s
`child()`, so cancelling the parent cancels both. A function's own `Err` of
kind `TimedOut` or `Cancelled` is a failure like any other, handled by its
task's `on_failure`.

## Coming from 3.x

In 3.x a host function was an `AsyncFunctionHandler` with an `#[async_trait]`
`execute(&self, ctx: &mut TaskContext<'_>, input: &Self::Input)` returning
`TaskOutcome`, writing through `TaskContext::set` with no `Result`, and
`Template` fields compiled by hand in `compile_input`. In 4.0 the trait is
`Function`, the context `Ctx`, the outcome `Outcome` (the same four
variants), every write fallible, and the config a derived `FunctionConfig`
whose `Expr` fields compile themselves. See
[Migrating from 3.x](../migration/from-3.md).
