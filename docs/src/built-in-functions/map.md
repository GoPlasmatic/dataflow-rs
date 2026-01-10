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
| `path` | string | Yes | Target path (e.g., "data.user.name") |
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

<div class="playground-widget" data-workflows='[{"id":"map_demo","name":"Map Demo","tasks":[{"id":"transform","name":"Transform","function":{"name":"map","input":{"mappings":[{"path":"data.full_name","logic":{"cat":[{"var":"data.first_name"}," ",{"var":"data.last_name"}]}},{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"data.full_name"},"!"]}},{"path":"data.is_adult","logic":{">=": [{"var":"data.age"},18]}},{"path":"temp_data.processed_at","logic":"2024-01-01T00:00:00Z"}]}}}]}]' data-message='{"data":{"first_name":"John","last_name":"Doe","age":25},"metadata":{}}'>
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
