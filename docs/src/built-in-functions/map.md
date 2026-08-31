# Map Function

The `map` function transforms and reorganizes data using JSONLogic expressions.

## Overview

The map function:

- Evaluates JSONLogic expressions against message context
- Assigns results to specified paths
- Supports nested path creation
- Tracks changes for audit trail

## Basic Usage

```json
{
    "function": {
        "name": "map",
        "input": {
            "mappings": [
                {
                    "path": "data.full_name",
                    "logic": {"cat": [{"var": "data.first_name"}, " ", {"var": "data.last_name"}]}
                }
            ]
        }
    }
}
```

## Configuration

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `mappings` | array | Yes | List of mapping operations |

### Mapping Object

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `path` | string \| JSONLogic | Yes | Target path (e.g., `"data.user.name"`). Since 3.9 it may be an expression that computes the destination per message — see [Computed Destinations](#computed-destinations) |
| `logic` | JSONLogic | Yes | Expression to evaluate |

## Path Syntax

### Dot Notation

Access and create nested structures:

```json
{"path": "data.user.profile.name", "logic": "John"}
```

Creates: `{"data": {"user": {"profile": {"name": "John"}}}}`

### Numeric Field Names

Use `#` prefix for numeric keys:

```json
{"path": "data.items.#0", "logic": "first item"}
```

Creates: `{"data": {"items": {"0": "first item"}}}`

### Root Field Assignment

Assigning to root fields (`data`, `metadata`, `temp_data`) merges objects:

```json
{"path": "data", "logic": {"new_field": "value"}}
```

Merges into existing data rather than replacing it.

### Computed Destinations

A destination is JSONLogic like every other parameter, so one mapping can write
somewhere different for each message:

```json
{
    "path": {"cat": ["data.accounts.", {"var": "data.id"}, ".balance"]},
    "logic": {"var": "data.amount"}
}
```

The static spelling above is a plain string, which *is* JSONLogic for itself —
it folds to a constant at `Engine::builder().build()` and keeps the precomputed
path split the write loop has always used. Only a destination that actually
reads the message pays to be split per write, so nothing changes for the
ordinary case.

A computed destination is recorded in `Change.path` and on the audit trail, so
it may not read a secret — the same rule as the value below.

## JSONLogic Expressions

### Copy Value

```json
{"path": "data.copy", "logic": {"var": "data.original"}}
```

### Static Value

```json
{"path": "data.status", "logic": "active"}
```

### String Concatenation

```json
{
    "path": "data.greeting",
    "logic": {"cat": ["Hello, ", {"var": "data.name"}, "!"]}
}
```

### Conditional Value

```json
{
    "path": "data.tier",
    "logic": {"if": [
        {">=": [{"var": "data.points"}, 1000]}, "gold",
        {">=": [{"var": "data.points"}, 500]}, "silver",
        "bronze"
    ]}
}
```

### Arithmetic

```json
{
    "path": "data.total",
    "logic": {"*": [{"var": "data.price"}, {"var": "data.quantity"}]}
}
```

### Array Operations

```json
{
    "path": "data.count",
    "logic": {"reduce": [
        {"var": "data.items"},
        {"+": [{"var": "accumulator"}, 1]},
        0
    ]}
}
```

### Literal Objects

A mapping's `logic` is evaluated in templating mode, so a multi-key object is an
output template and a *single*-key object whose key names an operator is that
operator. Prefix the key with `$` to emit it as data instead:

```json
{"path": "data.filter", "logic": {"$cat": ["a", "b"]}}
```

That writes the object `{"cat": ["a", "b"]}`; without the `$` it would write the
string `"ab"`. Exactly one prefix is stripped from **every** template key, not
only from keys that collide with an operator, so a mapping that emits a key
genuinely starting with `$` must double it — `{"$$oid": …}` writes
`{"$oid": …}`. A key naming no operator needs no escape at all:
`{"result": {"var": "data.x"}}` already writes `{"result": …}`.

See [Literal keys and the `$` escape](../advanced/jsonlogic.md#literal-keys-and-the--escape)
for the full table, and `Engine::template_key_escape()` if a tool needs the
prefix rather than hardcoding it.

### Secrets Are Refused

A mapping's result is written to the message, and the message is what the
engine records. So a mapping may not read `{"secret": "name"}` at all — not
verbatim, not through `cat` or a custom operator, not with a dynamic name.
`Engine::build()` rejects it with `SECRET_IN_MESSAGE_WRITE`, and
`check_workflow` reports it at `function.input.mappings[i].logic` — or at
`…[i].path`, since a computed destination is recorded too. Compute a
derived value (an HMAC, a signed URL) in a
[custom handler](../advanced/custom-functions.md) that reads the key through a
`Template`; see [Secrets](../advanced/secrets.md).

## Null Handling

If a JSONLogic expression evaluates to `null`, the mapping is skipped:

```json
// If data.optional doesn't exist, this mapping is skipped
{"path": "data.copy", "logic": {"var": "data.optional"}}
```

## Sequential Mappings

Mappings execute in order, allowing later mappings to use earlier results:

```json
{
    "mappings": [
        {
            "path": "temp_data.full_name",
            "logic": {"cat": [{"var": "data.first"}, " ", {"var": "data.last"}]}
        },
        {
            "path": "data.greeting",
            "logic": {"cat": ["Hello, ", {"var": "temp_data.full_name"}]}
        }
    ]
}
```

## Try It

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"map_demo","name":"Map Demo","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"transform","name":"Transform","function":{"name":"map","input":{"mappings":[{"path":"data.full_name","logic":{"cat":[{"var":"data.input.first_name"}," ",{"var":"data.input.last_name"}]}},{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"data.full_name"},"!"]}},{"path":"data.is_adult","logic":{">=": [{"var":"data.input.age"},18]}},{"path":"temp_data.processed_at","logic":"2024-01-01T00:00:00Z"}]}}}]}]' data-payload='{"first_name":"John","last_name":"Doe","age":25}'>
</div>

## Common Patterns

### Copy Between Contexts

```json
// Copy from data to metadata
{"path": "metadata.user_id", "logic": {"var": "data.id"}}

// Copy from data to temp_data
{"path": "temp_data.original", "logic": {"var": "data.value"}}
```

### Default Values

```json
{
    "path": "data.name",
    "logic": {"if": [
        {"!!": {"var": "data.name"}},
        {"var": "data.name"},
        "Unknown"
    ]}
}
```

### Computed Fields

```json
{
    "path": "data.subtotal",
    "logic": {"*": [{"var": "data.price"}, {"var": "data.quantity"}]}
}
```

## Best Practices

1. **Use temp_data** - Store intermediate results in temp_data
2. **Order Matters** - Place dependencies before dependent mappings
3. **Check for Null** - Handle missing fields with `if` or `!!` checks
4. **Merge Root Fields** - Use root assignment to merge, not replace
