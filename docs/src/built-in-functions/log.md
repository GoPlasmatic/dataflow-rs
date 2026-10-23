# Log Function

The `log` function emits structured log messages through the Rust [`log`](https://docs.rs/log) crate, with a dynamic message string and optional key-value fields evaluated against the message context. It never modifies the context and always succeeds with status `200`.

```json
{
    "function": {
        "name": "log",
        "input": {
            "level": "info",
            "message": {"cat": ["Processing order ", {"var": "data.order.id"}]},
            "fields": {
                "order_id": {"var": "data.order.id"},
                "total": {"var": "data.order.total"}
            }
        }
    }
}
```

## Configuration

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `level` | string | No | Log level: `"trace"`, `"debug"`, `"info"` (default), `"warn"`, or `"error"` |
| `message` | JSONLogic | Yes | Expression computing the log message text |
| `fields` | object | No | Map of structured field names to JSONLogic expressions (emitted sorted by key) |

## Output Format

Each log entry is formatted as `message [key=value, ...]` with `fields` sorted alphabetically by key. String values are formatted directly, while non-string values (numbers, booleans, arrays, objects) are serialized as compact JSON.

For example, given `data = {"order": {"id": "ORD-7", "total": 99.5, "items": [1, 2]}, "customer": {"name": "Ada"}}`:

```json
{
    "level": "info",
    "message": {"cat": ["Processing order ", {"var": "data.order.id"}, " for ", {"var": "data.order.total"}]},
    "fields": {
        "order_id": {"var": "data.order.id"},
        "customer": {"var": "data.customer.name"},
        "total": {"var": "data.order.total"},
        "items": {"var": "data.order.items"}
    }
}
```

emits the following log line at `info` level under the `dataflow::log` target:

```text
Processing order ORD-7 for 99.5 [customer=Ada, items=[1,2], order_id=ORD-7, total=99.5]
```

If an expression in `message` or `fields` fails to evaluate, `"<eval error>"` is written in its place, the evaluation error is logged at `error` level by the engine, and the `log` task still finishes with status `200`:

```text
<eval error> [x=<eval error>]
```

## Log Levels

| Level | Typical Use |
|-------|-----|
| `trace` | Detailed step-by-step values during deep debugging |
| `debug` | Intermediate state while developing workflows |
| `info` | Key workflow milestones |
| `warn` | Unexpected but recoverable conditions |
| `error` | Handled errors or fallback paths |

> **Zero overhead when filtered out:** Before evaluating `message` or `fields`, `log` checks `log::log_enabled!` for the task's level on target `dataflow::log`. When that level is disabled by your logger configuration (for example, a `debug` task when running at `info` level), no expressions are evaluated.

## Configuring Your Logger (`dataflow::log` Target)

All workflow `log` tasks emit under the `dataflow::log` target so you can filter workflow logs independently from internal engine logs.

With `env_logger` via environment variable:

```bash
RUST_LOG=dataflow::log=info cargo run
```

Or in Rust code:

```rust
env_logger::Builder::new()
    .filter_module("dataflow::log", log::LevelFilter::Debug)
    .init();
```

## Examples

### Static Log Message

```json
{
    "id": "log_start",
    "function": {"name": "log", "input": {"message": "Starting order processing"}}
}
```

### Conditional Warning

Combine `log` with a task `condition` to log only when an edge case occurs:

```json
{
    "id": "warn_missing",
    "condition": {"!": {"var": "data.shipping_address"}},
    "function": {
        "name": "log",
        "input": {
            "level": "warn",
            "message": {"cat": ["Order ", {"var": "data.order.id"}, " has no shipping address"]}
        }
    }
}
```

### Dumping Context State for Debugging

```json
{
    "id": "debug_state",
    "function": {
        "name": "log",
        "input": {
            "level": "debug",
            "message": "Current state",
            "fields": {
                "data": {"var": "data"},
                "has_email": {"!!": {"var": "data.email"}}
            }
        }
    }
}
```

## Secrets Protection

Because log lines leave the engine, neither `message` nor any expression in `fields` may reference `{"secret": "name"}`—compilation rejects the workflow at build time with `SECRET_IN_MESSAGE_WRITE`. See [Secrets](../advanced/secrets.md).
