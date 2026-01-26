# Publish Functions

The publish functions serialize structured data into string formats (JSON or XML). They are typically used at the end of a workflow to prepare output data for transmission or storage.

## publish_json

Serializes data from the source field to a JSON string.

### Configuration

```json
{
    "function": {
        "name": "publish_json",
        "input": {
            "source": "output",
            "target": "json_string"
        }
    }
}
```

### Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `source` | string | Yes | - | Field name in data to serialize (e.g., `output` or `nested.field`) |
| `target` | string | Yes | - | Field name where the JSON string will be stored |
| `pretty` | boolean | No | `false` | Whether to pretty-print the JSON output |

### Examples

#### Serialize Data to JSON

```json
{
    "id": "publish_response",
    "function": {
        "name": "publish_json",
        "input": {
            "source": "response",
            "target": "responseBody"
        }
    }
}
```

**Input:**
```json
{
    "data": {
        "response": {"status": "success", "count": 42}
    }
}
```

**Result:**
```json
{
    "data": {
        "response": {"status": "success", "count": 42},
        "responseBody": "{\"status\":\"success\",\"count\":42}"
    }
}
```

#### Pretty-Print JSON

```json
{
    "id": "publish_pretty",
    "function": {
        "name": "publish_json",
        "input": {
            "source": "user",
            "target": "userJson",
            "pretty": true
        }
    }
}
```

**Result:**
```json
{
    "data": {
        "userJson": "{\n  \"name\": \"Alice\",\n  \"age\": 30\n}"
    }
}
```

---

## publish_xml

Serializes data from the source field to an XML string.

### Configuration

```json
{
    "function": {
        "name": "publish_xml",
        "input": {
            "source": "output",
            "target": "xml_string",
            "root_element": "Response"
        }
    }
}
```

### Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `source` | string | Yes | - | Field name in data to serialize |
| `target` | string | Yes | - | Field name where the XML string will be stored |
| `root_element` | string | No | `root` | Name of the root XML element |

### JSON to XML Conversion

The serializer follows these rules:
- Object keys become XML element names
- Array items are wrapped in `<item>` elements
- Special characters are properly escaped (`<`, `>`, `&`, `"`, `'`)
- Invalid XML element names are sanitized (e.g., names starting with numbers get an underscore prefix)

### Examples

#### Serialize Data to XML

```json
{
    "id": "publish_xml_response",
    "function": {
        "name": "publish_xml",
        "input": {
            "source": "user",
            "target": "userXml",
            "root_element": "User"
        }
    }
}
```

**Input:**
```json
{
    "data": {
        "user": {"name": "Alice", "age": 30}
    }
}
```

**Result:**
```json
{
    "data": {
        "user": {"name": "Alice", "age": 30},
        "userXml": "<User><name>Alice</name><age>30</age></User>"
    }
}
```

#### Serialize Nested Data

```json
{
    "id": "publish_nested",
    "function": {
        "name": "publish_xml",
        "input": {
            "source": "response.data",
            "target": "xmlOutput",
            "root_element": "Data"
        }
    }
}
```

---

## Common Patterns

### Complete API Pipeline

```json
{
    "tasks": [
        {
            "id": "parse_request",
            "function": {
                "name": "parse_json",
                "input": {"source": "payload", "target": "request"}
            }
        },
        {
            "id": "process",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "data.response.message", "logic": {"cat": ["Hello, ", {"var": "data.request.name"}]}}
                    ]
                }
            }
        },
        {
            "id": "publish_response",
            "function": {
                "name": "publish_json",
                "input": {"source": "response", "target": "body"}
            }
        }
    ]
}
```

### XML-to-XML Transformation

```json
{
    "tasks": [
        {
            "id": "parse_xml",
            "function": {
                "name": "parse_xml",
                "input": {"source": "payload", "target": "input"}
            }
        },
        {
            "id": "transform",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "data.output.result", "logic": {"var": "data.input.value"}}
                    ]
                }
            }
        },
        {
            "id": "publish_xml",
            "function": {
                "name": "publish_xml",
                "input": {"source": "output", "target": "xmlResponse", "root_element": "Result"}
            }
        }
    ]
}
```

### Generate Both JSON and XML Outputs

```json
{
    "tasks": [
        {
            "id": "publish_json",
            "function": {
                "name": "publish_json",
                "input": {"source": "response", "target": "jsonOutput"}
            }
        },
        {
            "id": "publish_xml",
            "function": {
                "name": "publish_xml",
                "input": {"source": "response", "target": "xmlOutput", "root_element": "Response"}
            }
        }
    ]
}
```

## Error Handling

- **publish_json**: Returns an error if the source field is not found or is null
- **publish_xml**: Returns an error if the source field is not found or is null

## XML Element Name Sanitization

XML has strict rules for element names. The publish_xml function automatically sanitizes invalid names:

| Original | Sanitized |
|----------|-----------|
| `123field` | `_123field` |
| `field name` | `field_name` |
| `field@attr` | `field_attr` |
| `` (empty) | `_element` |

## Next Steps

- [Parse Functions](./parse.md) - Parse input data
- [Map Function](./map.md) - Transform data
- [Validation Function](./validation.md) - Validate before publishing
