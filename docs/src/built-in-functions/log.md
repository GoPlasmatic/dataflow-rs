# Log (Structured Logging)

The `log` function provides structured logging within workflows using the Rust `log` crate. Log messages and fields support JSONLogic expressions for dynamic content.

## Overview

The log function allows you to:

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

All log messages are emitted with the target `dataflow::log`, making it easy to filter in your logging configuration:

```rust
// Using env_logger
RUST_LOG=dataflow::log=info cargo run

// Or filter specifically for dataflow logs
env_logger::Builder::new()
    .filter_module("dataflow::log", log::LevelFilter::Debug)
    .init();
```

## Notes

- The log function **never modifies the message** — it is read-only
- The log function **never fails** — it always returns status 200 with no changes
- All JSONLogic expressions in `message` and `fields` are **pre-compiled** at engine startup
- If a JSONLogic expression fails to evaluate, the raw expression value is logged instead
- The `fields` are formatted as `key=value` pairs appended to the log message
