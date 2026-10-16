# Log Function

`log` emits a log line through the Rust [`log`](https://docs.rs/log) crate,
with a message and structured fields computed from the message. It writes
nothing to the context and always returns `200`.

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
| `level` | string | No | `trace`, `debug`, `info` (the default), `warn` or `error` |
| `message` | JSONLogic | Yes | The line's text |
| `fields` | object | No | Structured fields, each JSONLogic, emitted in key order |

## The line

The line is `message [key=value, ...]`, the fields sorted by key. A result
that is a string is written as it is; anything else as compact JSON. Over
`{"order": {"id": "ORD-7", "total": 99.5, "items": [1, 2]}, "customer": {"name": "Ada"}}`,

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

emits, at `info` under the target `dataflow::log`:

```text
Processing order ORD-7 for 99.5 [customer=Ada, items=[1,2], order_id=ORD-7, total=99.5]
```

An expression that fails to evaluate is written as `<eval error>`, in the
message or in a field, and the failure is logged at `error` by the engine
itself. The task still returns `200`:

```text
<eval error> [x=<eval error>]
```

## Levels

| Level | Use |
|-------|-----|
| `trace` | Very detailed tracing: every value on the way through |
| `debug` | Intermediate state while developing a workflow |
| `info` | Processing milestones |
| `warn` | Unusual but handled states |
| `error` | Failures the workflow handled |

A level the logger filters out costs nothing: when `dataflow::log` is not
enabled at the task's level, no expression is evaluated and the call returns
`200`. A `trace` task under an `info` logger does no work at all.

## The target

Every line is emitted under the target `dataflow::log`, so a logging
configuration can address it on its own. With `env_logger`:

```bash
RUST_LOG=dataflow::log=info cargo run
```

Or in code:

```rust
env_logger::Builder::new()
    .filter_module("dataflow::log", log::LevelFilter::Debug)
    .init();
```

The engine's own lines, such as a `map` mapping's failure or a `log`
expression's evaluation error, go under the engine's module targets, not
under `dataflow::log`.

## Examples

### A static message

```json
{
    "id": "log_start",
    "function": {"name": "log", "input": {"message": "Starting order processing"}}
}
```

### A warning on an edge case

A task `condition` keeps the line for the messages it is about:

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

### Debugging a context

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

The `data` field writes the whole of `data` as compact JSON.

## Secrets

A log line leaves the engine, so neither `message` nor any field may read
`{"secret": "name"}`: the definition is refused with
`SECRET_IN_MESSAGE_WRITE` at the expression. See
[Secrets](../advanced/secrets.md).

## Notes

- `log` never modifies the message and never fails: every call records
  `200` with no changes.
- The expressions are compiled when the engine is built; a static message
  folds to a constant.
- The fields are a `BTreeMap`, so they are emitted sorted by key whatever
  order they were written in.
