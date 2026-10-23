# Publish Functions

`publish_json` and `publish_xml` serialize a value from `data` into a formatted JSON or XML string under `data`. Workflows typically use them as a final step to prepare an output payload.

Both functions share a common configuration structure:

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `source` | string \| JSONLogic | Yes | — | Source path relative to `data` (for example, `"response"` reads `data.response`, and `"response.user"` reads `data.response.user`) |
| `target` | string \| JSONLogic | Yes | — | Destination path relative to `data` where the serialized string is written |
| `pretty` | boolean | No | `false` | Format the output as pretty-printed, indented JSON (`publish_json` only) |
| `root_element` | string \| JSONLogic | No | `"root"` | Name of the outer XML root element (`publish_xml` only) |

- Both `source` and `target` name paths relative to `data` (not inline values), and either can be a dynamic JSONLogic expression.
- For compatibility with 3.x workflows, a `source` that starts with `"data."` (such as `"data.response"`) first checks `data.data.response` and falls back to `data.response` if absent; an empty `source` (`""`) or `"data."` serializes the entire `data` object.
- Specifying `root_element` on `publish_json` or `pretty` on `publish_xml` is ignored and reported as an `UNKNOWN_KEY` advisory.
- **Missing or `null` source:** The value at `source` must exist and not be `null`; otherwise the task fails with `VALIDATION_ERROR` (status `500`, governed by `on_failure`):

  ```text
  Validation error: PublishJson: Source 'data.absent' not found or is null
  ```

- **No secrets:** Because `source`, `target`, and `root_element` affect data written to the message and audit trail, none of them may reference `{"secret": "..."}` (`SECRET_IN_MESSAGE_WRITE`). See [Secrets](../advanced/secrets.md).

## `publish_json`

Serializes the value at `data.<source>` into a JSON string at `data.<target>`:

```json
{
    "function": {
        "name": "publish_json",
        "input": {
            "source": "response",
            "target": "body"
        }
    }
}
```

When `data.response` is `{"status": "success", "count": 42}`, this writes:

```json
{
    "response": {"status": "success", "count": 42},
    "body": "{\"status\":\"success\",\"count\":42}"
}
```

With `"pretty": true`:

```json
{
    "pretty_body": "{\n  \"status\": \"success\",\n  \"count\": 42\n}"
}
```

Scalar values (strings, numbers, booleans) and arrays can also be serialized (for example, `"source": "response.count"` writes `"42"`). Both compact and pretty JSON output preserve the source object's key insertion order.

### Dynamic `source` and `target`

```json
{
    "source": {"cat": ["outputs.", {"var": "data.format"}]},
    "target": {"cat": ["rendered_", {"var": "data.format"}]}
}
```

When `data.format` is `"a"`, this serializes `data.outputs.a` into `data.rendered_a`.

## `publish_xml`

Serializes the value at `data.<source>` into an XML document string wrapped in `<root_element>`:

```json
{
    "function": {
        "name": "publish_xml",
        "input": {
            "source": "user",
            "target": "user_xml",
            "root_element": "User"
        }
    }
}
```

When `data.user` is `{"name": "Alice", "age": 30, "tags": ["a", "b"], "active": true, "note": null}`, this writes:

```json
{
    "user_xml": "<User><name>Alice</name><age>30</age><tags><item>a</item><item>b</item></tags><active>true</active><note></note></User>"
}
```

If `root_element` is omitted, the document is wrapped in `<root>`. Because `root_element` accepts a JSONLogic expression, you can also compute the root tag name dynamically from the message:

```json
{"source": "user", "target": "doc", "root_element": {"var": "data.doc_type"}}
```

### JSON-to-XML Conversion Rules

| JSON Value | XML Output |
|-------|------------|
| Object | One child element per key (in key order), named after the key |
| Array | One `<item>` element per array element |
| String | Escaped text content (`&`, `<`, `>`, `"`, and `'` are XML-escaped) |
| Number or boolean | Text representation (`30`, `true`, `false`) |
| `null` | Empty element (`<key></key>`) |

Object keys and `root_element` names are automatically sanitized into valid XML element names so untrusted or dynamic keys can never inject XML tags:
- If the first character is not an ASCII letter or `_`, a leading `_` is prepended.
- Any character other than an ASCII letter, digit, `-`, `_`, or `.` is replaced with `_`.
- An empty key becomes `_element`.

| JSON Key | Sanitized XML Tag |
|-----|---------|
| `123field` | `<_123field>` |
| `field name` | `<field_name>` |
| `field@attr` | `<field_attr>` |
| `ok-name_1.x` | `<ok-name_1.x>` |
| `""` (empty) | `<_element>` |

> **Note:** `publish_xml` is not a direct inverse of `parse_xml`. The `$text` and `@attr` keys produced by `parse_xml` are treated as ordinary object keys by `publish_xml` and sanitized into `<_text>` and `<_attr>` elements (`{"name": {"$text": "Alice"}}` serializes as `<root><name><_text>Alice</_text></name></root>`). Use a `map` task to flatten `$text` fields into plain values before calling `publish_xml`; see [Parse Functions → XML-to-JSON Conversion Rules](./parse.md#xml-to-json-conversion-rules).

## Common Patterns

### JSON Request In, JSON Response Out

```json
{
    "tasks": [
        {
            "id": "parse_request",
            "function": {"name": "parse_json", "input": {"source": "payload", "target": "request"}}
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
            "function": {"name": "publish_json", "input": {"source": "response", "target": "body"}}
        }
    ]
}
```

### XML In, XML Out

```json
{
    "tasks": [
        {
            "id": "parse",
            "function": {"name": "parse_xml", "input": {"source": "payload", "target": "input"}}
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
            "id": "publish",
            "function": {"name": "publish_xml", "input": {"source": "output", "target": "xml", "root_element": "Result"}}
        }
    ]
}
```

### Publishing Both JSON and XML

```json
{
    "tasks": [
        {
            "id": "publish_json",
            "function": {"name": "publish_json", "input": {"source": "response", "target": "json_output"}}
        },
        {
            "id": "publish_xml",
            "function": {"name": "publish_xml", "input": {"source": "response", "target": "xml_output", "root_element": "Response"}}
        }
    ]
}
```

### Continuing When the Source Is Missing

```json
{
    "id": "w",
    "tasks": [
        {
            "id": "publish",
            "on_failure": "continue",
            "function": {"name": "publish_json", "input": {"source": "absent", "target": "body"}}
        },
        {
            "id": "after",
            "function": {"name": "map", "input": {"mappings": [{"path": "data.after", "logic": true}]}}
        }
    ]
}
```

Here, `publish` records status `500` and a `VALIDATION_ERROR` (`Task publish error: Validation error: PublishJson: Source 'data.absent' not found or is null`), and because `"on_failure": "continue"` is set, `after` still runs and the run finishes as `CompletedWithErrors`.

## Next Steps

- [Parse](./parse.md) — Parse incoming JSON and XML payloads
- [Map](./map.md) — Transform and shape data before publishing
- [Validation](./validation.md) — Validate fields before publishing
