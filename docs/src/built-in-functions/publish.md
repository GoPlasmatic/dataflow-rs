# Publish Functions

`publish_json` and `publish_xml` serialize a value under `data` into a string
under `data`. A workflow typically calls one last, to prepare the text it
sends on.

Both share one config:

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `source` | string \| JSONLogic | Yes | | What to serialize, relative to `data`: `response` is `data.response`, `response.user` is `data.response.user` |
| `target` | string \| JSONLogic | Yes | | Where the text is written, relative to `data` |
| `pretty` | boolean | No | `false` | Indent the JSON. `publish_json` only |
| `root_element` | string \| JSONLogic | No | `"root"` | The document's root element. `publish_xml` only |

`source` and `target` name locations, never values, and either may be
computed from the message. A `source` that begins with `data.` is read with
3.x's fallback: `data.response` reads `data.data.response` and, when that is
missing, `data.response`; an empty `source` (`""`) or `"data."` serializes the
whole of `data`. Naming the other function's field (`root_element` on
`publish_json`, or `pretty` on `publish_xml`) is ignored and reported as an
`UNKNOWN_KEY` advisory at that key.

The source must exist and not be `null`. Otherwise the call fails with
`VALIDATION_ERROR`:

```text
Validation error: PublishJson: Source 'data.absent' not found or is null
```

The audit entry records `500`, the message gets the record as
`Task <id> error: <message>`, and the task's `on_failure` decides: by default
the failure reaches the workflow and stops the run; with
`"on_failure": "continue"` the next task runs. A computed parameter that
fails to evaluate fails the call the same way, with `LOGIC_ERROR`.

Both parameters name where the engine writes, and the destination is recorded
in each `Change`, so neither may read `{"secret": …}`, and nor may
`root_element`, which is written into the document. See
[Secrets](../advanced/secrets.md).

## publish_json

Serializes the source to JSON text: compact, or indented with `pretty`.

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

With `data.response` at `{"status": "success", "count": 42}`:

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

A scalar source serializes too: `"source": "response.count"` writes `"42"`.

### Key order

Compact and pretty JSON both keep the source's key order, in every build. A
value written as `z` then `a` publishes as `{"z":1,"a":…}`, compact or
indented. 3.x's default build sorted the keys of pretty JSON, and of XML,
unless `serde_json`'s `preserve_order` was on; 4.0 keeps the order in every
build.

### Computed parameters

```json
{
    "source": {"cat": ["outputs.", {"var": "data.format"}]},
    "target": {"cat": ["rendered_", {"var": "data.format"}]}
}
```

With `data.format` at `"a"`, this serializes `data.outputs.a` into
`data.rendered_a`.

## publish_xml

Serializes the source to an XML document under the root element.

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

With `data.user` at `{"name": "Alice", "age": 30, "tags": ["a", "b"], "active": true, "note": null}`:

```json
{
    "user_xml": "<User><name>Alice</name><age>30</age><tags><item>a</item><item>b</item></tags><active>true</active><note></note></User>"
}
```

Without `root_element` the document is wrapped in `<root>`. `root_element`
is JSONLogic too, so one task can name the document after the message:

```json
{"source": "user", "target": "doc", "root_element": {"var": "data.doc_type"}}
```

The root element's name is sanitized as a key's is (below), so a computed
name can never close the root and open elements of its own:
`"x><admin>true</admin><x"` names the element `x__admin_true__admin__x`.

### JSON to XML conversion

| Value | Written as |
|-------|------------|
| An object | One element per key, in the object's order, named after the key |
| An array | One `<item>` element per element |
| A string | Its text, with `&`, `<`, `>`, `"` and `'` escaped |
| A number or boolean | Its text |
| `null` | An empty element |

A key that is not an XML name is sanitized: a first character that is not a
letter or `_` gets a `_` in front, any other character that is not a letter,
digit, `-`, `_` or `.` becomes `_`, and an empty key becomes `_element`.

| Key | Element |
|-----|---------|
| `123field` | `<_123field>` |
| `field name` | `<field_name>` |
| `field@attr` | `<field_attr>` |
| `ok-name_1.x` | `<ok-name_1.x>` |
| (empty) | `<_element>` |

`publish_xml` is not the inverse of `parse_xml`. The `$text` and `@name` keys
`parse_xml` produces are ordinary keys here, and sanitization turns them into
`<_text>` and `<_name>` elements, not a text node or an attribute:
`{"name": {"$text": "Alice"}}` publishes as
`<root><name><_text>Alice</_text></name></root>`. Lift the `$text` leaves
with a `map` first; see
[XML to JSON conversion](./parse.md#xml-to-json-conversion).

## Common patterns

### Request in, response out

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

### XML in, XML out

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

### Both formats

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

### A missing source, without stopping the run

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

`publish` records `500` and the record
`VALIDATION_ERROR` `Task publish error: Validation error: PublishJson: Source 'data.absent' not found or is null`,
and `after` runs. The run ends `CompletedWithErrors`.

## Next Steps

- [Parse](./parse.md): read the input
- [Map](./map.md): shape the value to publish
- [Validation](./validation.md): check it before publishing
