# Parse Functions

The parse functions convert payload data into structured context data. They are typically used at the start of a workflow to load input data into the processing context.

## parse_json

Extracts JSON data from the payload or data context and stores it in a target field.

### Configuration

```json
{
    "function": {
        "name": "parse_json",
        "input": {
            "source": "payload",
            "target": "input_data"
        }
    }
}
```

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `source` | string | Yes | Path to read from: `payload`, `payload.field`, or `data.field` |
| `target` | string | Yes | Field name in data where the result will be stored |

### Examples

#### Parse Entire Payload

```json
{
    "id": "load_payload",
    "function": {
        "name": "parse_json",
        "input": {
            "source": "payload",
            "target": "request"
        }
    }
}
```

**Input:**
```json
{
    "payload": {"name": "Alice", "age": 30}
}
```

**Result:**
```json
{
    "data": {
        "request": {"name": "Alice", "age": 30}
    }
}
```

#### Parse Nested Payload Field

```json
{
    "id": "extract_body",
    "function": {
        "name": "parse_json",
        "input": {
            "source": "payload.body.user",
            "target": "user_data"
        }
    }
}
```

**Input:**
```json
{
    "payload": {
        "headers": {},
        "body": {
            "user": {"id": 123, "name": "Bob"}
        }
    }
}
```

**Result:**
```json
{
    "data": {
        "user_data": {"id": 123, "name": "Bob"}
    }
}
```

---

## parse_xml

Parses an XML string from the source path, converts it to JSON, and stores it in the target field.

### Configuration

```json
{
    "function": {
        "name": "parse_xml",
        "input": {
            "source": "payload",
            "target": "xml_data"
        }
    }
}
```

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `source` | string | Yes | Path to XML string: `payload`, `payload.field`, or `data.field` |
| `target` | string | Yes | Field name in data where the parsed JSON will be stored |

### XML to JSON Conversion

The XML parser follows these conventions:
- Element names become object keys
- Text content is stored under the element key
- Attributes are preserved in the JSON structure
- Multiple child elements with the same name become arrays

### Examples

#### Parse XML Payload

```json
{
    "id": "parse_xml_request",
    "function": {
        "name": "parse_xml",
        "input": {
            "source": "payload",
            "target": "request"
        }
    }
}
```

**Input:**
```json
{
    "payload": "<user><name>Alice</name><email>alice@example.com</email></user>"
}
```

**Result:**
```json
{
    "data": {
        "request": {
            "name": "Alice",
            "email": "alice@example.com"
        }
    }
}
```

#### Parse Nested XML String

```json
{
    "id": "parse_xml_body",
    "function": {
        "name": "parse_xml",
        "input": {
            "source": "payload.xmlContent",
            "target": "parsed"
        }
    }
}
```

---

## Common Patterns

### Load and Transform Pipeline

```json
{
    "tasks": [
        {
            "id": "load",
            "function": {
                "name": "parse_json",
                "input": {"source": "payload", "target": "input"}
            }
        },
        {
            "id": "transform",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "data.output.name", "logic": {"var": "data.input.name"}}
                    ]
                }
            }
        }
    ]
}
```

### Handle XML API Response

```json
{
    "tasks": [
        {
            "id": "parse_response",
            "function": {
                "name": "parse_xml",
                "input": {"source": "payload.response", "target": "apiResponse"}
            }
        },
        {
            "id": "extract_data",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "data.result", "logic": {"var": "data.apiResponse.result"}}
                    ]
                }
            }
        }
    ]
}
```

## Error Handling

- **parse_json**: Returns the source value as-is (even if null or not JSON)
- **parse_xml**: Returns an error if the source is not a string or if XML parsing fails

## Next Steps

- [Map Function](./map.md) - Transform the parsed data
- [Validation Function](./validation.md) - Validate the data structure
- [Publish Functions](./publish.md) - Serialize data for output
