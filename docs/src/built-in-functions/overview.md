# Built-in Functions Overview

Dataflow-rs comes with built-in functions for common data processing tasks, covering the complete lifecycle from parsing input to publishing output.

## Available Functions

| Function | Purpose | Modifies Data |
|----------|---------|---------------|
| `parse_json` | Parse JSON from payload into data context | Yes |
| `parse_xml` | Parse XML string into JSON data structure | Yes |
| `map` | Data transformation and field mapping | Yes |
| `validation` | Rule-based data validation | No (read-only) |
| `publish_json` | Serialize data to JSON string | Yes |
| `publish_xml` | Serialize data to XML string | Yes |

## Common Patterns

### Complete Pipeline: Parse → Transform → Validate → Publish

```json
{
    "tasks": [
        {
            "id": "parse_input",
            "function": {
                "name": "parse_json",
                "input": {
                    "source": "payload",
                    "target": "input"
                }
            }
        },
        {
            "id": "transform",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "data.user.fullName", "logic": {"cat": [{"var": "data.input.firstName"}, " ", {"var": "data.input.lastName"}]}}
                    ]
                }
            }
        },
        {
            "id": "validate",
            "function": {
                "name": "validation",
                "input": {
                    "rules": [
                        {"logic": {"!!": {"var": "data.user.fullName"}}, "message": "Full name required"}
                    ]
                }
            }
        },
        {
            "id": "publish",
            "function": {
                "name": "publish_json",
                "input": {
                    "source": "user",
                    "target": "response",
                    "pretty": true
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

### XML Processing Pipeline

```json
{
    "tasks": [
        {
            "id": "parse_xml_input",
            "function": {
                "name": "parse_xml",
                "input": {
                    "source": "payload",
                    "target": "xmlData"
                }
            }
        },
        {
            "id": "transform",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "data.response.status", "logic": "processed"}
                    ]
                }
            }
        },
        {
            "id": "publish_xml_output",
            "function": {
                "name": "publish_xml",
                "input": {
                    "source": "response",
                    "target": "xmlOutput",
                    "root_element": "Response"
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

- [Parse Functions](./parse.md) - JSON and XML parsing
- [Map Function](./map.md) - Data transformation
- [Validation Function](./validation.md) - Rule-based validation
- [Publish Functions](./publish.md) - JSON and XML serialization
