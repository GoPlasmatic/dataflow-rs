# Integration Functions

The `http_call`, `enrich`, and `publish_kafka` functions provide **typed
configuration schemas** for the three most common service-layer integration
patterns. Unlike `map` or `validation`, they do **not** ship with a built-in
handler — the actual I/O is provided by your application via
[`AsyncFunctionHandler`](../advanced/custom-functions.md).

## Why a config schema without an implementation?

The engine itself is I/O-agnostic: it doesn't bundle an HTTP client, a Kafka
producer, or any other transport. But the *shape* of these integrations is
predictable enough that dataflow-rs provides typed config structs so that:

- JSONLogic expressions inside the config (`path_logic`, `body_logic`, `key_logic`, …)
  are **pre-compiled at engine startup** — same fail-loud behaviour as `map` rules
- Misshapen config fails at `Engine::new()`, not at first message
- Your handler receives an already-validated `HttpCallConfig` / `EnrichConfig` /
  `PublishKafkaConfig` — no per-call JSON parse

## How to use them

For each integration variant you want to use, register a handler under the
matching name when building the engine:

```rust,ignore
use dataflow_rs::prelude::*;
use dataflow_rs::HttpCallConfig;
use async_trait::async_trait;

struct HttpCallHandler { /* reqwest::Client, connector registry, etc. */ }

#[async_trait]
impl AsyncFunctionHandler for HttpCallHandler {
    type Input = HttpCallConfig;

    async fn execute(
        &self,
        ctx: &mut TaskContext<'_>,
        cfg: &HttpCallConfig,
    ) -> Result<TaskOutcome> {
        // `resolve_*` applies the logic-then-static fallback for you and
        // evaluates on the worker thread's pooled arena.
        let path = cfg.resolve_path(ctx)?;      // path_logic, else static path
        let body = cfg.resolve_body(ctx)?;      // body_logic, else static body
        let method = cfg.method.as_str();       // canonical token for your client

        // Resolve cfg.connector against your own registry, make the call, then
        // merge the response into ctx at cfg.response_path…
        let _ = (path, body, method);
        Ok(TaskOutcome::Success)
    }
}

let engine = Engine::builder()
    .register("http_call", HttpCallHandler { /* … */ })
    .with_workflow(workflow)
    .build()?;
```

Skip the registration step and any workflow that uses these variants will fail
with `DataflowError::FunctionNotFound("http_call")` at dispatch time.

## Reading a config's dynamic fields

Each integration config pairs a static field with a `*_logic` JSONLogic field, and
the engine pre-compiles the latter at `build()`. Read them through the
`resolve_*` methods rather than the `compiled_*` slots:

| Method | Logic field | Static fallback |
|---|---|---|
| `HttpCallConfig::resolve_path` | `path_logic` | `path` |
| `HttpCallConfig::resolve_body` | `body_logic` | `body` |
| `EnrichConfig::resolve_path` | `path_logic` | `path` |
| `PublishKafkaConfig::resolve_key` | `key_logic` | none — `Ok(None)` |
| `PublishKafkaConfig::resolve_value` | `value_logic` | none — `Ok(None)` |

Three things they guarantee that a hand-rolled read does not:

- **Logic wins over the static field** when both are set, consistently across all
  five.
- **An evaluation failure propagates** as `DataflowError::LogicEvaluation` rather
  than silently falling back to the static value — substituting a different URL
  because an expression errored would hide a real problem.
- **Path and key results are coerced to a plain string** — a number becomes its
  digits, a container its compact JSON — because those values go into a URL or a
  partition key. `resolve_value` deliberately returns `Option<Value>` instead, so a
  producer that serializes unconditionally is not forced through the key's
  coercion and end up with different bytes on the wire.

Each `*_logic` field is a [`Template`](../advanced/custom-functions.md#config-fields-that-are-jsonlogic-template)
— the same type available for your own handler's config. There is no separate
compiled slot to read directly; `resolve_*` is the only supported way to get a
value out of one.

## Detecting a missing handler before it fails

Because these three names deserialize into typed built-in variants, a workflow
that uses one without a registered handler **builds cleanly** — `Engine::new()`
raises nothing, and the failure arrives on the first message. That is deliberate:
a host screening stored workflow definitions one row at a time should not be
stopped from booting by a single unusable row.

To detect the gap instead of discovering it at runtime, classify the name:

```rust
use dataflow_rs::{BuiltinKind, builtin_function_kind};

// Executed by the crate — always runnable, no registration needed.
assert_eq!(builtin_function_kind("map"), Some(BuiltinKind::SelfContained));

// Config schema only — needs a handler registered under the same name.
assert_eq!(
    builtin_function_kind("enrich"),
    Some(BuiltinKind::RequiresHandler),
);

// Not a built-in at all — lands in `FunctionConfig::Custom`.
assert_eq!(builtin_function_kind("my_handler"), None);
```

So a validator that gates workflow authoring can require a registration for
every `RequiresHandler` and `None` name, and accept `SelfContained` names
outright. `BUILTIN_FUNCTION_NAMES` gives the full set if you need to enumerate
it. Prefer these over parsing the text of `FunctionNotFound`, which is a
human-facing diagnostic and may be reworded at any time.

---

## http_call

Issue an HTTP request and optionally merge the response into the message context.

### Configuration

```json
{
    "function": {
        "name": "http_call",
        "input": {
            "connector": "user_service",
            "method": "GET",
            "path_logic": { "cat": ["/users/", {"var": "data.user_id"}] },
            "headers": { "X-Request-Id": "abc" },
            "response_path": "data.user_profile",
            "timeout_ms": 5000
        }
    }
}
```

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `connector` | string | Yes | Named reference resolved by your service layer |
| `method` | string | No | `GET` (default), `POST`, `PUT`, `PATCH`, `DELETE` — uppercase only |
| `path` | string | No | Static request path |
| `path_logic` | JSONLogic | No | Computed path; pre-compiled at startup |
| `headers` | object | No | Static request headers |
| `body` | any | No | Static request body |
| `body_logic` | JSONLogic | No | Computed body; pre-compiled at startup |
| `response_path` | string | No | Dot-path to merge response into the message context. Also accepted as `output` |
| `timeout_ms` | u64 | No | Request timeout in milliseconds (default: `30000`) |

`response_path` accepts `output` as an alias, so a service layer can present one
destination-field name across its whole function catalogue. Supplying **both**
keys is a `duplicate field` error rather than a precedence rule.

The alias is specific to `http_call`. `enrich` names its destination `merge_path`
and `publish_json` / `publish_xml` name theirs `target`; neither takes `output`.

### Unknown fields are rejected

All three integration configs reject keys they do not recognise. A misspelled
field used to parse cleanly and be discarded, so an `http_call` task would make
its request and silently throw the response away — no error at
`Engine::builder().build()`, none at dispatch. Now it fails at parse time:

```text
config for function 'http_call': unknown field `outputs`, expected one of
`connector`, `method`, `path`, `path_logic`, `headers`, `body`, `body_logic`,
`output`, `response_path`, `timeout_ms`
```

Note this fails when the workflow definition is parsed, so a host loading stored
definitions row by row sees one bad row fail its own parse rather than losing the
whole set.

### Converting `method` for your HTTP client

This crate takes no HTTP-client dependency, so your handler converts `HttpMethod`
into whatever type its client uses. `as_str()` gives the canonical token — the
same spelling the config accepts — so the bridge is one line and needs no match:

```rust
use dataflow_rs::HttpMethod;

// e.g. reqwest::Method::from_bytes(m.as_str().as_bytes())
assert_eq!(HttpMethod::Patch.as_str(), "PATCH");
assert_eq!(HttpMethod::Get.to_string(), "GET");

// Retry decisions without a hand-written table.
assert!(HttpMethod::Put.is_idempotent());
assert!(!HttpMethod::Post.is_idempotent());

// The vocabulary an `http_call` task may name, for validating your own
// operator-facing allow-lists against something the compiler keeps honest.
assert_eq!(HttpMethod::ALL.len(), 5);
```

`HttpMethod::ALL` is scoped to what `http_call` accepts. It is deliberately not a
general list of HTTP methods — don't reuse it to validate inbound routes, which
may legitimately accept `HEAD` or `OPTIONS`.

Use `path` **or** `path_logic`, not both. Same for `body` / `body_logic`.

---

## enrich

Fetch external data and merge it into the message context at a specified path.
A specialization of `http_call` aimed at the "look up and attach" pattern.

### Configuration

```json
{
    "function": {
        "name": "enrich",
        "input": {
            "connector": "customer_lookup",
            "method": "GET",
            "path_logic": { "cat": ["/customers/", {"var": "data.customer_id"}] },
            "merge_path": "data.customer",
            "timeout_ms": 5000,
            "on_error": "skip"
        }
    }
}
```

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `connector` | string | Yes | Named reference resolved by your service layer |
| `method` | string | No | HTTP method (default `GET`) |
| `path` | string | No | Static request path |
| `path_logic` | JSONLogic | No | Computed request path |
| `merge_path` | string | Yes | Dot-path where the response is merged into the context |
| `timeout_ms` | u64 | No | Request timeout in milliseconds (default: `30000`) |
| `on_error` | `"fail"` \| `"skip"` | No | Behaviour on lookup failure (default: `fail`) |

`on_error: skip` is useful when enrichment is best-effort and an absent
upstream service shouldn't fail the workflow.

---

## publish_kafka

Emit the message (or a derived value) to a Kafka topic.

### Configuration

```json
{
    "function": {
        "name": "publish_kafka",
        "input": {
            "connector": "events_cluster",
            "topic": "orders.processed",
            "key_logic": { "var": "data.order_id" },
            "value_logic": { "var": "data" }
        }
    }
}
```

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `connector` | string | Yes | Named reference resolved by your service layer |
| `topic` | string | Yes | Target Kafka topic |
| `key_logic` | JSONLogic | No | Computed message key |
| `value_logic` | JSONLogic | No | Computed message value (default: serialize the message) |

The handler decides exactly how to render the produced value — for example,
sending the entire message JSON when `value_logic` is omitted.

---

## Connectors

The `connector` field is a string that your handler resolves into a concrete
client (HTTP client + base URL, Kafka producer + cluster config, …). The
engine does not interpret it. A typical layout:

```rust,ignore
struct HttpCallHandler {
    connectors: HashMap<String, HttpConnector>,  // "user_service" -> &Client + base_url
}
```

This separation keeps secrets out of workflow JSON and lets you swap
endpoints (staging / prod) without touching rule definitions.

## Why typed configs matter

Compared to free-form `Custom` configs:

- **Startup-time validation** — bad config fails at `Engine::new()`
- **Pre-compiled JSONLogic** — `path_logic`, `body_logic`, `key_logic`, `value_logic`
  are all compiled once; the handler reads `Arc<Logic>` from the config and evaluates
  at zero allocation cost in the hot path
- **Stable shape** — the same config struct is shared by every handler in the
  ecosystem, so handlers from different crates can be swapped without rewriting
  workflows
