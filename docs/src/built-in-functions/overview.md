# Built-in Functions Overview

Dataflow-rs comes with built-in functions for common data processing tasks.

## Available Functions

| Function | Purpose | Modifies Data |
|----------|---------|---------------|
| `map` | Data transformation and field mapping | Yes |
| `validation` | Rule-based data validation | No (read-only) |

## Common Patterns

### Data Transformation Pipeline

```json
{
    "tasks": [
        {
            "id": "validate",
            "function": {
                "name": "validation",
                "input": {
                    "rules": [
                        {"logic": {"!!": {"var": "data.email"}}, "message": "Email required"}
                    ]
                }
            }
        },
        {
            "id": "transform",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "data.processed", "logic": true}
                    ]
                }
            }
        }
    ]
}
```

### Conditional Transformation

```json
{
    "tasks": [
        {
            "id": "conditional_map",
            "condition": {"==": [{"var": "metadata.type"}, "premium"]},
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "data.discount", "logic": 20}
                    ]
                }
            }
        }
    ]
}
```

## Function Configuration

All functions use this structure:

```json
{
    "function": {
        "name": "function_name",
        "input": {
            // Function-specific configuration
        }
    }
}
```

## Custom Functions

For operations beyond built-in functions, implement the `AsyncFunctionHandler` trait. See [Custom Functions](../advanced/custom-functions.md).

## Learn More

- [Map Function](./map.md) - Data transformation
- [Validation Function](./validation.md) - Rule-based validation
