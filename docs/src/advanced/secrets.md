# Secrets

A workflow sometimes needs a value the engine must never record: a signing
key, a partner token, a webhook secret. This page covers the one place such a
value can live.

## The problem

A message's context is one object, `{data, metadata, temp_data}`, and it
plays two roles at once. Every expression evaluates against it, and it is
also what the engine records: `Serialize for Message` writes it, a trace
starts from it and records every change to it, and every audit entry holds
the values a task wrote.

So "what a workflow may read" and "what the engine records" are the same
decision. For almost every value that is right. For a signing key it is
wrong, and nothing inside the context can say so. `TraceOptions::redact`
masks named paths in a trace *after* the fact, which is the tool you reach
for when a value should not have been there in the first place.

## The store

Secrets do not go in the context at all. They go in a `Secrets` store on the
expression engine, and expressions reach them through one reserved operator:

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::{ExprEngine, Secrets};
use dataflow_rs::{Engine, Message, Registry};
use serde_json::json;

// The host owns resolution: an environment variable, a vault, a file. Here
// the webhook accepts the current token and the next one, mid-rotation.
let store = Secrets::new(OwnedDataValue::from(&json!({
    "webhook_tokens": ["tok-7f2a", "tok-next-91c0"],
    "partner": {"hmac": "hmac-3d4e"}   // nested is fine: {"secret": "partner.hmac"}
})))
.unwrap();
let exprs = ExprEngine::builder().secrets(store).build().unwrap();

let engine = Engine::builder()
    .registry(Registry::new(exprs))
    .workflow_json(&json!({
        "id": "verify",
        "condition": {
            "in": [{"var": "metadata.headers.x-token"}, {"secret": "webhook_tokens"}]
        },
        "tasks": [
            {"id": "accept", "function": {"name": "map", "input": {
                "mappings": [{"path": "data.accepted", "logic": true}]}}}
        ]
    }))
    .build()
    .unwrap();

let mut message = Message::from_parts(
    OwnedDataValue::Null,
    OwnedDataValue::Null,
    OwnedDataValue::from(&json!({"headers": {"x-token": "tok-7f2a"}})),
);
engine.run(&mut message).await;
assert_eq!(message.to_json()["context"]["data"]["accepted"], json!(true));

// The condition read the whole list, and the message carries none of it:
// the token it holds is the one the request itself brought in.
let wire = serde_json::to_string(&message).unwrap();
assert!(!wire.contains("tok-next-91c0") && !wire.contains("hmac-3d4e"));
assert!(!wire.contains("secret"));
# });
```

`Secrets::new` takes the *resolved values*. The store must be an object
(anything else is a `Config` error); nested objects are allowed so a host can
namespace, and a dotted name walks into them. The `ExprEngine` holding the
store is what a `Registry` compiles against, so every workflow the registry's
engine runs shares one store.

`{"secret": "name"}` works anywhere JSONLogic runs on this engine: workflow,
group and task conditions, `validation` rules, `filter`, and any field of a
host function's config, which is usually where a credential belongs. A host
function configured with a key *name* rather than an expression reads it
directly:

```rust
# use dataflow_rs::Ctx;
# fn _demo(ctx: &Ctx<'_>, key_name: &str) {
let key = ctx.secret(key_name);   // Option<&OwnedDataValue>
# let _ = key;
# }
```

## The guarantee

A secret cannot appear in `Serialize for Message`, in a trace's `initial`
context or any of its changes, in an audit entry, or in anything a host
derives from a message, because the store is never part of a `Message`.
There is nothing to exclude. The types enforce this, so no code path has to,
and the crate pins it with a test that reads a secret from every place one
is allowed, runs under every trace policy, and searches every surface a host
can observe for the value.

Three things follow from that shape:

- **No hot-path cost.** Nothing about evaluation changes for an expression
  that does not invoke the operator.
- **`Debug` on the store prints names with the values masked**, and the
  store implements neither `Serialize` nor `Clone`. `Secrets::names` lists
  the top-level keys, never a value.
- **A layer reads the input without the secret.** `Ctx::input`, the config
  resolved for a call as a logging or dry-run layer reads it, gives `null`
  for every field whose expression reads a secret, and never evaluates it,
  so not even its error can carry the value. The function's own
  `Ctx::resolve` is the unredacted counterpart.

## What a secret may not do

Placement does not stop a workflow *copying* a secret into a recorded root:

```json
{ "path": "data.sig", "logic": { "secret": "partner_key" } }
```

No principled static line separates a verbatim copy from a derived value
(`cat`, `substr` and `if` all copy), so the rule is blunt. **An expression
whose result the engine records may not read a secret at all.** That holds
for a dynamic name (`{"secret": {"var": "…"}}`) as much as a literal one.

Every parameter is JSONLogic, so the rule covers destinations as well as
values. A path is recorded in `Change.path` and on the audit trail, and both
are serialized along with the value written there:

| Parameter | Why it is recorded |
|---|---|
| `map`: `logic` | The value written to the message |
| `map`: `path` | The destination, recorded in `Change.path` and the audit trail |
| `validation`: `message` | Rendered into `Message::errors`. A rule may *test* a secret in its `logic`; it may not *report* one |
| `log`: `message`, `fields.*` | Emitted to the log |
| `parse_*`: `source`, `target` | Name where the engine itself reads and writes |
| `publish_*`: `source`, `target` | Same |
| `publish_xml`: `root_element` | Written into the serialized document that lands in `data.{target}` |
| Any `Output` of a host function's config | A destination, recorded in every `Change` written through it |
| A host function's `#[config(records)]` field | The function declared that the engine records its result |

Everything else handed to a *host function* may read a secret, because what
happens to it from there is the function's business: every other field of
its config, and any condition, which yields a boolean rather than a recorded
value. An `Interpolated` document may not hold a `{"secret": …}` node at all
(`SECRET_IN_DOCUMENT`): a document is never evaluated, so the secret would
be handed on as written.

The check runs at authoring time and at build, from one implementation:

| Code | Fires when |
|---|---|
| `SECRET_IN_MESSAGE_WRITE` | Any recorded parameter above reads a secret |
| `UNKNOWN_SECRET` | An expression names, literally, a secret the engine does not declare |

Both are Rejected: `Engine::builder().build()` refuses a workflow with
either, and `Compiler::check` reports them at the authored path:

```rust
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::expr::{ExprEngine, Secrets};
use dataflow_rs::program::{CompileOptions, IssueCode};
use dataflow_rs::Registry;
use serde_json::json;

let store = Secrets::new(OwnedDataValue::from(&json!({"k": "v"}))).unwrap();
let exprs = ExprEngine::builder().secrets(store).build().unwrap();
let compiler = Registry::new(exprs).compiler(CompileOptions::default());

let leaky = parse(
    &json!({"id": "w", "tasks": [{"id": "sign", "function": {"name": "map", "input": {
        "mappings": [{"path": "data.sig", "logic": {"secret": "k"}}]}}}]}),
    ParseOptions::default(),
)
.def
.unwrap();
let issues = compiler.check(&leaky);
assert_eq!(issues[0].code, IssueCode::SecretInMessageWrite);
assert_eq!(issues[0].path.to_string(), "tasks[0].function.input.mappings[0].logic");
assert!(compiler.compile_one(&leaky).is_err());

let undeclared = parse(
    &json!({"id": "w", "condition": {"==": [{"var": "data.t"}, {"secret": "nope"}]},
            "tasks": [{"id": "t", "function": {"name": "map", "input": {"mappings": []}}}]}),
    ParseOptions::default(),
)
.def
.unwrap();
let issues = compiler.check(&undeclared);
assert_eq!(issues[0].code, IssueCode::UnknownSecret);
assert_eq!(issues[0].path.to_string(), "condition");
```

Derived values (an HMAC over the body, a signed URL) belong in a host
function, which reads the key through an `Expr` and writes only the result:

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::{ExprEngine, Output, Secrets};
use dataflow_rs::function::{FunctionConfig, testing};
use dataflow_rs::{Ctx, Expr, Function, Message, Outcome, Result};
use serde_json::json;

#[derive(FunctionConfig)]
struct SignConfig {
    key: Expr<String>,     // {"secret": "partner.hmac"} in the workflow
    body: Expr<String>,    // {"var": "data.body"}
    #[output(default = "data.signature")]
    output: Output,
}

struct Sign;

impl Function for Sign {
    type Config = SignConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &SignConfig) -> Result<Outcome> {
        let key = ctx.eval(&cfg.key)?;
        let body = ctx.eval(&cfg.body)?;
        // What a layer would see: the secret-reading field is `null`.
        assert_eq!(ctx.input()?.get("key"), Some(&OwnedDataValue::Null));
        let signature = hmac_hex(&key, &body);
        ctx.emit(&cfg.output, OwnedDataValue::from(&json!(signature)))?;   // derived, not the key
        Ok(Outcome::Ok)
    }
}
# fn hmac_hex(key: &str, body: &str) -> String { format!("{}:{}", body.len(), key.len()) }

let store = Secrets::new(OwnedDataValue::from(&json!({"partner": {"hmac": "hmac-91c0"}}))).unwrap();
let exprs = ExprEngine::builder().secrets(store).build().unwrap();
let data = OwnedDataValue::from(&json!({"body": "hello"}));
let called = testing::call(
    Sign,
    json!({"key": {"secret": "partner.hmac"}, "body": {"var": "data.body"}}),
)
.exprs(exprs)
.message(Message::from_parts(OwnedDataValue::Null, data, OwnedDataValue::Null))
.await;
assert_eq!(called.outcome.unwrap(), Outcome::Ok);
assert_eq!(called.message.to_json()["context"]["data"]["signature"], json!("5:9"));
# });
```

The contract for a function author is one line: **a function must not write
a secret-derived value into the message.** The engine cannot see past the
function boundary; whether `key` stays unrecorded from here is the function's
business.

## Unknown names

A literal name the engine does not declare is refused at build, so a typo
surfaces before the first message. Nothing that was working can break, since
the name never resolved. A dynamic name that resolves to nothing fails at
evaluation: a condition evaluates `false`, a `validation` rule records an
error, a host function's `Ctx::eval` returns `Err`. It is never `null`:
silently signing with an empty key is worse than an error.

Error text names the key, never a value.

## Limits, stated plainly

- **Static, not taint.** The check refuses expressions the engine itself
  records. A function can still leak; see the contract above.
- **Other operators' errors.** A datalogic operator that fails may echo its
  operands in the message (the `datetime` family does). Do not feed a secret
  to an operator whose failure formats its input; the `secret` operator's own
  errors never do.
- **In-process memory is trusted.** A value is copied into the evaluation
  arena when read and is not zeroized afterwards.
- **One store per expression engine, fixed when it is built.** A registry and
  every program compiled from it share it, and a reload through
  `engine.compiler()` keeps it. Rotation is a new `ExprEngine`, a new
  `Registry`, and a recompile of the definitions.
- **`secret` is a reserved operator name.** Registering a host operator under
  it fails `ExprEngineBuilder::build`; otherwise adding a store later would
  silently shadow it. `ExprEngine::operators` lists it on every engine, with
  or without a store, so `{"secret": "x"}` on an engine with no store is an
  error, never literal data.
