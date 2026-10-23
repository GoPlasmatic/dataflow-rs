# Secrets

Workflows often need sensitive values—such as signing keys, partner API tokens,
and webhook secrets—that must never appear in serialized messages, audit
entries, or execution traces. Store these values in the engine's `Secrets` store
and read them using the `{"secret": "name"}` operator.

## Why Secrets Belong Outside the Message Context

A message's context (`{data, metadata, temp_data}`) is both the input to
JSONLogic expressions and the recorded output of a run: `Message` serializes its
context directly, execution traces capture the initial context and every change
made to it, and audit entries record the values written by tasks. Any secret
placed in `data`, `metadata`, or `temp_data` will inevitably appear in
serialized messages and traces.

While `TraceOptions::redact` can mask specific paths in a trace, keeping secrets
out of the message context altogether prevents accidental leaks across every
output channel.

## Configuring the `Secrets` Store

Store secrets in a `Secrets` instance on `ExprEngineBuilder`. Workflows can then
access them through the reserved `{"secret": "name"}` operator:

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::{ExprEngine, Secrets};
use dataflow_rs::{Engine, Message, Registry};
use serde_json::json;

// Resolve secret values from your environment, vault, or config file first.
// Here, the webhook accepts both the current token and the next token during rotation.
let store = Secrets::new(OwnedDataValue::from(&json!({
    "webhook_tokens": ["tok-7f2a", "tok-next-91c0"],
    "partner": {"hmac": "hmac-3d4e"}   // Nested objects can be accessed via dotted paths: {"secret": "partner.hmac"}
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

// Even though the condition read the full token list, none of the store's secrets
// appear in the serialized message.
let wire = serde_json::to_string(&message).unwrap();
assert!(!wire.contains("tok-next-91c0") && !wire.contains("hmac-3d4e"));
assert!(!wire.contains("secret"));
# });
```

`Secrets::new` accepts an object of already-resolved secret values (passing a
non-object returns a `Config` error). You can nest objects to organize secrets
by namespace and access them with dotted paths like `{"secret": "partner.hmac"}`.
Because the `Registry` compiles workflows against this `ExprEngine`, all
workflows in the engine share the same secret store.

You can use `{"secret": "name"}` in workflow, group, and task conditions,
`validation` rules, `filter` conditions, and custom host function inputs. If a
host function takes a secret *name* as a string parameter rather than a
JSONLogic expression, it can look up the secret directly via `ctx.secret`:

```rust
# use dataflow_rs::Ctx;
# fn _demo(ctx: &Ctx<'_>, key_name: &str) {
let key = ctx.secret(key_name);   // Option<&OwnedDataValue>
# let _ = key;
# }
```

## Security Guarantees

Because the `Secrets` store is never part of a `Message`, secrets cannot leak
through `Serialize for Message`, trace `initial` snapshots, trace change logs,
or audit entries.

In addition:

- **Zero overhead when unused:** Expressions that do not call `secret` incur no
  runtime cost.
- **Redacted `Debug` output:** Formatting `Secrets` with `Debug` prints secret
  names with their values masked, and `Secrets` intentionally does not implement
  `Serialize` or `Clone`. `Secrets::names()` lists only top-level key names.
- **Automatic redaction for `CallLayer`s:** When a logging or dry-run layer
  calls `ctx.input()`, any field whose expression reads a secret is replaced
  with `null` without being evaluated. Inside the host function itself,
  `ctx.resolve(&cfg)` evaluates all fields including secrets.

## Preventing Secret Leaks

Keeping secrets in a separate store wouldn't help if a workflow simply copied a
secret into the message context:

```json
{ "path": "data.sig", "logic": { "secret": "partner_key" } }
```

Because JSONLogic operators like `cat`, `substr`, and `if` can copy or transform
values in arbitrary ways, dataflow-rs enforces a strict compile-time rule:
**any expression whose result is recorded by the engine is forbidden from
reading a secret.** This applies to both literal secret names and dynamic names
(`{"secret": {"var": "…"}}`).

This rule covers destination paths as well as values, since paths are recorded
in `Change.path` and on the audit trail:

| Parameter | Why Secret Reads Are Blocked |
|---|---|
| `map`: `logic` | The result is written directly into the message context. |
| `map`: `path` | The destination path is recorded in `Change.path` and the audit trail. |
| `validation`: `message` | Recorded in `Message::errors` (a rule may *test* a secret in `logic`, but may not *output* one in `message`). |
| `log`: `message`, `fields.*` | Emitted to application logs. |
| `parse_*` / `publish_*`: `source`, `target` | Determine paths read from and written to the message context. |
| `publish_xml`: `root_element` | Written into the serialized XML stored at `data.{target}`. |
| Any `Output` field of a host function | Recorded as the destination path in every `Change` emitted through it. |
| Any `#[config(records)]` field of a host function | Explicitly marked by the function author as recorded in the message or logs. |

All other expressions—including conditions (which only produce booleans) and
regular `Expr<T>` fields on custom host functions—are allowed to read secrets.
Note that `Interpolated` documents cannot contain `{"secret": …}` nodes at all
(`SECRET_IN_DOCUMENT`), because only `var` placeholders are substituted in
interpolated documents.

The compiler checks secret usage at both authoring time and build time:

| Issue Code | When It Is Reported |
|---|---|
| `SECRET_IN_MESSAGE_WRITE` | Any recorded expression listed above reads from `{"secret": …}`. |
| `UNKNOWN_SECRET` | An expression references a literal secret name that is not present in the `Secrets` store. |

Both issues have `Rejected` severity: `Engine::builder().build()` refuses to
build the workflow, and `Compiler::check` reports the exact authored path:

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

When you need to compute a value derived from a secret (such as an HMAC
signature or a signed URL) and write it to the message, implement that operation
in a custom host function. The host function can evaluate the secret via an
`Expr` field and write only the non-sensitive result to the message:

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::{ExprEngine, Output, Secrets};
use dataflow_rs::function::{FunctionConfig, testing};
use dataflow_rs::{Ctx, Expr, Function, Message, Outcome, Result};
use serde_json::json;

#[derive(FunctionConfig)]
struct SignConfig {
    key: Expr<String>,     // Configured as {"secret": "partner.hmac"} in the workflow
    body: Expr<String>,    // Configured as {"var": "data.body"}
    #[output(default = "data.signature")]
    output: Output,
}

struct Sign;

impl Function for Sign {
    type Config = SignConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &SignConfig) -> Result<Outcome> {
        let key = ctx.eval(&cfg.key)?;
        let body = ctx.eval(&cfg.body)?;
        // Layers calling `ctx.input()` see `null` for the secret-reading field:
        assert_eq!(ctx.input()?.get("key"), Some(&OwnedDataValue::Null));
        let signature = hmac_hex(&key, &body);
        ctx.emit(&cfg.output, OwnedDataValue::from(&json!(signature)))?;   // Writes the signature, never the key
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

## Handling Unknown Secret Names

- **Literal secret names** that do not exist in the `Secrets` store are rejected
  at compile time with `UNKNOWN_SECRET`, catching typos before any messages run.
- **Dynamic secret names** (such as `{"secret": {"var": "data.key_name"}}`) that
  fail to resolve at runtime produce an evaluation error: conditions evaluate to
  `false`, `validation` rules record an error, and `ctx.eval` in a host function
  returns `Err`. Missing secrets never silently resolve to `null`.

Error messages only ever include the secret's key name, never its value.

## Boundaries and Limitations

- **Host function responsibility:** The compiler prevents workflows from writing
  secrets directly into recorded fields, but it cannot inspect Rust code inside
  custom host functions. Ensure your host functions never write raw secrets into
  the message context or error messages.
- **Operator error formatting:** Some extended `datalogic-rs` operators (such as
  `datetime`) include invalid operand values in their error messages when
  parsing fails. Avoid passing secrets into operators that echo their inputs on
  failure.
- **In-process memory:** Resolved secrets are copied into the evaluation arena
  during expression evaluation and are not zeroized afterward.
- **Secret rotation:** Every workflow compiled by a `Registry` shares its
  `ExprEngine` and `Secrets` store (including across reloads via
  `engine.compiler()`). To rotate secrets, construct a new `ExprEngine` and
  `Registry` and recompile your workflows.
- **Reserved operator name:** `secret` is a reserved operator name on every
  `ExprEngine`. Calling `{"secret": "x"}` on an engine without a configured
  `Secrets` store returns an evaluation error rather than passing through as a
  literal JSON object.
