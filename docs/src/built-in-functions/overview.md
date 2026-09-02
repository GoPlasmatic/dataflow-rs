# Built-in Functions Overview

Dataflow-rs comes with built-in action functions for common data processing tasks, covering the complete lifecycle from parsing input to publishing output.

## Available Functions

| Function | Purpose | Modifies Data |
|----------|---------|---------------|
| `parse_json` | Parse JSON from payload into data context | Yes |
| `parse_xml` | Parse XML string into JSON data structure | Yes |
| `map` | Data transformation and field mapping | Yes |
| `validation` / `validate` | Rule-based data validation | No (read-only) |
| `filter` | Pipeline control flow — halt workflow or skip task | No |
| `log` | Structured logging with JSONLogic expressions | No |
| `publish_json` | Serialize data to JSON string | Yes |
| `publish_xml` | Serialize data to XML string | Yes |

## Every parameter is JSONLogic

Since 3.9 every parameter of every function above is a JSONLogic expression,
including the ones that name a destination — a `map` `path`, a `parse_*` or
`publish_*` `source` and `target`, a `validation` `message`. A JSON literal *is*
JSONLogic for itself, so the static spelling stays exactly what it always was
and costs nothing: it folds to a constant when the engine is built, and only a
parameter that actually reads the message does per-message work.

The one thing this changes for an author is that a single-key object whose key
names an operator evaluates as that operator, so a literal object is written
`{"$cat": …}` — see
[Literal keys and the `$` escape](../advanced/jsonlogic.md#literal-keys-and-the--escape).

In addition, dataflow-rs ships **typed config schemas** for three common
service-layer integrations — `http_call`, `enrich`, and `publish_kafka`.
These are not pre-registered: register an `AsyncFunctionHandler` under the
matching name and the engine handles config validation and JSONLogic
pre-compilation for you. See [Integrations](./integrations.md).

## Common Patterns

### Complete Pipeline: Parse → Transform → Validate → Publish

```json
{
    "tasks": [
        {
            "id": "parse_input",
            "name": "Parse input",
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
            "name": "Transform",
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
            "name": "Validate",
            "halt_on": "failure",
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
            "name": "Publish",
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

`halt_on: "failure"` on the validation is what stops `publish` from running on a
message that failed it — a failing rule records `400`, which `continue_on_error`
does not cover. See [Control Flow](../advanced/control-flow.md#halt_on).

### Conditional Transformation

```json
{
    "tasks": [
        {
            "id": "conditional_map",
            "name": "Conditional map",
            "condition": {"==": [{"var": "data.tier"}, "premium"]},
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
            "name": "Parse XML input",
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
            "name": "Transform",
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
            "name": "Publish XML output",
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
- [Filter Function](./filter.md) - Pipeline control flow (halt/skip)
- [Log Function](./log.md) - Structured logging
- [Publish Functions](./publish.md) - JSON and XML serialization
- [Integrations](./integrations.md) - Typed config for `http_call`, `enrich`, `publish_kafka`
