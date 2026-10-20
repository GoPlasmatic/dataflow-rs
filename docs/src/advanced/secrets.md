# Secrets

Workflows often need sensitive values that must never appear in messages,
audit entries, or traces, such as signing keys, partner tokens, and webhook
secrets. Store these values in the engine's `Secrets` store and read them with
the `{"secret": "name"}` operator.

## Why Context Roots Cannot Hold Secrets

A message's context (`{data, metadata, temp_data}`) is both the evaluation
input for JSONLogic and the recorded output of a run: `Serialize for Message`
writes it out, traces start from it and record every change to it, and audit
entries store the values tasks write. Any secret placed in the context will end
up in serialized messages and traces. `TraceOptions::redact` masks paths in a
trace, but keeping secrets out of the message altogether prevents accidental
exposure across every output channel.

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

The compiler rejects any literal secret name that is missing from the store at
build time (`UNKNOWN_SECRET`), catching typos before any message runs. A
dynamic secret name that fails to resolve produces an evaluation error:
conditions evaluate to `false`, `validation` rules record an error, and
`Ctx::eval` in a host function returns `Err`. Missing secrets never resolve to
`null`.

Error messages include only the secret key name, never its value.

## Boundaries and Limitations

- **Static checks only.** The compiler blocks secret reads in expressions that
  the engine records directly. Host functions must still avoid writing
  secret-derived values into the message.
- **Other operators' error messages.** Some `datalogic-rs` operators (such as
  `datetime`) include operand values when formatting evaluation errors. Do not
  pass secrets into operators that echo their inputs on failure.
- **In-process memory is trusted.** The evaluator copies resolved secrets into
  its arena during evaluation and does not zeroize arena memory afterward.
- **One store per `ExprEngine`.** Every workflow compiled from a `Registry`
  shares its `ExprEngine` and secret store, including across reloads via
  `engine.compiler()`. To rotate secrets, build a new `ExprEngine` and
  `Registry` and recompile the definitions.
- **`secret` is a reserved operator name.** `ExprEngineBuilder::build` rejects
  custom operators named `secret`. `ExprEngine::operators` lists `secret` on
  every engine, so `{"secret": "x"}` on an engine without a configured store
  returns an evaluation error rather than passing through as literal data.
