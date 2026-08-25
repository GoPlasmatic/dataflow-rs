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

XML is converted with [`quick-xml`](https://docs.rs/quick-xml)'s serde
deserializer, which reserves two key prefixes:

Rows show whole documents, since that is what `source` hands the parser:

| XML document | JSON at `data.<target>` |
|-----|------|
| `<root><name>Alice</name></root>` | `{"name": {"$text": "Alice"}}` |
| `<root><person id="1" role="admin"/></root>` | `{"person": {"@id": "1", "@role": "admin"}}` |
| `<root><empty/></root>` | `{"empty": {}}` |
| `<root><a><b>x</b></a></root>` | `{"a": {"b": {"$text": "x"}}}` |

Four consequences are worth planning for before you write the mappings that
read the result:

- **The outermost element is consumed, not represented.** `from_str`
  deserializes the document *into* the target, so the root tag contributes no
  key of its own: `<a><b>x</b></a>` on its own parses to `{"b": {"$text": "x"}}`.
  Paths into the result start at the root's children, which is why every row
  above needs a wrapper to show the element key at all.
- **Text content lives under `$text`, not directly under the element key.** A
  leaf element deserializes to an *object*, so the path to Alice's name is
  `data.request.name.$text` — reading `data.request.name` hands you
  `{"$text": "Alice"}`, and a condition comparing it to `"Alice"` is silently
  false.
- **Every value is a string.** `<age>30</age>` yields `{"$text": "30"}`: XML
  carries no type information, so compare against `"30"` or convert explicitly.
- **Repeated sibling elements do not become an array — only the last one
  survives.** `<root><item>a</item><item>b</item></root>` parses to
  `{"item": {"$text": "b"}}`, dropping `a`. For documents with repeated
  elements, parse them in a [custom handler](../advanced/custom-functions.md)
  that controls its own XML deserialization rather than using `parse_xml`.

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
            "name": {"$text": "Alice"},
            "email": {"$text": "alice@example.com"}
        }
    }
}
```

To lift those leaves into plain scalars, follow the parse with a `map`:

```json
{
    "id": "flatten_request",
    "function": {
        "name": "map",
        "input": {
            "mappings": [
                {"path": "data.user.name", "logic": {"var": "data.request.name.$text"}},
                {"path": "data.user.email", "logic": {"var": "data.request.email.$text"}}
            ]
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
                        {"path": "data.result", "logic": {"var": "data.apiResponse.result.$text"}}
                    ]
                }
            }
        }
    ]
}
```

## Error Handling

- **parse_json**: Never fails. A string source is parsed as JSON; if it does not
  parse, the string is stored as-is. A non-string source is stored unchanged.
- **parse_xml**: Returns an error if the source is not a string or if XML parsing fails

Note that a *successful* `parse_xml` can still lose data — repeated sibling
elements collapse to the last one, as described under
[XML to JSON Conversion](#xml-to-json-conversion). That is not reported as an
error.

## Next Steps

- [Map Function](./map.md) - Transform the parsed data
- [Validation Function](./validation.md) - Validate the data structure
- [Publish Functions](./publish.md) - Serialize data for output
