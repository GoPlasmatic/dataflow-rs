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
        // Resolve cfg.connector, evaluate cfg.compiled_path_logic if set,
        // make the call, merge response into ctx via cfg.response_path…
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
| `method` | string | No | `GET` (default), `POST`, `PUT`, `PATCH`, `DELETE` |
| `path` | string | No | Static request path |
| `path_logic` | JSONLogic | No | Computed path; pre-compiled at startup |
| `headers` | object | No | Static request headers |
| `body` | any | No | Static request body |
| `body_logic` | JSONLogic | No | Computed body; pre-compiled at startup |
| `response_path` | string | No | Dot-path to merge response into the message context |
| `timeout_ms` | u64 | No | Request timeout in milliseconds (default: `30000`) |

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
