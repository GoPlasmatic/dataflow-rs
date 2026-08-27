# Secrets

A workflow sometimes needs a value the engine must never record — a signing
key, a partner token, a webhook secret. This page is about the one place such
a value can live.

## The problem

`Message.context` is one object, `{data, metadata, temp_data}`, and it plays
two roles at once. Every expression evaluates against it, and it is also
exactly what the engine records: `Serialize for Message` writes it, every
[trace](../core-concepts/engine.md#execution-tracing) step snapshots it, and a
`map` task clones it once per mapping when mapping contexts are on.

So "what a workflow may read" and "what the engine records" are the same
decision. For almost every value that is right. For a signing key it is
exactly wrong — and there is no way to say so from inside the context.
`TraceOptions::redact_paths` prunes named subtrees *after* the fact, which is
the tool you reach for when a value should not have been there in the first
place.

## The store

Secrets do not go in the context at all. They go in a store on the engine,
and expressions reach them through one reserved operator:

```rust
# use dataflow_rs::{Engine, Message, Workflow};
# use serde_json::json;
# async fn run() -> dataflow_rs::Result<()> {
let workflow = Workflow::from_json(r#"{
    "id": "verify", "name": "verify", "priority": 0,
    "condition": {
        "==": [ { "var": "metadata.headers.x-token" }, { "secret": "webhook_token" } ]
    },
    "tasks": [
        { "id": "accept", "name": "accept", "function": {
            "name": "map",
            "input": { "mappings": [ { "path": "data.accepted", "logic": true } ] } } }
    ]
}"#)?;

let engine = Engine::builder()
    .with_secrets_json(&json!({
        "webhook_token": "tok-…",
        "partner": { "hmac": "…" }          // nested is fine: {"secret": "partner.hmac"}
    }))
    .with_workflow(workflow)
    .build()?;

let mut message = Message::builder()
    .metadata_json(&json!({ "headers": { "x-token": "tok-…" } }))
    .build();
engine.process_message(&mut message).await?;
assert_eq!(message.data()["accepted"], json!(true).into());
# Ok(()) }
```

`with_secrets` takes the *resolved values* — the host owns resolution, whether
that is an environment variable, a vault call, or a file. The store must be an
object; nested objects are allowed so a host can namespace, and a dotted name
walks into them.

`{"secret": "name"}` works anywhere JSONLogic runs on this engine: workflow and
task conditions, `validation` rules, `filter`, a custom handler's
[`Template`](./custom-functions.md) fields, the integration configs'
`path_logic` / `body_logic` / `key_logic` / `value_logic`, and a handler's own
`ctx.eval(..)`. A handler configured with a key *name* rather than an
expression reads it directly:

```rust,ignore
let key = ctx.secret(&input.key_name);   // Option<&OwnedDataValue>
```

## The guarantee

A secret cannot appear in `Serialize for Message`, in an `ExecutionTrace`
snapshot, in a `mapping_contexts` clone, or in anything a host derives from a
message — because the store is never part of a `Message`. There is nothing to
exclude. That is a property of the types, not of a code path, and the crate
pins it with a test: a workflow reads a secret from a condition, a validation
rule, a filter and a `Template`, runs under `TraceOptions::default()`, and the
serialized trace and message are checked for the value.

Two things follow from that shape:

- **No hot-path cost.** Nothing about evaluation changes for an expression
  that does not invoke the operator. An engine with a store and a workflow
  that never reads it runs exactly as before.
- **`Debug` on the store prints names with the values masked**, and the store
  implements neither `Serialize` nor `Clone`.

## What a secret may not do

Placement does not stop a workflow *copying* a secret into a recorded root:

```json
{ "path": "data.sig", "logic": { "secret": "partner_key" } }
```

Rather than try to tell a verbatim copy from a derived value — there is no
principled static line between the two, and `cat`, `substr` and `if` all copy
— the rule is blunt. **An expression whose result the engine writes to the
message or emits to a log may not read a secret at all**: a `map` mapping, a
`log` message, a `log` field. That holds even through a custom operator, and
for a dynamic name (`{"secret": {"var": "…"}}`) as much as a literal one.

The check runs at authoring time and at construction, from one implementation:

| Code | Fires when |
|---|---|
| `SECRET_IN_MESSAGE_WRITE` | A `map` mapping or `log` expression reads a secret |
| `UNKNOWN_SECRET` | An expression names a secret the engine does not declare |

`Engine::build()` refuses a workflow with either; `check_workflow` reports them
with the task id and a path such as `function.input.mappings[1].logic`:

```rust
use dataflow_rs::{Engine, IssueCode, Workflow};
use serde_json::json;

let leaky = Workflow::from_json(r#"{
    "id": "w", "name": "w", "priority": 0,
    "tasks": [ { "id": "sign", "name": "sign", "function": {
        "name": "map",
        "input": { "mappings": [ { "path": "data.sig", "logic": { "secret": "k" } } ] } } } ]
}"#).unwrap();

let builder = Engine::builder().with_secrets_json(&json!({ "k": "…" }));
let issues = builder.check_workflow(&leaky);
assert_eq!(issues[0].code, IssueCode::SecretInMessageWrite);
assert_eq!(issues[0].path.as_deref(), Some("function.input.mappings[0].logic"));
assert!(builder.with_workflow(leaky).build().is_err());
```

Derived values — an HMAC over the body, a signed URL — belong in a custom
handler, which reads the key through a `Template` and writes only the result:

```rust
# use async_trait::async_trait;
# use dataflow_rs::engine::functions::AsyncFunctionHandler;
# use dataflow_rs::{Result, TaskContext, TaskOutcome, Template, TemplateCompiler};
#[derive(serde::Deserialize)]
struct SignInput {
    key: Template,                       // {"secret": "partner.hmac"} in the workflow
    body: Template,                      // {"var": "data.body"}
}

struct Sign;

#[async_trait]
impl AsyncFunctionHandler for Sign {
    type Input = SignInput;

    fn compile_input(input: &mut SignInput, c: &TemplateCompiler) -> Result<()> {
        input.key.compile(c, "key")?;
        input.body.compile(c, "body")
    }

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &SignInput) -> Result<TaskOutcome> {
        let key: String = input.key.eval_into(ctx)?;
        let body: String = input.body.eval_into(ctx)?;
        let signature = hmac_hex(&key, &body);
        ctx.set("data.signature", signature.into());   // derived, not the key
        Ok(TaskOutcome::Success)
    }
}
# fn hmac_hex(_key: &str, body: &str) -> String { body.len().to_string() }
```

The contract for a handler author is one line: **a handler must not write a
secret-derived value into the message.** The engine cannot see past the
handler boundary; whether `key` stays unrecorded from here is the handler's
business.

## Unknown names

A literal name the engine does not declare fails `build()` — a typo is caught
before the first message, and nothing that was working can break, since the
name never resolved. A dynamic name that resolves to nothing fails at
evaluation: a condition evaluates `false`, a `validation` rule records
`EVALUATION_ERROR`, a `Template::eval` returns `Err`. It is never `null` —
signing with an empty key silently is the one outcome worse than an error.

Error text names the key, never a value.

## Limits, stated plainly

- **Static, not taint.** The check refuses expressions the engine itself
  records. A handler can still leak; see the contract above.
- **Other operators' errors.** A datalogic operator that fails may echo its
  operands in the message (the `datetime` family does). Do not feed a secret
  to an operator whose failure formats its input; the `secret` operator's own
  errors never do.
- **In-process memory is trusted.** A value is copied into the evaluation
  arena when read and is not zeroized afterwards.
- **Process-wide.** One store per engine, fixed at `build()`. It is carried
  across [`with_new_workflows`](../core-concepts/engine.md#enginewith_new_workflowsworkflows),
  so rotation is a rebuild.
- **`secret` is a reserved operator name.** Registering a host operator under
  it fails `build()` — otherwise adding a store later would silently shadow it.
  `Engine::operator_names()` lists it on every engine.
