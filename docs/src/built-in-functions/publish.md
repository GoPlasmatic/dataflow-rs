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
| `source` | string \| JSONLogic | Yes | - | Field name in data to serialize (e.g., `output` or `nested.field`) |
| `target` | string \| JSONLogic | Yes | - | Field name where the JSON string will be stored |
| `pretty` | boolean | No | `false` | Whether to pretty-print the JSON output. **Static**, so the output shape is known at build time |

`source` and `target` are JSONLogic, so either can be computed per message:

```json
{
    "source": {"cat": ["outputs.", {"var": "data.format"}]},
    "target": {"cat": ["rendered_", {"var": "data.format"}]}
}
```

Both resolve to the *name* of a location, not to the value at one. The static
spelling is a plain string, which is JSONLogic for itself: it folds to a
constant at build time and keeps the precomputed path split, so nothing changes
for the ordinary case.

Both name where the engine itself writes, and the destination is recorded in
`Change.path` and on the audit trail, so neither may read `{"secret": …}` — see
[Secrets](../advanced/secrets.md).

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
| `source` | string \| JSONLogic | Yes | - | Field name in data to serialize |
| `target` | string \| JSONLogic | Yes | - | Field name where the XML string will be stored |
| `root_element` | string \| JSONLogic | No | `root` | Name of the root XML element |

`source` and `target` accept a computed value exactly as for
[`publish_json`](#parameters), and so does `root_element` — so one task can name
the document after the message it is serializing:

```json
{"source": "output", "target": "xml", "root_element": {"var": "data.doc_type"}}
```

`root_element` is written into the serialized document that lands in
`data.{target}`, so it may not read a secret either.

### JSON to XML Conversion

The serializer follows these rules:
- Object keys become XML element names
- Array items are wrapped in `<item>` elements
- Special characters are properly escaped (`<`, `>`, `&`, `"`, `'`)
- Invalid XML element names are sanitized (e.g., names starting with numbers get an underscore prefix)

`publish_xml` is **not** the inverse of `parse_xml`. It has no notion of the
`$text` and `@name` keys `parse_xml` produces — they are ordinary object keys,
and sanitization turns them into `<_text>` and `<_name>` *elements* rather than
a text node or an attribute. Lift the `$text` leaves with a `map` before
publishing; see
[XML to JSON Conversion](./parse.md#xml-to-json-conversion).

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
                        {"path": "data.output.result", "logic": {"var": "data.input.value.$text"}}
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
