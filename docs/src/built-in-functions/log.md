# Log (Structured Logging)

The `log` function emits structured log records from inside a workflow through the Rust `log` crate. Log messages and fields accept JSONLogic expressions for dynamic content.

## Overview

With `log` you can:

- Emit structured log messages at any point in a workflow
- Use JSONLogic expressions for dynamic message content
- Attach structured fields for machine-readable log data
- Debug data flow without modifying the message

## Configuration

```json
{
    "function": {
        "name": "log",
        "input": {
            "level": "info",
            "message": "JSONLogic expression or static string",
            "fields": {
                "field_name": "JSONLogic expression"
            }
        }
    }
}
```

### Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `level` | string | No | Log level: `trace`, `debug`, `info` (default), `warn`, `error` |
| `message` | JSONLogic | Yes | The log message (evaluated as JSONLogic against message context) |
| `fields` | object | No | Key-value pairs where values are JSONLogic expressions |

## Log Levels

| Level | Use Case |
|-------|----------|
| `trace` | Very detailed debugging (function entry/exit, variable values) |
| `debug` | Debugging information (intermediate processing state) |
| `info` | General informational messages (processing milestones) |
| `warn` | Warning conditions (unusual but not erroneous states) |
| `error` | Error conditions (failures that are handled) |

## Examples

### Simple Static Message

```json
{
    "id": "log_start",
    "name": "Log start",
    "function": {
        "name": "log",
        "input": {
            "level": "info",
            "message": "Starting order processing"
        }
    }
}
```

### Dynamic Message with JSONLogic

```json
{
    "id": "log_order",
    "name": "Log order",
    "function": {
        "name": "log",
        "input": {
            "level": "info",
            "message": {"cat": ["Processing order ", {"var": "data.order.id"}, " for $", {"var": "data.order.total"}]},
            "fields": {
                "order_id": {"var": "data.order.id"},
                "customer": {"var": "data.customer.name"},
                "total": {"var": "data.order.total"}
            }
        }
    }
}
```

### Debug Logging

```json
{
    "id": "debug_state",
    "name": "Debug state",
    "function": {
        "name": "log",
        "input": {
            "level": "debug",
            "message": {"cat": ["Current data state: ", {"var": "data"}]},
            "fields": {
                "has_email": {"!!": {"var": "data.email"}},
                "item_count": {"var": "data.items.length"}
            }
        }
    }
}
```

### Warning on Edge Cases

```json
{
    "id": "warn_missing",
    "name": "Warn missing",
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

## Log Target

Every log message uses the target `dataflow::log`, so you can filter on it in your logging configuration.

Filter via `RUST_LOG` when running:

```bash
RUST_LOG=dataflow::log=info cargo run
```

Or configure the filter in code:

```rust
env_logger::Builder::new()
    .filter_module("dataflow::log", log::LevelFilter::Debug)
    .init();
```

## Notes

- The log function **never modifies the message**; it is read-only
- The log function **never fails**; it always returns status 200 with no changes
- The engine **pre-compiles** all JSONLogic expressions in `message` and `fields` at startup
- If the configured level is **filtered out** for the `dataflow::log` target (e.g. via `RUST_LOG`), the task short-circuits before evaluating any expression, so a disabled log task costs effectively nothing
- If a JSONLogic expression fails to evaluate, the task logs the raw expression value instead
- The `fields` are formatted as `key=value` pairs appended to the log message
- Neither `message` nor any field may read `{"secret": "name"}`. A log line is an exit the engine does not control, so `Engine::build()` rejects it with `SECRET_IN_MESSAGE_WRITE`. See [Secrets](../advanced/secrets.md)
